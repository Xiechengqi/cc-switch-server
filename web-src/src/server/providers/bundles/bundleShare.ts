import { confirmGrantPreview, grantCapabilityEdits, grantQuotaTerms } from "@/lib/grant-capability-preview";
import {capabilityIntent, capabilityQuotaContext, type ShareCapabilityPolicy, type CapabilityConsent} from "@/lib/share-capabilities";
import {
  shareApi,
  type SaveProviderBundleShareParams,
  type ShareRecord,
  type ShareUserGrant,
  type ShareUserGrantMap,
  type ShareUserPolicy,
  type ShareUserUsageEditMap,
  type ShareUserValueUsageInitializationMap,
} from "@/lib/api/share";
import {
  buildShareUserGrants,
  normalizeShareEmails,
  sharePolicyKey,
} from "@/utils/shareFormUtils";
import {
  getProviderSharePhase,
  PERMANENT_EXPIRES_AT,
  UNLIMITED_PARALLEL_LIMIT,
  UNLIMITED_TOKEN_LIMIT,
} from "@/utils/shareUtils";
import i18n from "@/i18n";
import { HttpResponseError, readWebSessionToken } from "@/lib/runtime";

export interface ProviderBundleShareDraft {
  capabilityPolicy?:ShareCapabilityPolicy;
  capabilityConsent?:CapabilityConsent;
  capabilityConsentScope?:string;
  baseline?: {
    capabilityPolicy?:ShareCapabilityPolicy;
    grantCapabilityPolicy?: boolean;
    shareId: string;
    configRevision: number;
    userGrants: ShareUserGrantMap;
    expiry: ProviderBundleShareExpiry;
    expiresAt: string;
    /** The Share-wide limits as loaded, so an untouched stored value saves as is. */
    tokenLimit: string;
    parallelLimit: string;
  };
  enabled: boolean;
  subdomain: string;
  description: string;
  freeAccess: boolean;
  tokenLimit: string;
  parallelLimit: string;
  expiry: ProviderBundleShareExpiry;
  userGrants: ShareUserGrantMap;
  userUsageEdits: ShareUserUsageEditMap;
  userValueUsageInitializations: ShareUserValueUsageInitializationMap;
  valueUsageOperationId: string;
  valueUsageOperationStartedAtMs: number;
  allowPersonalCredits: boolean;
  autoConsumeBankedReset: boolean;
  bankedResetExpiryLeadMinutes: string;
  previousResponseCacheEnabled: boolean;
  grokMediaPolicy: {
    imageGenerationEnabled: boolean;
    imageEditEnabled: boolean;
    videoGenerationEnabled: boolean;
  };
}

export const BUNDLE_SHARE_EXPIRY_PRESETS = [
  { value: "1h", seconds: 60 * 60, labelKey: "share.expiry.oneHour" },
  { value: "6h", seconds: 6 * 60 * 60, labelKey: "share.expiry.sixHours" },
  { value: "1d", seconds: 24 * 60 * 60, labelKey: "share.expiry.oneDay" },
  {
    value: "7d",
    seconds: 7 * 24 * 60 * 60,
    labelKey: "share.expiry.sevenDays",
  },
  {
    value: "30d",
    seconds: 30 * 24 * 60 * 60,
    labelKey: "share.expiry.thirtyDays",
  },
] as const;

/**
 * `current` keeps the saved expiry exactly; a preset always counts from the
 * moment of saving, so picking one renews the Share.
 */
export type ProviderBundleShareExpiry =
  "permanent" | "current" | (typeof BUNDLE_SHARE_EXPIRY_PRESETS)[number]["value"];

const RESERVED_SHARE_SLUGS = new Set([
  "admin",
  "api",
  "cdn-cgi",
  "router",
  "www",
]);

