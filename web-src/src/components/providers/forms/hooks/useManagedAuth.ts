import { useState, useCallback, useRef, useEffect } from "react";
import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { authApi } from "@/lib/api";
import type {
  ManagedAuthProvider,
  ManagedAuthStatus,
  ManagedAuthDeviceCodeResponse,
} from "@/lib/api";

type PollingState = "idle" | "polling" | "success" | "error";

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

  const {
    data: authStatus,
    isLoading: isLoadingStatus,
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
        queryClient.invalidateQueries({ queryKey: ["subscription"] }),
        queryClient.invalidateQueries({ queryKey: [authProvider, "quota"] }),
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

  useEffect(() => {
    return () => {
      stopPolling();
    };
  }, [stopPolling]);

  const startLoginMutation = useMutation({
    mutationFn: (params?: {
      oauthFlowMode?: "web_paste" | "localhost" | "cli" | "device";
      codexCallbackUrl?: string | null;
      kiroLoginProvider?: "google" | "github" | null;
    }) =>
      authApi.authStartLogin(
        authProvider,
        githubDomain,
        params?.oauthFlowMode,
        params?.codexCallbackUrl,
        params?.kiroLoginProvider,
      ),
    onSuccess: async (response) => {
      setDeviceCode(response);
      setPollingState("polling");
      setError(null);

      // Add a small buffer on top of GitHub's suggested interval to avoid
      // hitting slow_down responses too aggressively during device polling.
      const interval = Math.max((response.interval || 5) + 3, 8) * 1000;
      const expiresAt = Date.now() + response.expires_in * 1000;

      const pollOnce = async () => {
        if (Date.now() > expiresAt) {
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
          );
          if (newAccount) {
            stopPolling();
            setPollingState("success");
            await refetchStatus();
            await queryClient.invalidateQueries({ queryKey });
            setPollingState("idle");
            setDeviceCode(null);
          }
        } catch (e) {
          const errorMessage = e instanceof Error ? e.message : String(e);
          if (
            !errorMessage.includes("pending") &&
            !errorMessage.includes("slow_down")
          ) {
            stopPolling();
            setPollingState("error");
            setError(errorMessage);
          }
        }
      };

      void pollOnce();
      pollingIntervalRef.current = setInterval(pollOnce, interval);
      pollingTimeoutRef.current = setTimeout(() => {
        stopPolling();
        setPollingState("error");
        setError("Device code expired. Please try again.");
      }, response.expires_in * 1000);
    },
    onError: (e) => {
      setPollingState("error");
      setError(e instanceof Error ? e.message : String(e));
    },
  });

  const logoutMutation = useMutation({
    mutationFn: () => authApi.authLogout(authProvider),
    onSuccess: async () => {
      setPollingState("idle");
      setDeviceCode(null);
      setError(null);
      queryClient.setQueryData(queryKey, {
        provider: authProvider,
        authenticated: false,
        default_account_id: null,
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
    onSuccess: async () => {
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
    onSuccess: async () => {
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
    onSuccess: async () => {
      setPollingState("idle");
      setDeviceCode(null);
      setError(null);
      await refetchStatus();
      await queryClient.invalidateQueries({ queryKey });
    },
    onError: (e) => {
      console.error("[ManagedAuth] Failed to import local Cursor auth:", e);
      setError(e instanceof Error ? e.message : String(e));
    },
  });

  const startAuth = useCallback(
    (
      oauthFlowMode?: "web_paste" | "localhost" | "cli" | "device",
      options?: {
        codexCallbackUrl?: string | null;
        kiroLoginProvider?: "google" | "github" | null;
      },
    ) => {
      setPollingState("idle");
      setDeviceCode(null);
      setError(null);
      stopPolling();
      startLoginMutation.mutate(
        oauthFlowMode || options?.codexCallbackUrl || options?.kiroLoginProvider
          ? {
              oauthFlowMode,
              codexCallbackUrl: options?.codexCallbackUrl,
              kiroLoginProvider: options?.kiroLoginProvider,
            }
          : undefined,
      );
    },
    [startLoginMutation, stopPolling],
  );

  const startDefaultAuth = useCallback(() => {
    setPollingState("idle");
    setDeviceCode(null);
    setError(null);
    stopPolling();
    startLoginMutation.mutate(undefined);
  }, [startLoginMutation, stopPolling]);

  const cancelAuth = useCallback(() => {
    const activeDeviceCode = deviceCode?.device_code;
    stopPolling();
    setPollingState("idle");
    setDeviceCode(null);
    setError(null);
    if (activeDeviceCode) {
      void authApi
        .authCancelLogin(authProvider, activeDeviceCode)
        .catch((e) => {
          console.warn(
            "[ManagedAuth] Failed to cancel remote auth session:",
            e,
          );
        });
    }
  }, [authProvider, deviceCode?.device_code, stopPolling]);

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

  const setWorkspace = useCallback(
    (accountId: string, workspaceId: string) => {
      setWorkspaceMutation.mutate({ accountId, workspaceId });
    },
    [setWorkspaceMutation],
  );

  const importCursorLocalAuth = useCallback(() => {
    setPollingState("idle");
    setDeviceCode(null);
    setError(null);
    stopPolling();
    importCursorLocalMutation.mutate();
  }, [importCursorLocalMutation, stopPolling]);

  const accounts = authStatus?.accounts ?? [];

  return {
    authStatus,
    isLoadingStatus,
    accounts,
    hasAnyAccount: accounts.length > 0,
    isAuthenticated: authStatus?.authenticated ?? false,
    defaultAccountId: authStatus?.default_account_id ?? null,
    migrationError: authStatus?.migration_error ?? null,
    pollingState,
    deviceCode,
    error,
    isPolling: pollingState === "polling",
    isAddingAccount: startLoginMutation.isPending || pollingState === "polling",
    isImportingCursorLocalAuth: importCursorLocalMutation.isPending,
    isRemovingAccount: removeAccountMutation.isPending,
    isSettingDefaultAccount: setDefaultAccountMutation.isPending,
    isSettingWorkspace: setWorkspaceMutation.isPending,
    startAuth: startDefaultAuth,
    addAccount: startDefaultAuth,
    addAccountWithMode: startAuth,
    cancelAuth,
    logout,
    removeAccount,
    setDefaultAccount,
    setWorkspace,
    importCursorLocalAuth,
    refetchStatus,
  };
}
