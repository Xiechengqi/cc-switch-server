import {
  customRecipesForFamily,
  type ProviderFamilySpec,
} from "@/server/providerRegistry";
import {
  applyCustomRecipeToBundleDraft,
  createProviderBundleDraft,
  type ProviderBundleEditorDraft,
} from "./bundleDraft";

export function createDraftForSelectedFamily(
  family: ProviderFamilySpec,
): ProviderBundleEditorDraft {
  const draft = createProviderBundleDraft(family);
  const recipe = customRecipesForFamily(family)[0];
  return recipe ? applyCustomRecipeToBundleDraft(draft, recipe) : draft;
}
