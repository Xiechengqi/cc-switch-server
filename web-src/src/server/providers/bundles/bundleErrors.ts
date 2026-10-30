import type { TFunction } from "i18next";
import { HttpResponseError } from "@/lib/runtime";
import { isCoreProviderApp } from "@/server/providerRegistry";
import type { BundleValidationIssue } from "./bundleDraft";

const reasons = new Set([
  "auth_mismatch",
  "endpoint_mismatch",
  "credential_mismatch",
  "source_unverifiable",
  "no_enabled_apps",
  "resolution_failed",
  "extra_headers_mismatch",
]);

export function bundleCredentialConflict(
  error: unknown,
  t: TFunction,
): BundleValidationIssue | undefined {
  if (
    !(error instanceof HttpResponseError) ||
    ![
      "cc_switch_share_credential_source_mismatch",
      "cc_switch_share_binding_integrity_failed",
      "cc_switch_share_capacity_pool_derivation_failed",
    ].includes(error.code ?? "")
  )
    return;
  const details = error.details;
  if (!details || typeof details !== "object") return;
  const { reason, app, firstApp } = details as Record<string, unknown>;
  if (typeof reason !== "string" || !reasons.has(reason)) return;
  let message = t(`providerBundle.sharePreflight.${reason}`);
  if (
    typeof app === "string" &&
    isCoreProviderApp(app) &&
    typeof firstApp === "string" &&
    isCoreProviderApp(firstApp)
  ) {
    message += ` ${t("providerBundle.conflictApps", { app, firstApp })}`;
  }
  return {
    code: "shareCredentialConflict",
    field:
      reason === "endpoint_mismatch"
        ? "endpoint"
        : reason === "extra_headers_mismatch"
          ? "headers"
          : reason === "credential_mismatch" || reason === "source_unverifiable"
            ? "surfaceSecret"
            : reason === "no_enabled_apps"
              ? "surfaces"
              : "customBinding",
    message,
    surface:
      typeof app === "string" && isCoreProviderApp(app) ? app : undefined,
  };
}

export function isBundleRefreshConflict(error: unknown): boolean {
  return (
    error instanceof HttpResponseError &&
    error.status === 409 &&
    [
      "cc_switch_provider_revision_conflict",
      "cc_switch_provider_preview_conflict",
    ].includes(error.code ?? "")
  );
}
