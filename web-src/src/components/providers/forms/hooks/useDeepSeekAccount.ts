import { useCallback, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { authApi } from "@/lib/api";
import type { DeepSeekAccountStatus } from "@/lib/api";

type AddAccountInput = {
  email?: string | null;
  mobile?: string | null;
  password: string;
};

export function useDeepSeekAccount() {
  const queryClient = useQueryClient();
  const queryKey = ["deepseek-account-status"];
  const [error, setError] = useState<string | null>(null);

  const {
    data: authStatus,
    isLoading: isLoadingStatus,
    refetch: refetchStatus,
  } = useQuery<DeepSeekAccountStatus>({
    queryKey,
    queryFn: () => authApi.deepseekAccountStatus(),
    staleTime: 30000,
  });

  const addAccountMutation = useMutation({
    mutationFn: (input: AddAccountInput) => authApi.deepseekAccountAdd(input),
    onSuccess: async () => {
      setError(null);
      await refetchStatus();
      await queryClient.invalidateQueries({ queryKey });
    },
    onError: (e) => {
      setError(e instanceof Error ? e.message : String(e));
    },
  });

  const removeAccountMutation = useMutation({
    mutationFn: (accountId: string) => authApi.deepseekAccountRemove(accountId),
    onSuccess: async () => {
      setError(null);
      await refetchStatus();
      await queryClient.invalidateQueries({ queryKey });
    },
    onError: (e) => {
      setError(e instanceof Error ? e.message : String(e));
    },
  });

  const setDefaultAccountMutation = useMutation({
    mutationFn: (accountId: string) =>
      authApi.deepseekAccountSetDefault(accountId),
    onSuccess: async () => {
      setError(null);
      await refetchStatus();
      await queryClient.invalidateQueries({ queryKey });
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
    (accountId: string) => removeAccountMutation.mutate(accountId),
    [removeAccountMutation],
  );

  const setDefaultAccount = useCallback(
    (accountId: string) => setDefaultAccountMutation.mutate(accountId),
    [setDefaultAccountMutation],
  );

  const accounts = authStatus?.accounts ?? [];

  return {
    authStatus,
    isLoadingStatus,
    accounts,
    hasAnyAccount: accounts.length > 0,
    isAuthenticated: authStatus?.authenticated ?? false,
    defaultAccountId: authStatus?.default_account_id ?? null,
    error,
    isAddingAccount: addAccountMutation.isPending,
    isRemovingAccount: removeAccountMutation.isPending,
    isSettingDefaultAccount: setDefaultAccountMutation.isPending,
    addAccount,
    removeAccount,
    setDefaultAccount,
    refetchStatus,
  };
}
