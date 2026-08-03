const DEFAULT_ACCOUNT_ID = "default";

export interface OauthQuotaUpdatedIdentity {
  authProvider: string;
  accountId?: string | null;
  authIdentityGeneration?: number | null;
  providerId?: string | null;
  appType?: string | null;
}

export function resolveOauthQuotaQueryRoot(authProvider: string): string {
  if (authProvider === "github_copilot") return "copilot";
  if (authProvider === "ollama_cloud") return "ollama";
  return authProvider;
}

export function oauthQuotaRootKey(authProvider: string) {
  return [resolveOauthQuotaQueryRoot(authProvider), "quota"] as const;
}

export function oauthQuotaAccountKey(
  authProvider: string,
  accountId?: string | null,
  authIdentityGeneration?: number | null,
) {
  const key: Array<string | number> = [
    ...oauthQuotaRootKey(authProvider),
    accountId ?? DEFAULT_ACCOUNT_ID,
  ];
  if (authIdentityGeneration != null) {
    key.push(authIdentityGeneration);
  }
  return key;
}

export function oauthQuotaProviderKey(
  authProvider: string,
  providerId: string,
  appType: string,
) {
  return [...oauthQuotaRootKey(authProvider), providerId, appType] as const;
}

export function isStaleOauthQuotaAccountKey(
  queryKey: readonly unknown[],
  authProvider: string,
  accountId: string,
  currentAuthIdentityGeneration: number,
): boolean {
  const prefix = oauthQuotaAccountKey(authProvider, accountId);
  if (!prefix.every((value, index) => queryKey[index] === value)) {
    return false;
  }
  const cachedGeneration = queryKey[prefix.length];
  return (
    typeof cachedGeneration === "number" &&
    cachedGeneration !== currentAuthIdentityGeneration
  );
}

export function oauthQuotaInvalidationKeys({
  authProvider,
  accountId,
  authIdentityGeneration,
  providerId,
  appType,
}: OauthQuotaUpdatedIdentity) {
  const normalizedAccountId = accountId ?? DEFAULT_ACCOUNT_ID;

  if (authProvider === "ollama_cloud" || authProvider === "ollama") {
    return [oauthQuotaRootKey(authProvider)] as const;
  }

  const keys: Array<readonly (string | number)[]> = [
    oauthQuotaAccountKey(
      authProvider,
      normalizedAccountId,
      authIdentityGeneration,
    ),
  ];

  if (authProvider === "cursor_apikey" && providerId && appType) {
    keys.push(oauthQuotaProviderKey(authProvider, providerId, appType));
  }

  if (normalizedAccountId !== DEFAULT_ACCOUNT_ID) {
    keys.push(oauthQuotaAccountKey(authProvider, null));
  }

  return keys;
}