export function isValidShareSlug(value: string): boolean {
  const slug = value.trim();
  return (
    slug.length >= 6 &&
    slug.length <= 30 &&
    !slug.includes("--") &&
    !RESERVED_SHARE_SLUGS.has(slug) &&
    /^[a-z][a-z0-9-]*[a-z0-9]$/.test(slug)
  );
}

export function shareForBundle(
  shares: ShareRecord[] | undefined,
  bundleId: string,
): ShareRecord | undefined {
  return shares?.find((share) =>
    Object.values(share.bindings).some((providerId) => providerId === bundleId),
  );
}

export function createBundleShareDraft(
  share?: ShareRecord,
): ProviderBundleShareDraft {
  const expiresAt = share?.expiresAt ? Date.parse(share.expiresAt) : Number.NaN;
  // A saved finite expiry is shown as the real date, never rounded to a preset.
  const expiry: ProviderBundleShareExpiry =
    !Number.isFinite(expiresAt) || expiresAt >= Date.parse(PERMANENT_EXPIRES_AT)
      ? "permanent"
      : "current";
  const canonicalUserGrants = share?.userGrants ?? {};
  const shareToEmails = normalizeShareEmails(
    Object.values(canonicalUserGrants)
      .filter((grant) => grant.active !== false && grant.role === "shareto")
      .map((grant) => grant.email),
  );
  const defaultPolicy: ShareUserPolicy = {
    parallelLimit:
      share && share.parallelLimit > 0 ? share.parallelLimit : undefined,
    tokenLimit: share && share.tokenLimit > 0 ? share.tokenLimit : undefined,
    tokenPeriod: "lifetime",
    expiresAt:
      Number.isFinite(expiresAt) && expiresAt < Date.parse(PERMANENT_EXPIRES_AT)
        ? expiresAt
        : undefined,
  };
  const tokenLimit = share && share.tokenLimit >= 0 ? String(share.tokenLimit) : "";
  const parallelLimit = share && share.parallelLimit >= 0 ? String(share.parallelLimit) : "";
  return {
    baseline: share ? {
      capabilityPolicy:share.capabilityPolicy,
      grantCapabilityPolicy:share.grantCapabilityPolicy,
      shareId: share.id,
      configRevision: share.configRevision ?? 0,
      userGrants: { ...canonicalUserGrants, ...buildShareUserGrants({
        source: canonicalUserGrants, ownerEmail: share.ownerEmail, aclEmails: shareToEmails, defaultPolicy,
      }) },
      expiry,
      expiresAt: share.expiresAt,
      tokenLimit,
      parallelLimit,
    } : undefined,
    enabled: Boolean(
      share && share.status !== "paused" && share.status !== "deleted",
    ),
    subdomain: share?.shareSlug ?? "",
    description: share?.description ?? "",
    freeAccess: share?.freeAccess ?? false,
    tokenLimit,
    parallelLimit,
    expiry,
    userGrants: share
      ? buildShareUserGrants({
          source: canonicalUserGrants,
          ownerEmail: share.ownerEmail,
          aclEmails: shareToEmails,
          defaultPolicy,
        })
      : {},
    userUsageEdits: {},
    userValueUsageInitializations: {},
    valueUsageOperationId: crypto.randomUUID(),
    valueUsageOperationStartedAtMs: Date.now(),
    allowPersonalCredits: share?.allowPersonalCredits ?? false,
    autoConsumeBankedReset: share?.autoConsumeBankedReset ?? false,
    bankedResetExpiryLeadMinutes: String(
      share?.bankedResetExpiryLeadMinutes ?? 60,
    ),
    previousResponseCacheEnabled: share
      ? Boolean(share.previousResponseCacheEnabled)
      : true,
    capabilityPolicy: share?.capabilityPolicy,
    grokMediaPolicy: share?.grokMediaPolicy ?? {
      imageGenerationEnabled: false,
      imageEditEnabled: false,
      videoGenerationEnabled: false,
    },
  };
}

