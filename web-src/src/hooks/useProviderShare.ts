import { useMemo } from "react";
import type { AppId, ShareBindings, ShareRecord } from "@/lib/api";
import { useSharesQuery } from "@/lib/query";

const SHAREABLE_APPS = new Set<string>(["claude", "codex", "gemini"]);

export function isShareableApp(
  appId: AppId,
): appId is keyof ShareBindings {
  return SHAREABLE_APPS.has(appId);
}

export function findShareForProvider(
  shares: ShareRecord[],
  appId: keyof ShareBindings,
  providerId: string,
): ShareRecord | null {
  return (
    shares.find((share) => {
      if (share.status === "deleted") return false;
      const boundProviderId = share.bindings?.[appId];
      return typeof boundProviderId === "string" && boundProviderId === providerId;
    }) ?? null
  );
}

export type ProviderShareState = "none" | "active" | "paused" | "error";

export function getProviderShareState(
  share: ShareRecord | null | undefined,
): ProviderShareState {
  if (!share) return "none";
  if (share.status === "active") return "active";
  if (share.status === "paused") return "paused";
  return "error";
}

export function resolveShareOwnerEmail(
  clientOwnerEmail: string | null | undefined,
  shares: ShareRecord[],
): string {
  const fromTunnel = clientOwnerEmail?.trim();
  if (fromTunnel) return fromTunnel;
  const fromShare = shares.find((share) => share.ownerEmail?.trim())?.ownerEmail;
  return fromShare?.trim() ?? "";
}

export function useProviderShare(
  appId: AppId,
  providerId: string | undefined,
) {
  const query = useSharesQuery();
  const share = useMemo(() => {
    if (!providerId || !isShareableApp(appId)) return null;
    return findShareForProvider(query.data ?? [], appId, providerId);
  }, [query.data, appId, providerId]);

  return {
    ...query,
    share,
    state: getProviderShareState(share),
  };
}
