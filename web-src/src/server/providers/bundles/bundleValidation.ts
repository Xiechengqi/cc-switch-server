import type { CoreProviderApp } from "@/server/providerRegistry";
import {
  validateProviderBundleDraftIssue,
  type BundleValidationIssue,
} from "./bundleDraft";

export type { BundleValidationIssue };

export function firstBundleValidationIssue(
  draft: Parameters<typeof validateProviderBundleDraftIssue>[0],
): BundleValidationIssue | null {
  return validateProviderBundleDraftIssue(draft);
}

export function bundleValidationFieldId(issue: BundleValidationIssue): string {
  if (issue.surface) {
    return `bundle-field-${issue.field}-${issue.surface}`;
  }
  return `bundle-field-${issue.field}`;
}

export function matchesBundleValidationIssue(
  issue: BundleValidationIssue | null,
  field: BundleValidationIssue["field"],
  surface?: CoreProviderApp,
): boolean {
  return Boolean(
    issue &&
    issue.field === field &&
    (surface === undefined || issue.surface === surface),
  );
}
