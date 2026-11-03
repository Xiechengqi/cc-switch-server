import type { ProviderFamilySpec } from "@/server/providerRegistry";
import {
  createProviderBundleDraft,
  type ProviderBundleEditorDraft,
} from "./bundleDraft";

export function createDraftForSelectedFamily(
  family: ProviderFamilySpec,
): ProviderBundleEditorDraft {
  // Keep enabled Surfaces and authentication defaults owned by the Registry.
  return createProviderBundleDraft(family);
}