/**
 * ShareUserGrantsEditor reports the grant map and the usage-edit map through
 * two callbacks in the same tick.  A consumer that keeps both halves in one
 * draft object must fold each update functionally: spreading a single captured
 * draft in both handlers makes the second call overwrite the first, silently
 * dropping a newly added authorized user.
 */
export function bundleShareGrantHandlers(
  update: (
    apply: (current: ProviderBundleShareDraft) => ProviderBundleShareDraft,
  ) => void,
): {
  onChange: (userGrants: ProviderBundleShareDraft["userGrants"]) => void;
  onUsageEditsChange: (
    userUsageEdits: ProviderBundleShareDraft["userUsageEdits"],
  ) => void;
  onValueUsageInitializationsChange: (value: ShareUserValueUsageInitializationMap) => void;
} {
  return {
    onChange: (userGrants) => update((current) => ({ ...current, userGrants })),
    onUsageEditsChange: (userUsageEdits) =>
      update((current) => ({ ...current, userUsageEdits })),
    onValueUsageInitializationsChange: (userValueUsageInitializations) =>
      update((current) => ({ ...current, userValueUsageInitializations })),
  };
}

/**
 * A Share-wide Token or concurrency limit as typed: empty is unlimited, a
 * whole number from 1 is the limit, and anything else (0 included) is
 * invalid. 0 would stop the Share at its first request, so it is refused
 * rather than read as "unlimited" in one place and "none" in another.
 */
export function shareLimitInput(value: string): number | "unlimited" | "invalid" {
  const trimmed = value.trim();
  if (!trimmed) return "unlimited";
  const parsed = Number(trimmed);
  return Number.isSafeInteger(parsed) && parsed >= 1 ? parsed : "invalid";
}

/**
 * `shareLimitInput` against the value the Share was loaded with. A stored 0,
 * saved before 0 was refused, is "legacy": shown as needing a fix but saved
 * unchanged, so an untouched Share (and the list's quick enable) still saves.
 * Only an edited value has to be valid.
 */
export function shareLimitState(
  value: string,
  saved?: string,
): number | "unlimited" | "invalid" | "legacy" {
  const parsed = shareLimitInput(value);
  if (parsed !== "invalid" || saved === undefined || value.trim() !== saved.trim()) return parsed;
  const stored = Number(saved.trim());
  return Number.isSafeInteger(stored) && stored >= 0 ? "legacy" : parsed;
}

function shareLimitValue(value: string, fallback: number, saved?: string): number {
  const parsed = shareLimitState(value, saved);
  if (parsed === "invalid") {
    throw new Error(i18n.t("provider.share.limitInvalid"));
  }
  if (parsed === "legacy") return Number(value.trim());
  return parsed === "unlimited" ? fallback : parsed;
}

function limitValue(value: string, fallback: number): number {
  if (!value.trim()) return fallback;
  const parsed = Number(value);
  if (!Number.isSafeInteger(parsed) || parsed < 0) {
    throw new Error("Share limits must be non-negative integers");
  }
  return parsed;
}

function expiresAt(draft: ProviderBundleShareDraft): string {
  if (draft.expiry === "current") return draft.baseline?.expiresAt ?? PERMANENT_EXPIRES_AT;
  if (draft.expiry === "permanent") return PERMANENT_EXPIRES_AT;
  const preset = BUNDLE_SHARE_EXPIRY_PRESETS.find(
    (candidate) => candidate.value === draft.expiry,
  );
  if (!preset) throw new Error("Share expiry preset is invalid");
  // Opening-usage retries must retain the full payload, including an edited
  // relative expiry, if a previous response was lost after committing.
  const anchor = Object.keys(draft.userValueUsageInitializations).length ? draft.valueUsageOperationStartedAtMs : Date.now();
  return new Date(anchor + preset.seconds * 1000).toISOString();
}

export type BundleShareSaveAttempt = {
  draft: ProviderBundleShareDraft;
  providerRevision: number;
  session: string | null;
  params: SaveProviderBundleShareParams;
};

