import { describe, expect, it } from "vitest";
import i18n from "@/i18n";
import { HttpResponseError } from "@/lib/runtime";
import {
  bundleCredentialConflict,
  isBundleRefreshConflict,
} from "./bundleErrors";

describe("Bundle conflict diagnostics", () => {
  it("only asks to refresh a real revision or preview conflict", () => {
    for (const code of [
      "cc_switch_provider_revision_conflict",
      "cc_switch_provider_preview_conflict",
    ]) {
      expect(
        isBundleRefreshConflict(new HttpResponseError("stale", 409, { code })),
      ).toBe(true);
    }
    for (const code of [
      undefined,
      "cc_switch_share_credential_source_mismatch",
      "quota_revision_conflict",
      "cc_switch_provider_account_binding_in_use",
    ]) {
      expect(
        isBundleRefreshConflict(
          new HttpResponseError("not a stale Provider", 409, { code }),
        ),
      ).toBe(false);
    }
  });
  it.each([
    ["auth_mismatch", "customBinding"],
    ["endpoint_mismatch", "endpoint"],
    ["credential_mismatch", "surfaceSecret"],
    ["source_unverifiable", "surfaceSecret"],
    ["no_enabled_apps", "surfaces"],
    ["extra_headers_mismatch", "headers"],
  ])("maps %s to the correct field", (reason, field) => {
    const error = new HttpResponseError("raw error", 409, {
      code: "cc_switch_share_credential_source_mismatch",
      details: { reason, app: "codex", firstApp: "claude" },
    });
    expect(bundleCredentialConflict(error, i18n.t)).toMatchObject({
      field,
      surface: "codex",
    });
  });
  it("does not interpret arbitrary server details as a field or translation key", () => {
    expect(
      bundleCredentialConflict(
        new HttpResponseError("unknown", 409, {
          code: "cc_switch_share_credential_source_mismatch",
          details: { reason: "unknown", app: "arbitrary" },
        }),
        i18n.t,
      ),
    ).toBeUndefined();
  });
});
