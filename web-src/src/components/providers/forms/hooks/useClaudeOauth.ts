import { useState, useCallback, useRef, useEffect } from "react";
import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { authApi, isRemoteWebMode } from "@/lib/api";
import type {
  ManagedAuthStatus,
  ManagedAuthDeviceCodeResponse,
} from "@/lib/api";

type AuthState =
  | "idle"
  | "waiting_browser"
  | "waiting_paste"
  | "success"
  | "error";
export type ClaudeOAuthFlowMode = "localhost" | "web_paste";

export function useClaudeOauth() {
  const queryClient = useQueryClient();
  const queryKey = ["managed-auth-status", "claude_oauth"];

  const [authState, setAuthState] = useState<AuthState>("idle");
  // authStateRef 给 setTimeout 闭包用：里面拿不到最新的 authState 值（闭包捕获的是
  // 触发 timer 那一刻的值）。waiting_paste 的超时回调要根据当时状态决定是否报错。
  const authStateRef = useRef<AuthState>("idle");
  useEffect(() => {
    authStateRef.current = authState;
  }, [authState]);
  const [deviceCode, setDeviceCode] =
    useState<ManagedAuthDeviceCodeResponse | null>(null);
  const [error, setError] = useState<string | null>(null);

  const pollingTimeoutRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  const {
    data: authStatus,
    isLoading: isLoadingStatus,
    refetch: refetchStatus,
  } = useQuery<ManagedAuthStatus>({
    queryKey,
    queryFn: () => authApi.authGetStatus("claude_oauth"),
    staleTime: 30000,
  });

  const stopPolling = useCallback(() => {
    if (pollingTimeoutRef.current) {
      clearTimeout(pollingTimeoutRef.current);
      pollingTimeoutRef.current = null;
    }
  }, []);

  useEffect(() => {
    return () => {
      stopPolling();
    };
  }, [stopPolling]);

  const startLoginMutation = useMutation({
    mutationFn: (requestedFlowMode?: ClaudeOAuthFlowMode) =>
      authApi.authStartLogin(
        "claude_oauth",
        undefined,
        // 远程 web 模式（通过 client URL 访问）走 platform.claude.com out-of-band
        // 回调；桌面 Tauri 模式默认继续走 127.0.0.1:54545，但也允许用户
        // 显式选择 platform.claude.com 官方回调。
        requestedFlowMode ?? (isRemoteWebMode() ? "web_paste" : undefined),
      ),
    onSuccess: async (response, requestedFlowMode) => {
      setDeviceCode(response);
      setError(null);

      const flowMode =
        requestedFlowMode ?? (isRemoteWebMode() ? "web_paste" : "localhost");

      if (flowMode === "web_paste") {
        // Web-paste 模式：等用户从 platform.claude.com 复制 code 后调
        // submitPasteCode，没有自动轮询；只设个超时清掉 deviceCode。
        setAuthState("waiting_paste");
        const expiresMs = response.expires_in * 1000;
        pollingTimeoutRef.current = setTimeout(() => {
          // 超时只复位状态，不报错——用户可能正在 claude.ai 上慢慢操作。
          if (authStateRef.current === "waiting_paste") {
            setAuthState("error");
            setError("授权超时，请重试。");
          }
        }, expiresMs);
        return;
      }

      // 本机回调模式：原有的本机回调 + 轮询。
      setAuthState("waiting_browser");
      const interval = (response.interval || 3) * 1000;
      const expiresAt = Date.now() + response.expires_in * 1000;

      const schedulePoll = () => {
        if (Date.now() > expiresAt) {
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
            if (newAccount) {
              stopPolling();
              setAuthState("success");
              await refetchStatus();
              await queryClient.invalidateQueries({ queryKey });
              setAuthState("idle");
              setDeviceCode(null);
              return;
            }
          } catch (e) {
            const errorMessage = e instanceof Error ? e.message : String(e);
            if (
              !errorMessage.includes("pending") &&
              !errorMessage.includes("slow_down") &&
              !errorMessage.includes("authorization_pending")
            ) {
              stopPolling();
              setAuthState("error");
              setError(errorMessage);
              return;
            }
          }
          // 本次轮询完成后再安排下一次
          schedulePoll();
        }, interval);
      };

      schedulePoll();
    },
    onError: (e) => {
      setAuthState("error");
      setError(e instanceof Error ? e.message : String(e));
    },
  });

  const submitPasteCodeMutation = useMutation({
    mutationFn: async ({
      deviceCode: dc,
      code,
    }: {
      deviceCode: string;
      code: string;
    }) => authApi.authSubmitOauthCode("claude_oauth", dc, code),
    onSuccess: async () => {
      stopPolling();
      setAuthState("success");
      await refetchStatus();
      await queryClient.invalidateQueries({ queryKey });
      setAuthState("idle");
      setDeviceCode(null);
      setError(null);
    },
    onError: (e) => {
      // 失败时让用户能重试粘贴，不复位 deviceCode。
      setError(e instanceof Error ? e.message : String(e));
    },
  });

  const logoutMutation = useMutation({
    mutationFn: () => authApi.authLogout("claude_oauth"),
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
      await queryClient.invalidateQueries({ queryKey });
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
    onSuccess: async () => {
      setAuthState("idle");
      setDeviceCode(null);
      setError(null);
      await refetchStatus();
      await queryClient.invalidateQueries({ queryKey });
    },
    onError: (e) => {
      console.error("[ClaudeOAuth] Failed to remove account:", e);
      setError(e instanceof Error ? e.message : String(e));
    },
  });

  const setDefaultAccountMutation = useMutation({
    mutationFn: (accountId: string) =>
      authApi.authSetDefaultAccount("claude_oauth", accountId),
    onSuccess: async () => {
      await refetchStatus();
      await queryClient.invalidateQueries({ queryKey });
    },
    onError: (e) => {
      console.error("[ClaudeOAuth] Failed to set default account:", e);
      setError(e instanceof Error ? e.message : String(e));
    },
  });

  const startAuth = useCallback(
    (flowMode?: ClaudeOAuthFlowMode) => {
      setAuthState("idle");
      setDeviceCode(null);
      setError(null);
      stopPolling();
      startLoginMutation.mutate(flowMode);
    },
    [startLoginMutation, stopPolling],
  );

  const cancelAuth = useCallback(() => {
    stopPolling();
    setAuthState("idle");
    setDeviceCode(null);
    setError(null);
  }, [stopPolling]);

  const logout = useCallback(() => {
    logoutMutation.mutate();
  }, [logoutMutation]);

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
      submitPasteCodeMutation.mutate({ deviceCode: dc, code: trimmed });
    },
    [deviceCode, submitPasteCodeMutation],
  );

  const accounts = authStatus?.accounts ?? [];

  return {
    authStatus,
    isLoadingStatus,
    accounts,
    hasAnyAccount: accounts.length > 0,
    isAuthenticated: authStatus?.authenticated ?? false,
    defaultAccountId: authStatus?.default_account_id ?? null,
    authState,
    deviceCode,
    error,
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
    removeAccount,
    setDefaultAccount,
    refetchStatus,
  };
}
