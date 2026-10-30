import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ProviderBundleView, ProviderBundleRebindPreview } from "@/lib/api/providers";
import { HttpResponseError } from "@/lib/runtime";
import { familyById } from "@/server/providerRegistry";
import { editProviderBundleDraft } from "./bundleDraft";
import { applyBundleProviderSave, BundleSaveUncertainError, prepareBundleProviderSave, reconcileBundleSaveDraft } from "./bundleSave";

const api = vi.hoisted(() => ({
  upsertBundle: vi.fn(), getBundle: vi.fn(), previewBundleRebind: vi.fn(), applyBundleRebind: vi.fn(),
}));
vi.mock("@/lib/api/providers", () => ({ providersApi: api }));

export function customBundleFixture(): ProviderBundleView {
  const family = familyById("family.custom_http")!;
  return {
    id: "custom-bundle", familyId: family.familyId, name: "Custom", revision: 1,
    modelPolicyScope: "global", testApp: "codex", surfaceTestModels: {}, transport: {},
    supportedApps: ["claude", "codex", "gemini"], enabledApps: ["codex"],
    credentialConfigured: true, credentialSlots: [],
    surfaces: Object.fromEntries(family.surfaces.map(({app, profileId}) => [app, {
      app, profileId, revision: 1, providerType: "custom", providerTypeId: "custom",
      provider: { id: "custom-bundle", name: "Custom", settingsConfig: {
        env: app === "codex" ? { OPENAI_BASE_URL: "https://example.invalid/v1" } : {},
        modelMapping: { mode: "passthrough" },
      } },
      identity: { status: "bound" }, credentialConfigured: app === "codex",
      credentialSlots: app === "codex" ? ["/settingsConfig/apiKey"] : [],
      customBinding: {
        upstreamProtocol: app === "codex" ? "open_ai_responses" : app === "claude" ? "anthropic_messages" : "gemini_native",
        authScheme: app === "gemini" ? "api_key" : "bearer",
      },
    }])),
  };
}

beforeEach(() => { vi.resetAllMocks(); });

