import { beforeEach, describe, expect, it, vi } from "vitest";
import { act } from "react";
import { createRoot } from "react-dom/client";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ProviderBundleView } from "@/lib/api/providers";
import type { ManagedAuthAccount } from "@/lib/api/auth";
import { familyById } from "@/server/providerRegistry";
import i18n from "@/i18n";
import { HttpResponseError } from "@/lib/runtime";
import { ProviderBundleEditor } from "./ProviderBundleEditor";

const mocks = vi.hoisted(() => ({
  upsertBundle: vi.fn(), getBundle: vi.fn(), shareSave: vi.fn(), toastError: vi.fn(), onSaved: vi.fn(),
  validateBundleDraft: vi.fn(),
  accounts: [] as ManagedAuthAccount[],
}));
vi.mock("sonner", () => ({ toast: { error: mocks.toastError, success: vi.fn() } }));
vi.mock("@/lib/api/providers", () => ({ providersApi: {
  upsertBundle: mocks.upsertBundle,
  getBundle: mocks.getBundle,
  validateBundleDraft: mocks.validateBundleDraft,
  getRequestDefaults: async () => ({ requestTimeoutSeconds: 300, streamFirstByteTimeoutSeconds: 120, streamIdleTimeoutSeconds: 300 }),
  getHealthCheckConfig: async () => ({ timeoutSeconds: 30, maxRetries: 0, degradedThresholdSeconds: 10, testModels: { claude: "test", codex: "test", gemini: "test" } }),
} }));
vi.mock("@/lib/api/share", () => ({ shareApi: { saveProviderBundleShare: mocks.shareSave } }));
vi.mock("@/lib/query", async (original) => ({
  ...await original<typeof import("@/lib/query")>(),
  useManagedAccountsQuery: () => ({ data: mocks.accounts }),
  useSharesQuery: () => ({ data: [] }),
  useClientTunnelQuery: () => ({ data: { config: { ownerEmail: "owner@example.com" } } }),
}));
vi.mock("@/components/providers/ShareUserGrantsEditor", () => ({ ShareUserGrantsEditor: () => null }));

Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });

function fixture(): ProviderBundleView {
  const family = familyById("family.custom_http")!;
  return {
    id: "custom-editor", familyId: family.familyId, name: "Custom", revision: 1,
    modelPolicyScope: "global", testApp: "codex", surfaceTestModels: {}, transport: {},
    supportedApps: ["claude", "codex", "gemini"], enabledApps: ["codex"], credentialConfigured: true, credentialSlots: [],
    surfaces: Object.fromEntries(family.surfaces.map(({ app, profileId }) => [app, {
      app, profileId, revision: 1, providerType: "custom", providerTypeId: "custom",
      provider: { id: "custom-editor", name: "Custom", settingsConfig: {
        env: app === "codex" ? { OPENAI_BASE_URL: "https://example.invalid/v1" } : {},
        modelMapping: { mode: "passthrough" },
      } },
      customBinding: {
        upstreamProtocol: app === "codex" ? "open_ai_responses" : app === "claude" ? "anthropic_messages" : "gemini_native",
        authScheme: app === "gemini" ? "api_key" : "bearer",
      },
      identity: { status: "bound" }, credentialConfigured: app === "codex",
      credentialSlots: app === "codex" ? ["/settingsConfig/apiKey"] : [],
    }])),
  };
}

function saveButton(): HTMLButtonElement {
  const button = Array.from(document.querySelectorAll<HTMLButtonElement>("button"))
    .find(button => button.textContent === i18n.t("common.save"));
  if (!button) throw new Error("save button missing");
  return button;
}

beforeEach(() => {
  vi.resetAllMocks();
  mocks.accounts = [];
  mocks.validateBundleDraft.mockResolvedValue({ requiresRebind: false });
});

