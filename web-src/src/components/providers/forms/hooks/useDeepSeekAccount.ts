import { useCallback, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { authApi } from "@/lib/api";
import type { DeepSeekAccountStatus } from "@/lib/api";

type AddAccountInput = {
  identifier?: string | null;
  accessToken: string;
};

export function useDeepSeekAccount() {
  const queryClient = useQueryClient();
  const queryKey = ["deepseek-account-status"];
  const [error, setError] = useState<string | null>(null);

  const {
    data: authStatus,
    isLoading: isLoadingStatus,
    isError: isStatusError,
    error: statusQueryError,
    refetch: refetchStatus,
  } = useQuery<DeepSeekAccountStatus>({
    queryKey,
    queryFn: () => authApi.deepseekAccountStatus(),
    staleTime: 30000,
  });

  const invalidateDeepSeekAccountViews = useCallback(
    () =>
      Promise.all([
        queryClient.invalidateQueries({ queryKey }),
        queryClient.invalidateQueries({ queryKey: ["managed-auth-accounts"] }),
        queryClient.invalidateQueries({ queryKey: ["providers"] }),
        queryClient.invalidateQueries({ queryKey: ["share"] }),
      ]),
    [queryClient],
  );

  const addAccountMutation = useMutation({
    mutationFn: (input: AddAccountInput) => authApi.deepseekAccountAdd(input),
    onMutate: () => setError(null),
    onSuccess: async () => {
      setError(null);
      await invalidateDeepSeekAccountViews();
    },
    onError: (e) => {
      setError(e instanceof Error ? e.message : String(e));
    },
  });

  const removeAccountMutation = useMutation({
    mutationFn: (accountId: string) => authApi.deepseekAccountRemove(accountId),
    onMutate: () => setError(null),
    onSuccess: async () => {
      setError(null);
      await invalidateDeepSeekAccountViews();
    },
    onError: (e) => {
      setError(e instanceof Error ? e.message : String(e));
    },
  });

  const setDefaultAccountMutation = useMutation({
    mutationFn: (accountId: string) =>
      authApi.deepseekAccountSetDefault(accountId),
    onMutate: () => setError(null),
    onSuccess: async () => {
      setError(null);
      await invalidateDeepSeekAccountViews();
    },
    onError: (e) => {
      setError(e instanceof Error ? e.message : String(e));
    },
  });

  const addAccount = useCallback(
    (input: AddAccountInput) => addAccountMutation.mutateAsync(input),
    [addAccountMutation],
  );

  const removeAccount = useCallback(
    (accountId: string) => removeAccountMutation.mutateAsync(accountId),
    [removeAccountMutation],
  );

  const setDefaultAccount = useCallback(
    (accountId: string) => setDefaultAccountMutation.mutate(accountId),
    [setDefaultAccountMutation],
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
    isStatusError,
    accounts,
    hasAnyAccount: accounts.length > 0,
    isAuthenticated: authStatus?.authenticated ?? false,
    defaultAccountId: authStatus?.default_account_id ?? null,
    error: error ?? statusErrorMessage,
    isAddingAccount: addAccountMutation.isPending,
    isRemovingAccount: removeAccountMutation.isPending,
    isSettingDefaultAccount: setDefaultAccountMutation.isPending,
    addAccount,
    removeAccount,
    setDefaultAccount,
    refetchStatus,
  };
}