describe("Bundle Provider save orchestration", () => {
  it("saves a new Custom Provider through ordinary creation", async () => {
    const bundle = customBundleFixture();
    const draft = editProviderBundleDraft(bundle);
    draft.expectedRevision = undefined;
    draft.clientRequestId = "stable-create-request";
    api.upsertBundle.mockResolvedValue(bundle);
    expect((await prepareBundleProviderSave(draft)).kind).toBe("saved");
    expect(api.upsertBundle).toHaveBeenCalledWith(expect.objectContaining({ clientRequestId: "stable-create-request" }));
    expect(api.previewBundleRebind).not.toHaveBeenCalled();
  });

  it("skips the Provider write when retrying Share after Provider was saved", async () => {
    const bundle = customBundleFixture();
    const draft = editProviderBundleDraft(bundle);
    draft.name = "Renamed";
    const saved = { ...bundle, name: "Renamed", revision: 2 };
    api.upsertBundle.mockResolvedValue(saved);
    expect(await prepareBundleProviderSave(draft, bundle)).toEqual({ kind: "saved", bundle: saved });
    expect(await prepareBundleProviderSave(editProviderBundleDraft(saved), saved)).toEqual({ kind: "saved", bundle: saved });
    expect(api.upsertBundle).toHaveBeenCalledTimes(1);
  });

  it.each(["codex", "claude"] as const)("previews a %s binding change, including a disabled App", async (app) => {
    const bundle = customBundleFixture();
    const draft = editProviderBundleDraft(bundle);
    draft.surfaces.find(surface => surface.app === app)!.customBinding!.authScheme = "api_key";
    expect(bundle.surfaces[app]!.customBinding!.authScheme).toBe("bearer");
    const preview = { previewToken: "opaque-preview-token" } as ProviderBundleRebindPreview;
    api.previewBundleRebind.mockResolvedValue(preview);
    const result = await prepareBundleProviderSave(draft, bundle);
    expect(result.kind).toBe("confirm");
    expect(api.upsertBundle).not.toHaveBeenCalled();
    if (result.kind !== "confirm") throw new Error("preview expected");
    api.applyBundleRebind.mockResolvedValue(bundle);
    await applyBundleProviderSave(draft, result.confirmation);
    expect(api.applyBundleRebind).toHaveBeenCalledWith(result.confirmation.writeDraft, "opaque-preview-token");
  });

  it("does not apply a preview after other draft fields change", async () => {
    const bundle = customBundleFixture();
    const draft = editProviderBundleDraft(bundle);
    draft.surfaces.find(surface => surface.app === "codex")!.customBinding!.upstreamProtocol = "open_ai_chat";
    api.previewBundleRebind.mockResolvedValue({ previewToken: "token" });
    const result = await prepareBundleProviderSave(draft, bundle);
    if (result.kind !== "confirm") throw new Error("preview expected");
    draft.notes = "Changed after preview";
    await expect(applyBundleProviderSave(draft, result.confirmation)).rejects.toMatchObject({ status: 409 });
    expect(api.applyBundleRebind).not.toHaveBeenCalled();
  });

  it("reads back a lost write response without repeating credential writes", async () => {
    const bundle = customBundleFixture();
    const draft = editProviderBundleDraft(bundle);
    draft.name = "Renamed";
    api.upsertBundle.mockRejectedValue(new TypeError("Failed to fetch"));
    api.getBundle.mockResolvedValue({ ...bundle, revision: 2 });
    await expect(prepareBundleProviderSave(draft, bundle)).rejects.toMatchObject({
      name: "BundleSaveUncertainError", latest: expect.objectContaining({ revision: 2 }),
    });
    expect(api.getBundle).toHaveBeenCalledWith(bundle.id);
    expect(api.upsertBundle).toHaveBeenCalledTimes(1);
    expect(draft.name).toBe("Renamed");
  });

  it("preserves uncertainty if read-back fails, but does not read back a definite conflict", async () => {
    const bundle = customBundleFixture();
    const draft = editProviderBundleDraft(bundle);
    draft.name = "Renamed";
    api.upsertBundle.mockRejectedValue(new Error("connection lost"));
    api.getBundle.mockRejectedValue(new Error("offline"));
    await expect(prepareBundleProviderSave(draft, bundle)).rejects.toBeInstanceOf(BundleSaveUncertainError);
    api.getBundle.mockClear();
    api.upsertBundle.mockRejectedValue(new HttpResponseError("stale revision", 409));
    await expect(prepareBundleProviderSave(draft, bundle)).rejects.toMatchObject({ status: 409 });
    expect(api.getBundle).not.toHaveBeenCalled();
  });

  it("reconciles matching read-back without bypassing concurrent or unverifiable credential edits", () => {
    const bundle = customBundleFixture();
    const draft = editProviderBundleDraft(bundle);
    draft.name = "Renamed";
    const latest = { ...bundle, name: "Renamed", revision: 2 };
    const recovered = reconcileBundleSaveDraft(draft, latest)!;
    expect(recovered.expectedRevision).toBe(2);
    expect(reconcileBundleSaveDraft(draft, { ...latest, notes: "Concurrent change" })).toBeUndefined();
    expect(reconcileBundleSaveDraft(draft, { ...latest, revision: 3 })).toBeUndefined();
    expect(reconcileBundleSaveDraft({ ...draft, expectedRevision: undefined, clientRequestId: "create-key" }, latest)).toBeUndefined();
    draft.surfaces[1]!.secret.value = "replacement-kept-in-draft";
    expect(reconcileBundleSaveDraft(draft, latest)).toBeUndefined();
    expect(draft.surfaces[1]!.secret.value).toBe("replacement-kept-in-draft");
    expect(reconcileBundleSaveDraft(draft, { ...latest, revision: 1 })?.surfaces[1]!.secret.value)
      .toBe("replacement-kept-in-draft");
  });
});
