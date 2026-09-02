import { useState, useCallback, useRef, useEffect } from "react";
import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { authApi } from "@/lib/api";
import type {
  ManagedAuthProvider,
  ManagedAuthStatus,
  ManagedAuthDeviceCodeResponse,
  QoderSite,
  CodeBuddySite,
} from "@/lib/api";
import { oauthQuotaRootKey } from "@/lib/query/oauthQuotaKeys";

type PollingState = "idle" | "polling" | "success" | "error";

interface ManagedAuthStartParams {
  operationGeneration: number;
  oauthFlowMode?: "web_paste" | "localhost" | "cli" | "cli_manual" | "device";
  kiroLoginProvider?: "google" | "github" | null;
  qoderSite?: QoderSite | null;
  codeBuddySite?: CodeBuddySite | null;
  accountId?: string | null;
}

interface ManagedAuthCallbackParams {
  operationGeneration: number;
  deviceCode: string;
  callbackUrl: string;
}

export function useManagedAuth(
  authProvider: ManagedAuthProvider,
  githubDomain?: string,
) {
  const queryClient = useQueryClient();
  const queryKey = ["managed-auth-status", authProvider];

  const [pollingState, setPollingState] = useState<PollingState>("idle");
  const [deviceCode, setDeviceCode] =
    useState<ManagedAuthDeviceCodeResponse | null>(null);
  const [error, setError] = useState<string | null>(null);

  const pollingIntervalRef = useRef<ReturnType<typeof setInterval> | null>(
    null,
  );
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
    queryFn: () => authApi.authGetStatus(authProvider),
    staleTime: 30000,
  });

  const invalidateManagedAccountViews = useCallback(
    () =>
      Promise.all([
        queryClient.invalidateQueries({ queryKey }),
        queryClient.invalidateQueries({ queryKey: ["managed-auth-accounts"] }),
        queryClient.invalidateQueries({ queryKey: ["subscription"] }),
        queryClient.invalidateQueries({
          queryKey: oauthQuotaRootKey(authProvider),
        }),
        queryClient.invalidateQueries({ queryKey: ["providers"] }),
        queryClient.invalidateQueries({ queryKey: ["share"] }),
      ]),
    [authProvider, queryClient],
  );

  const stopPolling = useCallback(() => {
    if (pollingIntervalRef.current) {
      clearInterval(pollingIntervalRef.current);
      pollingIntervalRef.current = null;
    }
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

  const cancelRemoteLogin = useCallback(
    async (activeDeviceCode: string) => {
      try {
        await authApi.authCancelLogin(authProvider, activeDeviceCode);
      } catch (e) {
        console.warn("[ManagedAuth] Failed to cancel remote auth session:", e);
      }
    },
    [authProvider],
  );

  useEffect(() => {
    return () => {
      operationGenerationRef.current += 1;
      stopPolling();
    };
  }, [stopPolling]);

  const startLoginMutation = useMutation({
    mutationFn: (params: ManagedAuthStartParams) =>
      authApi.authStartLogin(
        authProvider,
        githubDomain,
        params.oauthFlowMode,
        params.kiroLoginProvider,
        params.qoderSite,
        params.codeBuddySite,
        params.accountId,
      ),
    onSuccess: async (response, params) => {
      const generation = params.operationGeneration;
      if (!isCurrentOperation(generation)) {
        void cancelRemoteLogin(response.device_code);
        return;
      }
      setDeviceCode(response);
      setPollingState("polling");
      setError(null);

      if (response.flow === "cli_manual") {
        pollingTimeoutRef.current = setTimeout(() => {
          if (retireOperation(generation) === null) return;
          stopPolling();
          setPollingState("error");
          setError("OAuth session expired. Please try again.");
        }, response.expires_in * 1000);
        return;
      }

      // Add a small buffer on top of GitHub's suggested interval to avoid
      // hitting slow_down responses too aggressively during device polling.
      const interval = Math.max((response.interval || 5) + 3, 8) * 1000;
      const expiresAt = Date.now() + response.expires_in * 1000;

      const pollOnce = async () => {
        if (!isCurrentOperation(generation)) return;
        if (Date.now() > expiresAt) {
          if (retireOperation(generation) === null) return;
          stopPolling();
          setPollingState("error");
          setError("Device code expired. Please try again.");
          return;
        }

        try {
          const newAccount = await authApi.authPollForAccount(
            authProvider,
            response.device_code,
            githubDomain,
            response.state,
          );
          if (!isCurrentOperation(generation)) return;
          if (newAccount) {
            const completionGeneration = retireOperation(generation);
            if (completionGeneration === null) return;
            stopPolling();
            setPollingState("success");
            await invalidateManagedAccountViews();
            if (!isCurrentOperation(completionGeneration)) return;
            setPollingState("idle");
            setDeviceCode(null);
            setError(null);
            return;
          }
        } catch (e) {
          if (!isCurrentOperation(generation)) return;
          const errorMessage = e instanceof Error ? e.message : String(e);
          if (
            !errorMessage.includes("pending") &&
            !errorMessage.includes("slow_down")
          ) {
            if (retireOperation(generation) === null) return;
            stopPolling();
            setPollingState("error");
            setError(errorMessage);
            return;
          }
        }

        if (!isCurrentOperation(generation)) return;
        pollingIntervalRef.current = setTimeout(() => {
          void pollOnce();
        }, interval);
      };

      void pollOnce();
      pollingTimeoutRef.current = setTimeout(() => {
        if (retireOperation(generation) === null) return;
        stopPolling();
        setPollingState("error");
        setError("Device code expired. Please try again.");
      }, response.expires_in * 1000);
    },
    onError: (e, params) => {
      if (retireOperation(params.operationGeneration) === null) return;
      stopPolling();
      setPollingState("error");
      setError(e instanceof Error ? e.message : String(e));
    },
  });

  const submitOauthCallbackMutation = useMutation({
    mutationFn: (params: ManagedAuthCallbackParams) => {
      return authApi.authSubmitOauthCallback(
        authProvider,
        params.deviceCode,
        params.callbackUrl,
      );
    },
    onSuccess: async (_, params) => {
      const completionGeneration = retireOperation(params.operationGeneration);
      if (completionGeneration === null) return;
      stopPolling();
      setPollingState("success");
      await refetchStatus();
      await invalidateManagedAccountViews();
      if (!isCurrentOperation(completionGeneration)) return;
      setPollingState("idle");
      setDeviceCode(null);
      setError(null);
    },
    onError: (e, params) => {
      if (!isCurrentOperation(params.operationGeneration)) return;
      setError(e instanceof Error ? e.message : String(e));
    },
  });

  const logoutMutation = useMutation({
    mutationFn: () => authApi.authLogout(authProvider),
    onMutate: () => setError(null),
    onSuccess: async () => {
      setPollingState("idle");
      setDeviceCode(null);
      setError(null);
      queryClient.setQueryData(queryKey, {
        provider: authProvider,
        authenticated: false,
        default_account_id: null,
        codex_oauth:
          authProvider === "codex_oauth"
            ? {
                status: "unconfigured",
                accountCount: 0,
                activeAccountId: null,
              }
            : null,
        accounts: [],
      });
      await invalidateManagedAccountViews();
    },
    onError: async (e) => {
      console.error("[ManagedAuth] Failed to logout:", e);
      setError(e instanceof Error ? e.message : String(e));
      await refetchStatus();
    },
  });

  const removeAccountMutation = useMutation({
    mutationFn: (accountId: string) =>
      authApi.authRemoveAccount(authProvider, accountId),
    onMutate: () => setError(null),
    onSuccess: async () => {
      setPollingState("idle");
      setDeviceCode(null);
      setError(null);
      await refetchStatus();
      await invalidateManagedAccountViews();
    },
    onError: (e) => {
      console.error("[ManagedAuth] Failed to remove account:", e);
      setError(e instanceof Error ? e.message : String(e));
    },
  });

  const setDefaultAccountMutation = useMutation({
    mutationFn: (accountId: string) =>
      authApi.authSetDefaultAccount(authProvider, accountId),
    onMutate: () => setError(null),
    onSuccess: async () => {
      setError(null);
      await refetchStatus();
      await invalidateManagedAccountViews();
    },
    onError: (e) => {
      console.error("[ManagedAuth] Failed to set default account:", e);
      setError(e instanceof Error ? e.message : String(e));
    },
  });

  const setWorkspaceMutation = useMutation({
    mutationFn: (params: { accountId: string; workspaceId: string }) =>
      authApi.authSetWorkspace(
        authProvider,
        params.accountId,
        params.workspaceId,
      ),
    onMutate: () => setError(null),
    onSuccess: async () => {
      setError(null);
      await refetchStatus();
      await invalidateManagedAccountViews();
    },
    onError: (e) => {
      console.error("[ManagedAuth] Failed to set workspace:", e);
      setError(e instanceof Error ? e.message : String(e));
    },
  });

  const importCursorLocalMutation = useMutation({
    mutationFn: () => authApi.importCursorLocalAuth(),
    onMutate: () => setError(null),
    onSuccess: async () => {
      setPollingState("idle");
      setDeviceCode(null);
      setError(null);
      await invalidateManagedAccountViews();
    },
    onError: (e) => {
      console.error("[ManagedAuth] Failed to import local Cursor auth:", e);
      setError(e instanceof Error ? e.message : String(e));
    },
  });

  const startAuth = useCallback(
    (
      oauthFlowMode?:
        "web_paste" | "localhost" | "cli" | "cli_manual" | "device",
      options?: {
        kiroLoginProvider?: "google" | "github" | null;
        qoderSite?: QoderSite | null;
        codeBuddySite?: CodeBuddySite | null;
        accountId?: string | null;
      },
    ) => {
      const activeDeviceCode = deviceCode?.device_code;
      stopPolling();
      const operationGeneration = beginOperation();
      if (activeDeviceCode) {
        void cancelRemoteLogin(activeDeviceCode);
      }
      setPollingState("idle");
      setDeviceCode(null);
      setError(null);
      startLoginMutation.mutate({
        operationGeneration,
        oauthFlowMode,
        kiroLoginProvider: options?.kiroLoginProvider,
        qoderSite: options?.qoderSite,
        codeBuddySite: options?.codeBuddySite,
        accountId: options?.accountId,
      });
    },
    [
      beginOperation,
      cancelRemoteLogin,
      deviceCode?.device_code,
      startLoginMutation,
      stopPolling,
    ],
  );

  const startDefaultAuth = useCallback(() => startAuth(), [startAuth]);

  const cancelAuth = useCallback(() => {
    const activeDeviceCode = deviceCode?.device_code;
    beginOperation();
    stopPolling();
    setPollingState("idle");
    setDeviceCode(null);
    setError(null);
    if (activeDeviceCode) {
      void cancelRemoteLogin(activeDeviceCode);
    }
  }, [beginOperation, cancelRemoteLogin, deviceCode?.device_code, stopPolling]);

  const submitOauthCallback = useCallback(
    (callbackUrl: string) => {
      const activeDeviceCode = deviceCode?.device_code;
      if (!activeDeviceCode) {
        return Promise.reject(new Error("OAuth session is not active."));
      }
      return submitOauthCallbackMutation.mutateAsync({
        operationGeneration: operationGenerationRef.current,
        deviceCode: activeDeviceCode,
        callbackUrl,
      });
    },
    [deviceCode?.device_code, submitOauthCallbackMutation],
  );

  const logout = useCallback(() => {
    const activeDeviceCode = deviceCode?.device_code;
    beginOperation();
    stopPolling();
    setPollingState("idle");
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
    setPollingState("idle");
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

  const setWorkspace = useCallback(
    (accountId: string, workspaceId: string) => {
      setWorkspaceMutation.mutate({ accountId, workspaceId });
    },
    [setWorkspaceMutation],
  );

  const importCursorLocalAuth = useCallback(() => {
    cancelAuth();
    importCursorLocalMutation.mutate();
  }, [cancelAuth, importCursorLocalMutation]);

  const accounts = authStatus?.accounts ?? [];
  const codexSelection = authStatus?.codex_oauth ?? null;
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
    codexSelection,
    activeCodexAccountId: codexSelection?.activeAccountId ?? null,
    needsCodexAccountSelection: codexSelection?.status === "needs_selection",
    migrationError: authStatus?.migration_error ?? null,
    pollingState,
    deviceCode,
    error: error ?? statusErrorMessage,
    isPolling: pollingState === "polling",
    isAddingAccount: startLoginMutation.isPending || pollingState === "polling",
    isImportingCursorLocalAuth: importCursorLocalMutation.isPending,
    isRemovingAccount: removeAccountMutation.isPending,
    isSettingDefaultAccount: setDefaultAccountMutation.isPending,
    isSettingWorkspace: setWorkspaceMutation.isPending,
    isSubmittingOauthCallback: submitOauthCallbackMutation.isPending,
    startAuth: startDefaultAuth,
    addAccount: startDefaultAuth,
    addAccountWithMode: startAuth,
    cancelAuth,
    logout,
    logoutAsync,
    removeAccount,
    removeAccountAsync: removeAccountMutation.mutateAsync,
    setDefaultAccount,
    selectActiveCodexAccount: setDefaultAccountMutation.mutateAsync,
    setWorkspace,
    submitOauthCallback,
    importCursorLocalAuth,
    refetchStatus,
    invalidateAccountViews: invalidateManagedAccountViews,
  };
}
