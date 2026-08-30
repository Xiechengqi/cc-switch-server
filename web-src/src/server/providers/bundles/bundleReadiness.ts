import { familyById, profileById } from "@/server/providerRegistry";
import type {
  CoreProviderApp,
  ProviderFamilySpec,
} from "@/server/providerRegistry";
import {
  modelPoliciesForSurface,
  type BundleSurfaceEditorDraft,
  type ProviderBundleEditorDraft,
} from "./bundleDraft";

/**
 * What is still missing before a Surface can serve traffic. Deliberately separate from
 * `validateProviderBundleDraftIssue`, which answers a different question — "may this be
 * saved" — by returning the single first blocking issue. A readiness strip has to show
 * every Surface at once, so it needs a per-Surface answer, not a first-wins one.
 */
export type BundleGap = "account" | "endpoint" | "credential" | "model";

/** Most upstream cause first, so a Surface reports the gap the user must fix first. */
const GAP_PRIORITY: BundleGap[] = [
  "account",
  "endpoint",
  "credential",
  "model",
];

export interface SurfaceReadiness {
  app: CoreProviderApp;
  enabled: boolean;
  /** null when the Surface is ready, or when it is disabled and therefore not asked to work. */
  gap: BundleGap | null;
  /**
   * The Surface's own gap, with the shared connection gap stripped out. The two are
   * edited in different cards, so a badge has to know which one it is allowed to claim.
   */
  ownGap: BundleGap | null;
}

export interface BundleReadiness {
  surfaces: SurfaceReadiness[];
  /** Gap on the shared upstream connection: every enabled Surface inherits it. */
  connection: BundleGap | null;
  /** How many enabled Surfaces have a gap of their own, ignoring the shared one. */
  surfaceGaps: number;
  ready: boolean;
}

function firstGap(gaps: BundleGap[]): BundleGap | null {
  for (const candidate of GAP_PRIORITY) {
    if (gaps.includes(candidate)) return candidate;
  }
  return null;
}

function optionalSecretSlot(slot: string): boolean {
  return slot.endsWith("/AWS_SESSION_TOKEN");
}

function connectionGaps(
  draft: ProviderBundleEditorDraft,
  family: ProviderFamilySpec,
): BundleGap[] {
  const credentialProfile = profileById(family.credentialProfileId);
  const gaps: BundleGap[] = [];
  if (
    credentialProfile?.credentialPolicy.mode === "managed_account" &&
    !draft.accountId
  ) {
    gaps.push("account");
  }
  if (
    family.endpointScope === "bundle" &&
    !draft.endpoint.trim() &&
    family.surfaces.some(
      (surface) => profileById(surface.profileId)?.endpointPolicy === "custom",
    )
  ) {
    gaps.push("endpoint");
  }
  const missingSecret = Object.entries(draft.secrets).some(
    ([slot, secret]) =>
      !optionalSecretSlot(slot) && !secret.configured && !secret.value.trim(),
  );
  if (
    missingSecret ||
    (credentialProfile?.formComposition === "aws" && !draft.awsRegion.trim())
  ) {
    gaps.push("credential");
  }
  return gaps;
}

function surfaceGaps(
  draft: ProviderBundleEditorDraft,
  family: ProviderFamilySpec,
  surface: BundleSurfaceEditorDraft,
): BundleGap[] {
  const profile = profileById(surface.profileId);
  if (!profile) return [];
  const gaps: BundleGap[] = [];
  if (
    family.endpointScope === "surface" &&
    profile.endpointPolicy === "custom" &&
    !surface.endpoint.trim()
  ) {
    gaps.push("endpoint");
  }
  if (
    profile.formComposition === "custom" &&
    !surface.secret.configured &&
    !surface.secret.value.trim()
  ) {
    gaps.push("credential");
  }
  // A Surface whose Profile pins the policy keeps its own model even in global scope,
  // which is exactly what the model card renders for it.
  const followsBundle =
    draft.modelPolicyScope === "global" &&
    modelPoliciesForSurface(surface).length > 1;
  const policy = followsBundle ? draft.modelPolicy : surface.modelPolicy;
  const upstreamModel = followsBundle
    ? draft.upstreamModel
    : surface.upstreamModel;
  if (policy === "single" && !upstreamModel.trim()) gaps.push("model");
  return gaps;
}

export function bundleReadiness(
  draft: ProviderBundleEditorDraft,
): BundleReadiness {
  const family = familyById(draft.familyId);
  if (!family) {
    return {
      surfaces: draft.surfaces.map((surface) => ({
        app: surface.app,
        enabled: surface.enabled,
        gap: null,
        ownGap: null,
      })),
      connection: null,
      surfaceGaps: 0,
      ready: false,
    };
  }
  const connection = firstGap(connectionGaps(draft, family));
  let ownGapCount = 0;
  const surfaces = draft.surfaces.map((surface) => {
    if (!surface.enabled) {
      return { app: surface.app, enabled: false, gap: null, ownGap: null };
    }
    const own = firstGap(surfaceGaps(draft, family, surface));
    if (own) ownGapCount += 1;
    return {
      app: surface.app,
      enabled: true,
      gap: firstGap([connection, own].filter((gap): gap is BundleGap => !!gap)),
      ownGap: own,
    };
  });
  return {
    surfaces,
    connection,
    surfaceGaps: ownGapCount,
    ready:
      draft.surfaces.some((surface) => surface.enabled) &&
      !connection &&
      ownGapCount === 0,
  };
}
