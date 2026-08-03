import { useState, useCallback, useRef, useEffect } from "react";
import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { authApi, isRemoteWebMode } from "@/lib/api";
import type {
  ManagedAuthStatus,
  ManagedAuthDeviceCodeResponse,
} from "@/lib/api";
import { oauthQuotaRootKey } from "@/lib/query/oauthQuotaKeys";

type AuthState =
  "idle" | "waiting_browser" | "waiting_paste" | "success" | "error";
export type ClaudeOAuthFlowMode = "localhost" | "web_paste";

interface ClaudeAuthStartParams {
  operationGeneration: number;
  flowMode?: ClaudeOAuthFlowMode;
}

interface ClaudePasteCodeParams {
  operationGeneration: number;
  deviceCode: string;
  code: string;
}

export function useClaudeOauth() {
  const queryClient = useQueryClient();
  const queryKey = ["managed-auth-status", "claude_oauth"];

  const [authState, setAuthState] = useState<AuthState>("idle");
  const [deviceCode, setDeviceCode] =
    useState<ManagedAuthDeviceCodeResponse | null>(null);
  const [error, setError] = useState<string | null>(null);

  const pollingTimeoutRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const operationGenerationRef = useRef(0);

  const {
    data: authStatus,
    isLoading: isLoadingStatus,
    isFetching: isFetchingStatus,
    isError: isStatusError,
    error: statusQueryError,
    refetch: refetchStatus,
  } = useQuery<ManagedAuthStatus>({
    queryKey,
    queryFn: () => authApi.authGetStatus("claude_oauth"),
    staleTime: 30000,
  });

  const invalidateClaudeAccountViews = useCallback(
    () =>
      Promise.all([
        queryClient.invalidateQueries({ queryKey }),
        queryClient.invalidateQueries({ queryKey: ["managed-auth-accounts"] }),
        queryClient.invalidateQueries({ queryKey: ["subscription"] }),
        queryClient.invalidateQueries({
          queryKey: oauthQuotaRootKey("claude_oauth"),
        }),
        queryClient.invalidateQueries({ queryKey: ["providers"] }),
        queryClient.invalidateQueries({ queryKey: ["share"] }),
      ]),
    [queryClient],
  );

  const stopPolling = useCallback(() => {
    if (pollingTimeoutRef.current) {
      clearTimeout(pollingTimeoutRef.current);
      pollingTimeoutRef.current = null;
    }
  }, []);

  const beginOperation = useCallback(() => {
    operationGenerationRef.current += 1;
    return operationGenerationRef.current;
  }, []);

  const isCurrentOperation = useCallback(
    (generation: number) => operationGenerationRef.current === generation,
    [],
  );

  const retireOperation = useCallback((generation: number) => {
    if (operationGenerationRef.current !== generation) return null;
    operationGenerationRef.current += 1;
    return operationGenerationRef.current;
  }, []);

  const cancelRemoteLogin = useCallback(async (activeDeviceCode: string) => {
    try {
      await authApi.authCancelLogin("claude_oauth", activeDeviceCode);
    } catch (e) {
      console.warn("[ClaudeOAuth] Failed to cancel remote auth session:", e);
    }
  }, []);

  useEffect(() => {
    return () => {
      operationGenerationRef.current += 1;
      stopPolling();
    };
  }, [stopPolling]);

  const startLoginMutation = useMutation({
    mutationFn: (params: ClaudeAuthStartParams) =>
      authApi.authStartLogin(
        "claude_oauth",
        undefined,
        // 远程 web 模式（通过 client URL 访问）走 platform.claude.com out-of-band
        // 回调；桌面 Tauri 模式默认继续走 127.0.0.1:54545，但也允许用户
        // 显式选择 platform.claude.com 官方回调。
        params.flowMode ?? (isRemoteWebMode() ? "web_paste" : undefined),
      ),
    onSuccess: async (response, params) => {
      const generation = params.operationGeneration;
      if (!isCurrentOperation(generation)) {
        void cancelRemoteLogin(response.device_code);
        return;
      }
      setDeviceCode(response);
      setError(null);

      const flowMode =
        params.flowMode ?? (isRemoteWebMode() ? "web_paste" : "localhost");

      if (flowMode === "web_paste") {
        // Web-paste 模式：等用户从 platform.claude.com 复制 code 后调
        // submitPasteCode，没有自动轮询；只设个超时清掉 deviceCode。
        setAuthState("waiting_paste");
        const expiresMs = response.expires_in * 1000;
        pollingTimeoutRef.current = setTimeout(() => {
          if (retireOperation(generation) === null) return;
          stopPolling();
          setAuthState("error");
          setError("授权超时，请重试。");
        }, expiresMs);
        return;
      }

      // 本机回调模式：原有的本机回调 + 轮询。
      setAuthState("waiting_browser");
      const interval = (response.interval || 3) * 1000;
      const expiresAt = Date.now() + response.expires_in * 1000;

      const schedulePoll = () => {
        if (!isCurrentOperation(generation)) return;
        if (Date.now() > expiresAt) {
          if (retireOperation(generation) === null) return;
          stopPolling();
          setAuthState("error");
          setError("授权超时，请重试。");
          return;
        }

        pollingTimeoutRef.current = setTimeout(async () => {
          try {
            const newAccount = await authApi.authPollForAccount(
              "claude_oauth",
              response.device_code,
            );
            if (!isCurrentOperation(generation)) return;
            if (newAccount) {
              const completionGeneration = retireOperation(generation);
              if (completionGeneration === null) return;
              stopPolling();
              setAuthState("success");
              await invalidateClaudeAccountViews();
              if (!isCurrentOperation(completionGeneration)) return;
              setAuthState("idle");
              setDeviceCode(null);
              setError(null);
              return;
            }
          } catch (e) {
            if (!isCurrentOperation(generation)) return;
            const errorMessage = e instanceof Error ? e.message : String(e);
            if (
              !errorMessage.includes("pending") &&
              !errorMessage.includes("slow_down") &&
              !errorMessage.includes("authorization_pending")
            ) {
              if (retireOperation(generation) === null) return;
              stopPolling();
              setAuthState("error");
              setError(errorMessage);
              return;
            }
          }
          if (!isCurrentOperation(generation)) return;
          schedulePoll();
        }, interval);
      };

      schedulePoll();
    },
    onError: (e, params) => {
      if (retireOperation(params.operationGeneration) === null) return;
      stopPolling();
      setAuthState("error");
      setError(e instanceof Error ? e.message : String(e));
    },
  });

  const submitPasteCodeMutation = useMutation({
    mutationFn: async ({ deviceCode: dc, code }: ClaudePasteCodeParams) =>
      authApi.authSubmitOauthCode("claude_oauth", dc, code),
    onSuccess: async (_, params) => {
      const completionGeneration = retireOperation(params.operationGeneration);
      if (completionGeneration === null) return;
      stopPolling();
      setAuthState("success");
      await invalidateClaudeAccountViews();
      if (!isCurrentOperation(completionGeneration)) return;
      setAuthState("idle");
      setDeviceCode(null);
      setError(null);
    },
    onError: (e, params) => {
      if (!isCurrentOperation(params.operationGeneration)) return;
      // 失败时让用户能重试粘贴，不复位 deviceCode。
      setError(e instanceof Error ? e.message : String(e));
    },
  });

  const logoutMutation = useMutation({
    mutationFn: () => authApi.authLogout("claude_oauth"),
    onMutate: () => setError(null),
    onSuccess: async () => {
      setAuthState("idle");
      setDeviceCode(null);
      setError(null);
      queryClient.setQueryData(queryKey, {
        provider: "claude_oauth",
        authenticated: false,
        default_account_id: null,
        accounts: [],
      });
      await invalidateClaudeAccountViews();
    },
    onError: async (e) => {
      console.error("[ClaudeOAuth] Failed to logout:", e);
      setError(e instanceof Error ? e.message : String(e));
      await refetchStatus();
    },
  });

  const removeAccountMutation = useMutation({
    mutationFn: (accountId: string) =>
      authApi.authRemoveAccount("claude_oauth", accountId),
    onMutate: () => setError(null),
    onSuccess: async () => {
      setAuthState("idle");
      setDeviceCode(null);
      setError(null);
      await invalidateClaudeAccountViews();
    },
    onError: (e) => {
      console.error("[ClaudeOAuth] Failed to remove account:", e);
      setError(e instanceof Error ? e.message : String(e));
    },
  });

  const setDefaultAccountMutation = useMutation({
    mutationFn: (accountId: string) =>
      authApi.authSetDefaultAccount("claude_oauth", accountId),
    onMutate: () => setError(null),
    onSuccess: async () => {
      await invalidateClaudeAccountViews();
    },
    onError: (e) => {
      console.error("[ClaudeOAuth] Failed to set default account:", e);
      setError(e instanceof Error ? e.message : String(e));
    },
  });

  const startAuth = useCallback(
    (flowMode?: ClaudeOAuthFlowMode) => {
      const activeDeviceCode = deviceCode?.device_code;
      stopPolling();
      const operationGeneration = beginOperation();
      if (activeDeviceCode) {
        void cancelRemoteLogin(activeDeviceCode);
      }
      setAuthState("idle");
      setDeviceCode(null);
      setError(null);
      startLoginMutation.mutate({ operationGeneration, flowMode });
    },
    [
      beginOperation,
      cancelRemoteLogin,
      deviceCode?.device_code,
      startLoginMutation,
      stopPolling,
    ],
  );

  const cancelAuth = useCallback(() => {
    const activeDeviceCode = deviceCode?.device_code;
    beginOperation();
    stopPolling();
    setAuthState("idle");
    setDeviceCode(null);
    setError(null);
    if (activeDeviceCode) {
      void cancelRemoteLogin(activeDeviceCode);
    }
  }, [beginOperation, cancelRemoteLogin, deviceCode?.device_code, stopPolling]);

  const logout = useCallback(() => {
    const activeDeviceCode = deviceCode?.device_code;
    beginOperation();
    stopPolling();
    setAuthState("idle");
    setDeviceCode(null);
    setError(null);
    if (activeDeviceCode) {
      void cancelRemoteLogin(activeDeviceCode).then(() => {
        logoutMutation.mutate();
      });
      return;
    }
    logoutMutation.mutate();
  }, [
    beginOperation,
    cancelRemoteLogin,
    deviceCode?.device_code,
    logoutMutation,
    stopPolling,
  ]);

  const logoutAsync = useCallback(async () => {
    const activeDeviceCode = deviceCode?.device_code;
    beginOperation();
    stopPolling();
    setAuthState("idle");
    setDeviceCode(null);
    setError(null);
    if (activeDeviceCode) {
      await cancelRemoteLogin(activeDeviceCode);
    }
    return logoutMutation.mutateAsync();
  }, [
    beginOperation,
    cancelRemoteLogin,
    deviceCode?.device_code,
    logoutMutation,
    stopPolling,
  ]);

  const removeAccount = useCallback(
    (accountId: string) => {
      removeAccountMutation.mutate(accountId);
    },
    [removeAccountMutation],
  );

  const setDefaultAccount = useCallback(
    (accountId: string) => {
      setDefaultAccountMutation.mutate(accountId);
    },
    [setDefaultAccountMutation],
  );

  const submitPasteCode = useCallback(
    (code: string) => {
      const dc = deviceCode?.device_code;
      if (!dc) {
        setError("授权流程未启动或已过期，请重新点击登录。");
        return;
      }
      const trimmed = code.trim();
      if (!trimmed) {
        setError("请粘贴 platform.claude.com 上显示的授权码。");
        return;
      }
      setError(null);
      submitPasteCodeMutation.mutate({
        operationGeneration: operationGenerationRef.current,
        deviceCode: dc,
        code: trimmed,
      });
    },
    [deviceCode, submitPasteCodeMutation],
  );

  const accounts = authStatus?.accounts ?? [];
  const statusErrorMessage = statusQueryError
    ? statusQueryError instanceof Error
      ? statusQueryError.message
      : String(statusQueryError)
    : null;

  return {
    authStatus,
    isLoadingStatus,
    isFetchingStatus,
    isStatusError,
    accounts,
    hasAnyAccount: accounts.length > 0,
    isAuthenticated: authStatus?.authenticated ?? false,
    defaultAccountId: authStatus?.default_account_id ?? null,
    authState,
    deviceCode,
    error: error ?? statusErrorMessage,
    isWaitingBrowser: authState === "waiting_browser",
    isWaitingPaste: authState === "waiting_paste",
    isSubmittingPaste: submitPasteCodeMutation.isPending,
    isAddingAccount:
      startLoginMutation.isPending ||
      authState === "waiting_browser" ||
      authState === "waiting_paste",
    canUseLocalCallback: !isRemoteWebMode(),
    isRemovingAccount: removeAccountMutation.isPending,
    isSettingDefaultAccount: setDefaultAccountMutation.isPending,
    startAuth,
    addAccount: startAuth,
    cancelAuth,
    submitPasteCode,
    logout,
    logoutAsync,
    removeAccount,
    removeAccountAsync: removeAccountMutation.mutateAsync,
    setDefaultAccount,
    refetchStatus,
  };
}
