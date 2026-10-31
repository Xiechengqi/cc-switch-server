import type { ShareTokenPeriod, ShareUserGrant, ShareUserGrantMap, ShareUserPolicy } from "@/lib/api/share";
import { stageGrantPolicy } from "@/utils/shareFormUtils";

export type ShareUserPolicyBatchPatch = {
  parallelLimit?: { value?: number };
  tokenLimit?: {
    value?: number;
    valueQuota?: ShareUserPolicy["valueQuota"];
    period: ShareTokenPeriod;
    periodAnchorAtMs?: number;
  };
  expiresAt?: { value?: number };
};

/** What a batch asks for one grant, starting from its waiting USD policy when there is one. */
export function patchedSharePolicy(grant: ShareUserGrant, patch: ShareUserPolicyBatchPatch): ShareUserPolicy {
  const policy = { ...(grant.pendingValueQuota?.policy ?? grant.policy) };
  if (patch.parallelLimit) {
    policy.parallelLimit = patch.parallelLimit.value;
  }
  if (patch.tokenLimit) {
    policy.tokenLimit = patch.tokenLimit.value;
    if (patch.tokenLimit.valueQuota) policy.valueQuota = patch.tokenLimit.valueQuota;
    else delete policy.valueQuota;
    policy.tokenPeriod = patch.tokenLimit.period;
    policy.tokenPeriodAnchorAtMs = patch.tokenLimit.periodAnchorAtMs;
  }
  if (patch.expiresAt) {
    policy.expiresAt = patch.expiresAt.value;
  }
  return policy;
}

export function applyShareUserPolicyBatch(
  value: ShareUserGrantMap,
  selectedEmails: ReadonlySet<string>,
  patch: ShareUserPolicyBatchPatch,
): ShareUserGrantMap {
  const normalizedEmails = new Set(
    Array.from(selectedEmails, (email) => email.trim().toLowerCase()).filter(
      Boolean,
    ),
  );
  const updated: ShareUserGrantMap = { ...value };

  for (const [key, grant] of Object.entries(value)) {
    const email = grant.email.trim().toLowerCase();
    // Unreadable USD fields cannot be rebuilt into a policy safely.
    if (!normalizedEmails.has(email) || grant.valueQuotaMalformed) continue;
    updated[key] = stageGrantPolicy(grant, patchedSharePolicy(grant, patch));
  }

  return updated;
}
