import type { ProviderResource } from "@/lib/api/providers";

import {
  customPolicyForProfile,
  driverForProfile,
  profileById,
  providerRegistry,
} from "./providerRegistry";

export type ProviderOperation =
  "forward" | "test" | "discovery" | "connectivity";

export function providerResourceSupportsOperation(
  resource: ProviderResource | undefined,
  operation: ProviderOperation,
): boolean | undefined {
  if (!resource?.profileId) return undefined;
  const profile = profileById(resource.profileId);
  if (!profile) return false;

  const fixedDriver = driverForProfile(profile);
  if (fixedDriver) {
    return fixedDriver.operations[operation] === "supported";
  }

  const binding = resource.customBinding;
  const policy = customPolicyForProfile(profile);
  if (!binding || !policy) return false;
  const driver = providerRegistry.drivers.find(
    (candidate) =>
      policy.allowedDriverIds.includes(candidate.driverId) &&
      candidate.upstreamProtocol === binding.upstreamProtocol &&
      candidate.acceptedAuthSchemes.includes(binding.authScheme),
  );
  return driver?.operations[operation] === "supported";
}
