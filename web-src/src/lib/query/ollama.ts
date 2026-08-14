import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import {
  providersApi,
  type OllamaCloudSnapshot,
  type ProviderResource,
} from "@/lib/api/providers";

export const ollamaCloudKeys = {
  all: ["ollamaCloud", "accountUsage"] as const,
  snapshot: (resource: ProviderResource) =>
    [
      ...ollamaCloudKeys.all,
      resource.app,
      resource.provider.id,
      resource.revision,
    ] as const,
};

export function isOllamaCloudSnapshotForResource(
  snapshot: OllamaCloudSnapshot,
  resource: ProviderResource,
): boolean {
  return (
    snapshot.providerKey.app === resource.app &&
    snapshot.providerKey.providerId === resource.provider.id &&
    snapshot.providerRevision === resource.revision
  );
}

function assertSnapshotScope(
  snapshot: OllamaCloudSnapshot,
  resource: ProviderResource,
): OllamaCloudSnapshot {
  if (!isOllamaCloudSnapshotForResource(snapshot, resource)) {
    throw new Error("Provider changed while reading its Ollama account usage");
  }
  return snapshot;
}

export function useOllamaQuota(resource: ProviderResource, enabled = true) {
  return useQuery<OllamaCloudSnapshot>({
    queryKey: ollamaCloudKeys.snapshot(resource),
    queryFn: async () =>
      assertSnapshotScope(
        await providersApi.getProviderAccountUsage(
          resource.app,
          resource.provider.id,
        ),
        resource,
      ),
    enabled,
    staleTime: 5 * 60_000,
    gcTime: 0,
    refetchOnWindowFocus: false,
    retry: false,
  });
}

export function useRefreshOllamaQuota(resource: ProviderResource) {
  const queryClient = useQueryClient();
  const queryKey = ollamaCloudKeys.snapshot(resource);

  return useMutation<OllamaCloudSnapshot>({
    mutationKey: [...queryKey, "force-refresh"],
    gcTime: 0,
    mutationFn: async () =>
      assertSnapshotScope(
        await providersApi.refreshProviderAccountUsage(
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
