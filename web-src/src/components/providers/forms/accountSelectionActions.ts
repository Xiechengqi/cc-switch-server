interface RemoveAccountAndUpdateSelectionInput {
  accountId: string;
  selectedAccountId?: string | null;
  removeAccount: (accountId: string) => Promise<unknown>;
  onAccountSelect?: (accountId: string | null) => void;
}

export async function removeAccountAndUpdateSelection({
  accountId,
  selectedAccountId,
  removeAccount,
  onAccountSelect,
}: RemoveAccountAndUpdateSelectionInput): Promise<void> {
  await removeAccount(accountId);
  if (selectedAccountId === accountId) {
    onAccountSelect?.(null);
  }
}

interface LogoutAccountsAndClearSelectionInput {
  logout: () => Promise<unknown>;
  onAccountSelect?: (accountId: string | null) => void;
}

export async function logoutAccountsAndClearSelection({
  logout,
  onAccountSelect,
}: LogoutAccountsAndClearSelectionInput): Promise<void> {
  await logout();
  onAccountSelect?.(null);
}