export async function saveBundleShare(
  bundleId: string,
  draft: ProviderBundleShareDraft,
  existing?: ShareRecord,
  retry?: {
    providerRevision: number;
    session: string | null;
    attempt: { current: BundleShareSaveAttempt | undefined };
  },
): Promise<ShareRecord | undefined> {
  const assertSession = () => {
    if (retry && retry.session !== readWebSessionToken()) {
      throw new DOMException("Sign-in session changed", "AbortError");
    }
  };
  const send = async (params: SaveProviderBundleShareParams) => {
    assertSession();
    if (retry && params.valueUsageOperationId) {
      retry.attempt.current = { draft, providerRevision: retry.providerRevision, session: retry.session, params };
    }
    try {
      const share = await shareApi.saveProviderBundleShare(params);
      assertSession();
      if (retry) retry.attempt.current = undefined;
      return share;
    } catch (error) {
      if (retry && error instanceof HttpResponseError && error.status < 500 && error.status !== 408) {
        retry.attempt.current = undefined;
      }
      throw error;
    }
  };
  const previous = retry?.attempt.current;
  if (retry && previous?.draft === draft && previous.params.bundleId === bundleId
    && previous.providerRevision === retry.providerRevision && previous.session === retry.session) {
    return send(previous.params);
  }
  if (retry) retry.attempt.current = undefined;

  if ((capabilityIntent(draft.capabilityPolicy)!==capabilityIntent(draft.baseline?.capabilityPolicy) ||
      !!draft.capabilityPolicy?.allowedOperations.length && capabilityQuotaContext(draft.capabilityPolicy,draft.userGrants,Number(draft.tokenLimit))!==capabilityQuotaContext(draft.baseline?.capabilityPolicy,draft.baseline?.userGrants??{},Number(draft.baseline?.tokenLimit??-1))) && (!draft.capabilityConsent || draft.capabilityConsentScope!==capabilityQuotaContext(draft.capabilityPolicy,draft.userGrants,Number(draft.tokenLimit)))) {
    throw new Error(i18n.language.startsWith("zh")?"请先确认扩展能力与额度覆盖预览":"Confirm the capability and quota coverage preview first");
  }
  const subdomain = draft.subdomain.trim();
  if (draft.enabled && subdomain && !isValidShareSlug(subdomain)) {
    throw new Error("Share slug is invalid");
  }
  const tokenLimit = shareLimitValue(draft.tokenLimit, UNLIMITED_TOKEN_LIMIT, draft.baseline?.tokenLimit);
  const parallelLimit = shareLimitValue(
    draft.parallelLimit,
    UNLIMITED_PARALLEL_LIMIT,
    draft.baseline?.parallelLimit,
  );
  const expiry = expiresAt(draft);
  const userGrants = Object.keys(draft.userGrants).length
    ? draft.userGrants
    : undefined;
  const userGrantEdits = draft.baseline
    ? changedBundleUserGrants(draft.baseline.userGrants, draft.userGrants, draft.baseline.grantCapabilityPolicy)
    : undefined;
  const allowedUsageEmails = new Set(
    Object.values(draft.userGrants)
      .filter(
        (grant) =>
          grant.active !== false && grant.manager !== "routerShareMarket",
      )
      .map((grant) => grant.email.trim().toLowerCase()),
  );
  const userUsageEdits = Object.fromEntries(
    Object.entries(draft.userUsageEdits).filter(([email]) =>
      allowedUsageEmails.has(email.trim().toLowerCase()),
    ),
  );
  for (const email of Object.keys(userUsageEdits)) {
    const grant = draft.userGrants[email.trim().toLowerCase()] ?? draft.userGrants[email];
    if (grant?.valueQuotaMalformed) throw new MalformedValueQuotaGrantError(grant.email);
  }
  if (!draft.baseline) {
    const flagged = Object.values(draft.userGrants).find((grant) => grant.active !== false && grant.valueQuotaMalformed);
    if (flagged) throw new MalformedValueQuotaGrantError(flagged.email);
  }
  const bankedResetExpiryLeadMinutes = limitValue(
    draft.bankedResetExpiryLeadMinutes,
    60,
  );
  if (
    bankedResetExpiryLeadMinutes < 10 ||
    bankedResetExpiryLeadMinutes > 7 * 24 * 60
  ) {
    throw new Error(
      "Banked Reset lead time must be between 10 and 10080 minutes",
    );
  }
  if (draft.enabled) {
    assertSession();
    const preflight = await shareApi.preflightProviderBundleShare(bundleId);
    if (!preflight.canShare) {
      throw new HttpResponseError(i18n.t(`providerBundle.sharePreflight.${preflight.reason ?? "resolution_failed"}`, {
        defaultValue: "Unable to verify a common credential source. Check the enabled Apps' endpoints and credentials before sharing.",
      }), 400, { code: "cc_switch_share_credential_source_mismatch", details: preflight.details ?? { reason: preflight.reason } });
    }
  }
  const initializations = Object.fromEntries(Object.entries(draft.userValueUsageInitializations).filter(([email]) =>
    allowedUsageEmails.has(email) && !!draft.userGrants[email]?.policy.valueQuota));
  const conversions = Object.fromEntries(Object.entries(draft.userGrants).flatMap(([email, grant]) => {
    if (!allowedUsageEmails.has(email) || !grant.quotaUnitConversion) return [];
    if (grant.quotaUnitConversionPolicy !== sharePolicyKey(grant.policy)) throw new Error("quota_conversion_preview_stale");
    delete userUsageEdits[email];
    return [[email, grant.quotaUnitConversion]];
  }));
  const baseline = draft.baseline;
  const grantEdits = baseline?.grantCapabilityPolicy ? grantCapabilityEdits(baseline.userGrants, draft.userGrants) : {};
  const changesPermissions = Object.keys(grantEdits).length > 0;
  const changesCoverage = baseline?.grantCapabilityPolicy && Object.entries(baseline.userGrants).some(([email, grant]) =>
    grant.active !== false && grant.manager !== "routerShareMarket" && !!grant.policy.allowedOperations?.length
      && (tokenLimit !== (Number(baseline.tokenLimit) || -1) || !!userGrantEdits?.[email]?.policy));
  let consent = draft.capabilityConsent;
  if (baseline && (changesPermissions || changesCoverage)) {
    assertSession();
    const preview = await shareApi.previewGrantCapabilities(baseline.shareId, {
      expectedConfigRevision: baseline.configRevision,
      grantCapabilityEdits: grantEdits,
      userGrantEdits,
      shareTokenLimit: tokenLimit,
    });
    assertSession();
    consent = confirmGrantPreview(preview);
  }
  return send({
    expectedAccountCycle: Object.values(draft.userGrants).find((grant) => grant.active !== false && grant.policy.tokenPeriod === "accountReset")?.accountQuotaCycle,
    bundleId,
    shareId: draft.baseline?.shareId,
    expectedConfigRevision: draft.baseline?.configRevision,
    enabled: draft.enabled,
    subdomain: subdomain || existing?.shareSlug || "",
    description: draft.description.trim() || undefined,
    freeAccess: draft.freeAccess,
    tokenLimit,
    parallelLimit,
    expiresAt: expiry,
    allowPersonalCredits: draft.allowPersonalCredits,
    autoConsumeBankedReset: draft.autoConsumeBankedReset,
    bankedResetExpiryLeadMinutes,
    previousResponseCacheEnabled: draft.previousResponseCacheEnabled,
    ...(consent ? {capabilityConsent:consent} : {}),
    ...(changesPermissions ? {grantCapabilityEdits:grantEdits} : {}),
    ...(baseline?.grantCapabilityPolicy ? {} : {grokMediaPolicy:draft.grokMediaPolicy}),
    userGrants: draft.baseline ? undefined : userGrants,
    userGrantEdits,
    userUsageEdits,
    ...(Object.keys(initializations).length ? {userValueUsageInitializations: initializations} : {}),
    ...(Object.keys(conversions).length ? {userQuotaConversions: conversions} : {}),
    ...(consent || Object.keys(initializations).length || Object.keys(conversions).length
      ? { valueUsageOperationId: retry ? crypto.randomUUID() : draft.valueUsageOperationId }
      : {}),
  });
}

