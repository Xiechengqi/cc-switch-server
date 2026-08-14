import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import {
  providersApi,
  type CodingPlanQuotaSnapshot,
  type ProviderResource,
} from "@/lib/api/providers";

const NO_RUNTIME_FINGERPRINT = "runtime-unavailable";

export const codingPlanQuotaKeys = {
  all: ["codingPlanQuota"] as const,
  snapshot: (resource: ProviderResource) =>
    [
      ...codingPlanQuotaKeys.all,
      "snapshot",
      resource.app,
      resource.provider.id,
      resource.revision,
      resource.runtime?.runtimeFingerprint ?? NO_RUNTIME_FINGERPRINT,
    ] as const,
};

export function isCodingPlanQuotaSnapshotForResource(
  snapshot: CodingPlanQuotaSnapshot,
  resource: ProviderResource,
): boolean {
  return (
    snapshot.providerKey.app === resource.app &&
    snapshot.providerKey.providerId === resource.provider.id &&
    snapshot.providerRevision === resource.revision &&
    snapshot.runtimeFingerprint === resource.runtime?.runtimeFingerprint
  );
}

function assertSnapshotScope(
  snapshot: CodingPlanQuotaSnapshot,
  resource: ProviderResource,
): CodingPlanQuotaSnapshot {
  if (!isCodingPlanQuotaSnapshotForResource(snapshot, resource)) {
    throw new Error("Provider changed while reading its coding-plan quota");
  }
  return snapshot;
}

export function useCodingPlanQuota(resource: ProviderResource, enabled = true) {
  const contract = resource.runtime?.codingPlan;
  return useQuery<CodingPlanQuotaSnapshot>({
    queryKey: codingPlanQuotaKeys.snapshot(resource),
    queryFn: async () =>
      assertSnapshotScope(
        await providersApi.getCodingPlanQuota(
          resource.app,
          resource.provider.id,
        ),
        resource,
      ),
    enabled: enabled && Boolean(contract),
    staleTime: contract?.quota.cacheTtlMs ?? 60_000,
    refetchOnWindowFocus: false,
    retry: false,
  });
}

export function useRefreshCodingPlanQuota(resource: ProviderResource) {
  const queryClient = useQueryClient();
  const queryKey = codingPlanQuotaKeys.snapshot(resource);

  return useMutation<CodingPlanQuotaSnapshot>({
    mutationKey: [...queryKey, "force-refresh"],
    mutationFn: async () =>
      assertSnapshotScope(
        await providersApi.refreshCodingPlanQuota(
          resource.app,
          resource.provider.id,
        ),
        resource,
      ),
    onSuccess: (snapshot) => {
      queryClient.setQueryData(queryKey, snapshot);
    },
  });
}