describe("Provider Bundle editor save stages", () => {
  it("shows Buddy account labels and follows the upstream probe model instead of global defaults", async () => {
    for (const product of ["workbuddy_oauth", "codebuddy_oauth"] as const) {
      const family = familyById(`family.${product}`)!;
      const view = fixture();
      view.familyId = family.familyId;
      view.enabledApps = ["claude", "codex", "gemini"];
      view.surfaces = Object.fromEntries(family.surfaces.map(({ app, profileId }) => [app, {
        ...view.surfaces[app]!, profileId, providerType: product, providerTypeId: product,
        provider: { id: view.id, name: "Buddy", settingsConfig: { modelMapping: { mode: "single", upstreamModel: "auto" } },
          meta: { providerType: product, authBinding: { source: "managed_account", authProvider: product, accountId: "buddy-local-id", authIdentityGeneration: 1 } } },
      }]));
      mocks.accounts = [{ id: "buddy-local-id", provider: product, authIdentityGeneration: 1,
        login: "buddy-local-id", email: null, displayName: "旧账号昵称", avatar_url: null,
        authenticated_at: 1, is_default: true, github_domain: "",
        subscriptionExpiry: { capability: "automatic", rule:null, ruleNextExpiresAt:null, automaticExpiresAt:null,
          legacyManualExpiresAt:null, manualExpiresAt:null, effectiveExpiresAt:null, source:null, kind:null },
      }];
      const container = document.createElement("div");
      document.body.appendChild(container);
      const root = createRoot(container);
      const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
      client.setQueryData(["managed-auth-status", product], { accounts: mocks.accounts, authenticated:true, default_account_id:"buddy-local-id" });
      client.setQueryData(["account-capabilities"], [{ providerType: product,
        inferenceBindingSupported:true, credentialOwnership:"managed_account", deprecatedForInference:false,
        supportsStartLogin:true, supportsImport:true,
      }]);
      try {
        await act(async () => root.render(<QueryClientProvider client={client}>
          <ProviderBundleEditor bundle={view} onSaved={mocks.onSaved} onCancel={() => {}} />
        </QueryClientProvider>));
        expect(container.textContent).toContain("旧账号昵称");
        await act(async () => container.querySelector<HTMLButtonElement>("#bundle-card-diagnostics")!.click());
        const model = container.querySelector<HTMLInputElement>("#provider-bundle-test-model")!;
        expect(model.value).toBe("auto");
        expect(model.disabled).toBe(true);
        expect(container.textContent).toContain(i18n.t("providerBundle.testModelFollowsUpstreamHint", { model:"auto" }));
        expect(container.textContent).not.toContain(i18n.t("providerBundle.testModelDefaultHint", { model:"test" }));
      } finally {
        await act(async () => root.unmount()); client.clear(); container.remove();
      }
    }
  });

  it("hides authentication controls, keeps API Key visible and preserves stored Bearer", async () => {
    const container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    try {
      await act(async () => root.render(<QueryClientProvider client={client}>
        <ProviderBundleEditor bundle={fixture()} onSaved={mocks.onSaved} onCancel={() => {}} />
      </QueryClientProvider>));
      expect(container.textContent).not.toContain(i18n.t("serverProviderForm.binding.authScheme"));
      expect(container.textContent).toContain(i18n.t("providerBundle.apiKey"));
      const advanced = Array.from(container.querySelectorAll<HTMLButtonElement>("button"))
        .find(button => button.textContent === i18n.t("providerBundle.authAdvanced"))!;
      expect(advanced).toBeDefined();
      await act(async () => advanced.click());
      expect(container.textContent).toContain(i18n.t("serverProviderForm.binding.authScheme"));
      expect(container.textContent).toContain("Authorization: Bearer");
      expect(mocks.upsertBundle).not.toHaveBeenCalled();
    } finally {
      await act(async () => root.unmount()); client.clear(); container.remove();
    }
  });

  it("stops before any writes on a typed draft conflict and does not ask for refresh", async () => {
    mocks.validateBundleDraft.mockRejectedValue(new HttpResponseError("carrier mismatch", 409, {
      code: "cc_switch_share_credential_source_mismatch",
      details: { reason: "auth_mismatch", app: "codex", firstApp: "claude" },
    }));
    const container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    try {
      await act(async () => root.render(<QueryClientProvider client={client}>
        <ProviderBundleEditor bundle={fixture()} onSaved={mocks.onSaved} onCancel={() => {}} />
      </QueryClientProvider>));
      await act(async () => {
        const input = container.querySelector<HTMLInputElement>("#bundle-field-name")!;
        Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(input, "Renamed");
        input.dispatchEvent(new Event("input", { bubbles: true }));
      });
      await act(async () => saveButton().click());
      expect(mocks.validateBundleDraft).toHaveBeenCalledTimes(1);
      expect(mocks.upsertBundle).not.toHaveBeenCalled();
      expect(mocks.shareSave).not.toHaveBeenCalled();
      expect(mocks.toastError).toHaveBeenCalledWith(expect.stringContaining(i18n.t("providerBundle.sharePreflight.auth_mismatch")));
      expect(container.textContent).toContain(i18n.t("serverProviderForm.binding.authScheme"));
      expect(container.querySelector("#bundle-field-customBinding-codex")).not.toBeNull();
    } finally {
      await act(async () => root.unmount()); client.clear(); container.remove();
    }
  });
  it("keeps Share retry enabled after Provider saves, without writing Provider twice", async () => {
    const bundle = fixture();
    const saved = { ...bundle, name: "Renamed", revision: 2 };
    mocks.upsertBundle.mockResolvedValue(saved);
    mocks.shareSave.mockRejectedValueOnce(new Error("Share temporarily unavailable"))
      .mockResolvedValueOnce(undefined);
    const container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    try {
      await act(async () => root.render(
        <QueryClientProvider client={client}>
          <ProviderBundleEditor bundle={bundle} onSaved={mocks.onSaved} onCancel={() => {}} />
        </QueryClientProvider>,
      ));
      await act(async () => {
        const input = document.querySelector<HTMLInputElement>("#bundle-field-name")!;
        Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(input, "Renamed");
        input.dispatchEvent(new Event("input", { bubbles: true }));
      });
      await act(async () => saveButton().click());
      expect(mocks.upsertBundle).toHaveBeenCalledTimes(1);
      expect(mocks.shareSave).toHaveBeenCalledTimes(1);
      expect(mocks.toastError).toHaveBeenCalledWith(i18n.t("providerBundle.shareSaveFailed", { error: "Share temporarily unavailable" }));
      expect(mocks.onSaved).not.toHaveBeenCalled();
      expect(saveButton().disabled).toBe(false);
      await act(async () => saveButton().click());
      expect(mocks.upsertBundle).toHaveBeenCalledTimes(1);
      expect(mocks.shareSave).toHaveBeenCalledTimes(2);
      expect(mocks.onSaved).toHaveBeenCalledWith(saved);
    } finally {
      await act(async () => root.unmount());
      client.clear();
      container.remove();
    }
  });

  it("keeps Share completion retryable after a lost Provider response is safely reconciled", async () => {
    const bundle = fixture();
    const saved = { ...bundle, name: "Renamed", revision: 2 };
    mocks.upsertBundle.mockRejectedValue(new TypeError("Failed to fetch"));
    mocks.getBundle.mockResolvedValue(saved);
    mocks.shareSave.mockResolvedValue(undefined);
    const container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    try {
      await act(async () => root.render(
        <QueryClientProvider client={client}>
          <ProviderBundleEditor bundle={bundle} onSaved={mocks.onSaved} onCancel={() => {}} />
        </QueryClientProvider>,
      ));
      await act(async () => {
        const input = document.querySelector<HTMLInputElement>("#bundle-field-name")!;
        Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(input, "Renamed");
        input.dispatchEvent(new Event("input", { bubbles: true }));
      });
      await act(async () => saveButton().click());
      expect(mocks.upsertBundle).toHaveBeenCalledTimes(1);
      expect(mocks.getBundle).toHaveBeenCalledWith(bundle.id);
      expect(mocks.shareSave).not.toHaveBeenCalled();
      expect(mocks.onSaved).not.toHaveBeenCalled();
      expect(saveButton().disabled).toBe(false);
      await act(async () => saveButton().click());
      expect(mocks.upsertBundle).toHaveBeenCalledTimes(1);
      expect(mocks.shareSave).toHaveBeenCalledTimes(1);
      expect(mocks.onSaved).toHaveBeenCalledWith(saved);
    } finally {
      await act(async () => root.unmount());
      client.clear();
      container.remove();
    }
  });
});