/**
 * Enforced policy plus the scheduled one. Dropping a schedule (the editor does
 * so when the owner stages a different policy) is a change even when the
 * enforced policy itself is untouched.
 */
function grantPolicyKey(grant: ShareUserGrant, grantCapabilities: boolean) {
  const key = (terms: ShareUserPolicy) => sharePolicyKey(grantCapabilities ? grantQuotaTerms(terms) : terms);
  return JSON.stringify([key(grant.policy), grant.pendingValueQuota?.policy ? key(grant.pendingValueQuota.policy) : null]);
}

export class MalformedValueQuotaGrantError extends Error {
  readonly email: string;
  constructor(email: string) {
    super(i18n.t("share.userLimit.valueQuota.malformedSave", {
      email,
      defaultValue: "{{email}} has a USD quota this page can't read. Reload, or remove the user, before saving.",
    }));
    this.name = "MalformedValueQuotaGrantError";
    this.email = email;
  }
}

/**
 * Only policy/authorization intent goes over the wire, never live counters.
 * A grant whose USD fields could not be read may be revoked, but never
 * re-sent as a policy: that policy would be rebuilt without the unreadable
 * quota or schedule.
 */
export function changedBundleUserGrants(baseline: ShareUserGrantMap, draft: ShareUserGrantMap, grantCapabilities = false) {
  const changes: Record<string, { expectedRevision: number; policy: ShareUserPolicy | null }> = {};
  for (const email of new Set([...Object.keys(baseline), ...Object.keys(draft)])) {
    const previous = baseline[email];
    const next = draft[email];
    if ((previous?.active !== false && previous?.manager === "routerShareMarket") ||
        (next?.active !== false && next?.manager === "routerShareMarket")) continue;
    const wasActive = !!previous && previous.active !== false;
    const active = !!next && next.active !== false;
    if (wasActive === active && (!active || grantPolicyKey(previous, grantCapabilities) === grantPolicyKey(next, grantCapabilities))) continue;
    if (active && (previous?.valueQuotaMalformed || next.valueQuotaMalformed)) {
      throw new MalformedValueQuotaGrantError(next.email || email);
    }
    changes[email] = { expectedRevision: previous?.revision ?? 0, policy: active ? (grantCapabilities ? grantQuotaTerms(next.policy) : next.policy) : null };
  }
  return Object.keys(changes).length ? changes : undefined;
}

export async function enableBundleShare(
  bundleId: string,
  existing?: ShareRecord,
): Promise<ShareRecord | undefined> {
  const draft = createBundleShareDraft(existing);
  draft.enabled = true;
  if (!draft.subdomain.trim()) {
    const suggestion = await shareApi.suggestShareSlug();
    draft.subdomain = suggestion.subdomain;
  }
  return saveBundleShare(bundleId, draft, existing);
}

export async function toggleBundleShare(
  bundleId: string,
  existing?: ShareRecord,
): Promise<"enabled" | "disabled"> {
  if (existing && getProviderSharePhase(existing) === "sharing") {
    await shareApi.disable(existing.id);
    return "disabled";
  }

  await enableBundleShare(bundleId, existing);
  return "enabled";
}

export async function deleteBundleShare(share: ShareRecord): Promise<void> {
  await shareApi.delete(share.id);
}
