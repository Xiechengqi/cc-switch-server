import { beforeEach, describe, expect, it, vi } from "vitest";
import { act } from "react";
import { createRoot } from "react-dom/client";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { CodeBuddySite, ManagedAuthAccount, WorkBuddyCheckinView } from "@/lib/api/auth";
import type { BuddyProviderType, ProviderBundleView } from "@/lib/api/providers";
import { familyById } from "@/server/providerRegistry";
import { HttpResponseError } from "@/lib/runtime";
import i18n from "@/i18n";
import { ProviderBundleEditor } from "./ProviderBundleEditor";
import { duplicateProviderBundleDraft, toProviderBundleWriteDraft } from "./bundleDraft";
import { ManagedAccountSection } from "@/components/providers/forms/ManagedAccountSection";

const mocks = vi.hoisted(() => ({
  upsert: vi.fn(), get: vi.fn(), saved: vi.fn(), error: vi.fn(), start: vi.fn(),
  cancel: vi.fn(), importCredentials: vi.fn(),
  getCheckin: vi.fn(), setCheckinPolicy: vi.fn(), runCheckin: vi.fn(),
  accounts: [] as ManagedAuthAccount[],
}));
vi.mock("sonner", () => ({ toast: { error: mocks.error, success: vi.fn() } }));
vi.mock("@/lib/api/providers", () => ({ providersApi: {
  upsertBundle: mocks.upsert, getBundle: mocks.get,
  fetchBuddyAccountModels: async (identity: unknown) => ({
    ...(identity as object), models: [], stale: false, fetchedAtMs: 1,
  }),
  getRequestDefaults: async () => ({ requestTimeoutSeconds: 300, streamFirstByteTimeoutSeconds: 120, streamIdleTimeoutSeconds: 300 }),
  getHealthCheckConfig: async () => ({ timeoutSeconds: 30, maxRetries: 0, degradedThresholdSeconds: 10, testModels: {} }),
} }));
vi.mock("@/lib/api/share", () => ({ shareApi: { saveProviderBundleShare: async () => undefined } }));
vi.mock("@/lib/api", async (original) => ({ ...await original<object>(), authApi: {
  authGetStatus: async (provider: string) => ({ provider, accounts: mocks.accounts, authenticated: true, default_account_id: null }),
  authStartLogin: mocks.start, authCancelLogin: mocks.cancel, authPollForAccount: async () => null,
} }));
vi.mock("@/lib/api/auth", async (original) => ({
  ...await original<object>(), authImportBuddyCredentials: mocks.importCredentials,
  authGetCheckin: mocks.getCheckin, authSetCheckinPolicy: mocks.setCheckinPolicy, authCheckin: mocks.runCheckin,
}));
vi.mock("@/lib/query/accounts", async (original) => ({
  ...await original<object>(), useManagedAccountsQuery: () => ({ data: mocks.accounts }),
}));
vi.mock("@/lib/query", async (original) => ({
  ...await original<object>(),
  useManagedAccountsQuery: () => ({ data: mocks.accounts }),
  useSharesQuery: () => ({ data: [] }),
  useClientTunnelQuery: () => ({ data: { config: { ownerEmail: "owner@example.com" } } }),
}));
vi.mock("@/components/providers/ShareUserGrantsEditor", () => ({ ShareUserGrantsEditor: () => null }));

// Exercise the editor's controlled value and save orchestration independently
// of Radix's pointer/focus implementation (human viewport checks stay separate).
vi.mock("@/components/ui/select", async () => {
  const React = await import("react");
  const SelectTrigger = () => null;
  return {
    Select: ({ children, value, onValueChange, disabled }: {
      children: React.ReactNode; value?: string; disabled?: boolean; onValueChange?: (value: string) => void;
    }) => {
      const trigger = React.Children.toArray(children).find((child) => React.isValidElement(child) && child.type === SelectTrigger);
      const props = React.isValidElement<Record<string, unknown>>(trigger) ? trigger.props : {};
      return <select {...props} value={value ?? ""} disabled={disabled}
        onChange={(event) => onValueChange?.(event.target.value)}>{children}</select>;
    },
    SelectTrigger, SelectValue: () => null,
    SelectContent: ({ children }: { children: React.ReactNode }) => <>{children}</>,
    SelectGroup: ({ children }: { children: React.ReactNode }) => <>{children}</>,
    SelectLabel: () => null, SelectSeparator: () => null,
    SelectItem: ({ value, children }: { value: string; children: React.ReactNode }) => <option value={value}>{children}</option>,
  };
});

Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });

function account(product: BuddyProviderType, site: CodeBuddySite): ManagedAuthAccount {
  return {
    id: "buddy-account", provider: product, authIdentityGeneration: 1,
    login: "fixture", displayName: "Fixture Account", avatar_url: null,
    authenticated_at: 1, is_default: false, github_domain: "", buddy: { site },
    subscriptionExpiry: { capability: "automatic", rule: null, ruleNextExpiresAt: null,
      automaticExpiresAt: null, legacyManualExpiresAt: null, manualExpiresAt: null,
      effectiveExpiresAt: null, source: null, kind: null },
  };
}

function fixture(product: BuddyProviderType, buddyLoginSite?: CodeBuddySite): ProviderBundleView {
  const family = familyById(`family.${product}`)!;
  return {
    id: "buddy-provider", familyId: family.familyId, name: "Buddy", revision: 1,
    buddyLoginSite, modelPolicyScope: "global", testApp: "codex", surfaceTestModels: {}, transport: {},
    supportedApps: ["claude", "codex", "gemini"], enabledApps: ["claude", "codex", "gemini"],
    credentialConfigured: true, credentialSlots: [],
    surfaces: Object.fromEntries(family.surfaces.map(({ app, profileId }) => [app, {
      app, profileId, revision: 1, providerType: product, providerTypeId: product,
      provider: { id: "buddy-provider", name: "Buddy", settingsConfig: { modelMapping: { mode: "single", upstreamModel: "auto" } },
        meta: { providerType: product, authBinding: { source: "managed_account", authProvider: product,
          accountId: "buddy-account", authIdentityGeneration: 1 } } },
      identity: { status: "bound" }, credentialConfigured: true, credentialSlots: [],
    }])),
  };
}

async function mount(bundle: ProviderBundleView) {
  const container = document.createElement("div"); document.body.appendChild(container);
  const root = createRoot(container);
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  client.setQueryData(["account-capabilities"], [{ providerType: bundle.familyId.slice(7),
    inferenceBindingSupported: true, credentialOwnership: "managed_account", deprecatedForInference: false,
    supportsStartLogin: true, supportsImport: true,
  }]);
  const render = async () => {
    await act(async () => root.render(<QueryClientProvider client={client}>
      <ProviderBundleEditor bundle={bundle} onSaved={mocks.saved} onCancel={() => {}} />
    </QueryClientProvider>));
    await act(async () => { await new Promise((resolve) => setTimeout(resolve, 0)); });
  };
  await render();
  return {
    container, render,
    selector: () => container.querySelector<HTMLSelectElement>(`#${bundle.familyId.slice(7)}-login-site`)!,
    save: () => Array.from(container.querySelectorAll<HTMLButtonElement>("button"))
      .find((button) => button.textContent === i18n.t("common.save"))!,
    close: async () => { await act(async () => root.unmount()); client.clear(); container.remove(); },
  };
}

async function choose(select: HTMLSelectElement, value: CodeBuddySite) {
  await act(async () => { select.value = value; select.dispatchEvent(new Event("change", { bubbles: true })); });
}

const initialCheckin: WorkBuddyCheckinView = {
  supported: true, reason: null, enabled: false, time: "09:00", timezone: "Asia/Shanghai", revision: 0,
  todayCheckedIn: false, pending: false, lastResult: null, nextRetryAt: null, quotaPending: false,
};

beforeEach(() => {
  vi.resetAllMocks(); mocks.accounts = []; mocks.cancel.mockResolvedValue(undefined);
  mocks.getCheckin.mockResolvedValue(initialCheckin);
  mocks.setCheckinPolicy.mockImplementation(async (_id, _generation, revision, enabled, time) => ({
    ...initialCheckin, revision: revision + 1, enabled, time,
  }));
  mocks.runCheckin.mockResolvedValue({ ...initialCheckin, todayCheckedIn: true,
    lastResult: { status: "success", atMs: Date.now(), message: "checked_in" } });
});

describe("Buddy provider check-in entry", () => {
  it("lets the bound WorkBuddy account opt in and check in without saving the provider", async () => {
    mocks.accounts = [account("workbuddy_oauth", "cn")];
    const view = await mount(fixture("workbuddy_oauth"));
    try {
      await act(async () => { await new Promise((resolve) => setTimeout(resolve, 0)); });
      expect(mocks.getCheckin).toHaveBeenCalledWith("buddy-account", 1);
      expect(mocks.setCheckinPolicy).not.toHaveBeenCalled();
      expect(mocks.runCheckin).not.toHaveBeenCalled();
      const toggle = view.container.querySelector<HTMLButtonElement>(`[role="switch"][aria-label="${i18n.t("workBuddyCheckin.auto")}"]`)!;
      expect(toggle).not.toBeNull();
      await act(async () => toggle.click());
      await act(async () => { await new Promise((resolve) => setTimeout(resolve, 0)); });
      expect(mocks.setCheckinPolicy).toHaveBeenCalledWith("buddy-account", 1, 0, true, "09:00");
      const run = Array.from(view.container.querySelectorAll<HTMLButtonElement>("button"))
        .find((button) => button.textContent === i18n.t("workBuddyCheckin.now"))!;
      expect(run).toBeDefined();
      await act(async () => run.click());
      await act(async () => { await new Promise((resolve) => setTimeout(resolve, 0)); });
      expect(mocks.runCheckin).toHaveBeenCalledWith("buddy-account", 1);
      expect(run.disabled).toBe(true);
      expect(view.save().disabled).toBe(true);
      expect(mocks.upsert).not.toHaveBeenCalled();
    } finally { await view.close(); }
  });

  it("explains the unsupported CodeBuddy product without querying or signing in", async () => {
    mocks.accounts = [account("codebuddy_oauth", "cn")];
    const view = await mount(fixture("codebuddy_oauth"));
    try {
      expect(view.container.textContent).toContain(i18n.t("workBuddyCheckin.unsupportedProvider"));
      expect(mocks.getCheckin).not.toHaveBeenCalled();
      expect(mocks.runCheckin).not.toHaveBeenCalled();
    } finally { await view.close(); }
  });

  it("keeps international accounts unsupported even when the new-login preference is China", async () => {
    mocks.accounts = [account("workbuddy_oauth", "intl")];
    mocks.getCheckin.mockResolvedValue({ ...initialCheckin, supported: false, reason: "unsupported_site" });
    const view = await mount(fixture("workbuddy_oauth", "cn"));
    try {
      await act(async () => { await new Promise((resolve) => setTimeout(resolve, 0)); });
      expect(view.container.textContent).toContain(i18n.t("workBuddyCheckin.unsupported"));
      expect(Array.from(view.container.querySelectorAll("button"))
        .some((button) => button.textContent === i18n.t("workBuddyCheckin.now"))).toBe(false);
      expect(mocks.runCheckin).not.toHaveBeenCalled();
      expect(mocks.setCheckinPolicy).not.toHaveBeenCalled();
    } finally { await view.close(); }
  });

  it("explains a stale account binding without operating on its replacement", async () => {
    mocks.accounts = [{ ...account("workbuddy_oauth", "cn"), authIdentityGeneration: 2 }];
    const view = await mount(fixture("workbuddy_oauth"));
    try {
      expect(view.container.textContent).toContain(i18n.t("workBuddyCheckin.bindingUnavailable"));
      expect(mocks.getCheckin).not.toHaveBeenCalled();
      expect(mocks.runCheckin).not.toHaveBeenCalled();
    } finally { await view.close(); }
  });
});

describe.each(["workbuddy_oauth", "codebuddy_oauth"] as const)("%s login site", (product) => {
  it("marks site-only edits dirty, retains a failed save, then saves and reopens the selected site", async () => {
    mocks.accounts = [account(product, "cn")];
    const bundle = fixture(product, "intl");
    const view = await mount(bundle);
    const saved = { ...bundle, buddyLoginSite: "cn" as const, revision: 2 };
    try {
      expect(view.selector().value).toBe("intl");
      expect(view.save().disabled).toBe(true);
      await choose(view.selector(), "cn");
      expect(view.save().disabled).toBe(false);
      mocks.get.mockResolvedValue(bundle);
      mocks.upsert.mockRejectedValueOnce(new HttpResponseError("offline", 503));
      await act(async () => view.save().click());
      expect(view.selector().value).toBe("cn");
      expect(mocks.saved).not.toHaveBeenCalled();
      mocks.upsert.mockResolvedValueOnce(saved);
      await act(async () => view.save().click());
      expect(mocks.upsert.mock.lastCall?.[0]).toMatchObject({ buddyLoginSite: "cn", expectedRevision: 1,
        managedAccount: { accountId: "buddy-account", authIdentityGeneration: 1 } });
      expect(mocks.saved).toHaveBeenCalled();
      expect(view.save().disabled).toBe(true);
    } finally { await view.close(); }
    const reopened = await mount(saved);
    try { expect(reopened.selector().value).toBe("cn"); expect(reopened.save().disabled).toBe(true); }
    finally { await reopened.close(); }
    expect(toProviderBundleWriteDraft(duplicateProviderBundleDraft(saved)).buddyLoginSite).toBe("cn");
  });

  it("derives legacy defaults without dirtying and keeps a manual choice when account data arrives late", async () => {
    const view = await mount(fixture(product));
    const locale = product === "workbuddy_oauth" ? "workBuddyOauth" : "codeBuddyOauth";
    try {
      expect(view.selector().value).toBe("intl");
      expect(view.save().disabled).toBe(true);
      mocks.accounts = [account(product, "cn")]; await view.render();
      expect(view.selector().value).toBe("cn");
      expect(view.container.textContent).toContain(i18n.t(`${locale}.boundAccountSite`, { site: i18n.t(`${locale}.cnSite`) }));
      expect(view.save().disabled).toBe(true);
      await choose(view.selector(), "intl");
      mocks.accounts = [{ ...account(product, "cn"), displayName: "Late account result" }]; await view.render();
      expect(view.selector().value).toBe("intl");
      expect(view.save().disabled).toBe(false);
      mocks.accounts = [{ ...account(product, "cn"), authIdentityGeneration: 2 }]; await view.render();
      expect(view.container.textContent).toContain(i18n.t(`${locale}.boundAccountSite`, { site: i18n.t(`${locale}.unknownSite`) }));
      expect(view.selector().value).toBe("intl");
    } finally { await view.close(); }
  });

  it("pins the chosen site through login start/polling and locks import until cancellation", async () => {
    mocks.accounts = [account(product, "cn")];
    let finish!: (value: unknown) => void;
    mocks.start.mockReturnValue(new Promise((resolve) => { finish = resolve; }));
    const view = await mount(fixture(product));
    try {
      const login = Array.from(view.container.querySelectorAll<HTMLButtonElement>("button"))
        .find((button) => button.textContent?.includes(i18n.t("accountAuth.addAnotherAccount")))!;
      await act(async () => login.click());
      expect(mocks.start.mock.lastCall?.[5]).toBe("cn");
      expect(view.selector().disabled).toBe(true);
      expect(view.container.querySelector<HTMLTextAreaElement>("details textarea")!.disabled).toBe(true);
      mocks.accounts = [account(product, "intl")]; await view.render();
      expect(view.selector().value).toBe("cn");
      await act(async () => finish({ device_code: "site-flow", flow: "browser", site: "cn",
        verification_uri: "https://example.invalid/login", expires_in: 60, interval: 1 }));
      expect(view.selector().disabled).toBe(true);
      const cancel = Array.from(view.container.querySelectorAll<HTMLButtonElement>("button"))
        .find((button) => button.textContent === i18n.t("common.cancel"))!;
      await act(async () => cancel.click());
      expect(mocks.cancel).toHaveBeenCalledWith(product, "site-flow");
      expect(view.selector().disabled).toBe(false);
      expect(view.selector().value).toBe("intl");
    } finally { await view.close(); }
  });

  it("uses the saved preference for import preview and apply while locking login and site changes", async () => {
    mocks.accounts = [account(product, "intl")];
    let finish!: (value: unknown) => void;
    mocks.importCredentials.mockReturnValueOnce(new Promise((resolve) => { finish = resolve; }));
    const view = await mount(fixture(product, "cn"));
    const locale = product === "workbuddy_oauth" ? "workBuddyOauth" : "codeBuddyOauth";
    const payload = { accessToken: "fixture-import", uid: "fixture-uid" };
    try {
      const textarea = view.container.querySelector<HTMLTextAreaElement>("details textarea")!;
      const importButton = () => view.container.querySelector<HTMLButtonElement>("details button")!;
      const login = Array.from(view.container.querySelectorAll<HTMLButtonElement>("button"))
        .find((button) => button.textContent === i18n.t("accountAuth.addAnotherAccount"))!;
      await act(async () => {
        Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value")!.set!.call(textarea, JSON.stringify(payload));
        textarea.dispatchEvent(new Event("input", { bubbles: true }));
      });
      await act(async () => importButton().click());
      expect(mocks.importCredentials).toHaveBeenCalledWith(product, "cn", payload, true);
      expect(view.selector().disabled).toBe(true);
      expect(textarea.disabled).toBe(true);
      expect(login.disabled).toBe(true);
      await act(async () => login.click());
      expect(mocks.start).not.toHaveBeenCalled();
      await act(async () => finish({ preview: true, accounts: [{ id: "buddy-account" }] }));
      expect(view.selector().disabled).toBe(false);
      expect(view.selector().value).toBe("cn");
      expect(importButton().textContent).toBe(i18n.t(`${locale}.applyImport`, { count: 1 }));
      mocks.importCredentials.mockResolvedValueOnce({ preview: false, accounts: [{ id: "buddy-account" }] });
      await act(async () => importButton().click());
      expect(mocks.importCredentials).toHaveBeenLastCalledWith(product, "cn", payload, false);
      expect(textarea.value).toBe("");
      expect(view.selector().value).toBe("cn");
    } finally { await view.close(); }
  });

  it("unlocks the site after an immediately rejected login, including repeated failures", async () => {
    mocks.accounts = [account(product, "cn")];
    mocks.start.mockRejectedValue(new Error("fixture login failure"));
    const view = await mount(fixture(product));
    try {
      const login = () => Array.from(view.container.querySelectorAll<HTMLButtonElement>("button"))
        .find((button) => button.textContent?.includes(i18n.t("accountAuth.addAnotherAccount")))!;
      for (let attempt = 0; attempt < 2; attempt += 1) {
        await act(async () => login().click());
        await act(async () => { await new Promise((resolve) => setTimeout(resolve, 0)); });
        expect(mocks.start).toHaveBeenCalledTimes(attempt + 1);
        expect(view.selector().disabled).toBe(false);
        expect(view.container.querySelector<HTMLTextAreaElement>("details textarea")!.disabled).toBe(false);
      }
    } finally { await view.close(); }
  });

  it("keeps a derived site fixed during import and clears the preview if the bound account's site changes", async () => {
    mocks.accounts = [account(product, "cn")];
    let finish!: (value: unknown) => void;
    mocks.importCredentials.mockReturnValue(new Promise((resolve) => { finish = resolve; }));
    const view = await mount(fixture(product));
    try {
      const textarea = view.container.querySelector<HTMLTextAreaElement>("details textarea")!;
      await act(async () => {
        Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value")!.set!.call(textarea, "{}");
        textarea.dispatchEvent(new Event("input", { bubbles: true }));
      });
      await act(async () => view.container.querySelector<HTMLButtonElement>("details button")!.click());
      mocks.accounts = [account(product, "intl")]; await view.render();
      expect(view.selector().value).toBe("cn");
      expect(view.selector().disabled).toBe(true);
      await act(async () => finish({ preview: true, accounts: [{ id: "preview" }] }));
      expect(view.selector().value).toBe("intl");
      expect(textarea.value).toBe("");
      expect(view.container.querySelector<HTMLButtonElement>("details button")!.disabled).toBe(true);
      expect(view.save().disabled).toBe(true);
    } finally { await view.close(); }
  });
});

it("ignores a late import result after leaving a product and returning to the same site", async () => {
  const container = document.createElement("div"); document.body.appendChild(container);
  const root = createRoot(container);
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const selectAccount = vi.fn();
  client.setQueryData(["account-capabilities"], ["workbuddy_oauth", "codebuddy_oauth"].map((providerType) => ({
    providerType, inferenceBindingSupported: true, credentialOwnership: "managed_account",
    deprecatedForInference: false, supportsStartLogin: true, supportsImport: true,
  })));
  const render = async (product: BuddyProviderType) => {
    mocks.accounts = [account(product, "cn")];
    await act(async () => root.render(<QueryClientProvider client={client}>
      <ManagedAccountSection providerType={product} selectedAccountId="buddy-account"
        selectedAccountGeneration={1} buddyLoginSite="cn" onAccountSelect={selectAccount} />
    </QueryClientProvider>));
  };
  let finish!: (value: unknown) => void;
  mocks.importCredentials.mockResolvedValueOnce({ preview: true, accounts: [{ id: "import-preview" }] })
    .mockReturnValueOnce(new Promise((resolve) => { finish = resolve; }));
  try {
    await render("workbuddy_oauth");
    const textarea = container.querySelector<HTMLTextAreaElement>("textarea")!;
    await act(async () => {
      Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value")!.set!.call(textarea, "{}");
      textarea.dispatchEvent(new Event("input", { bubbles: true }));
    });
    await act(async () => container.querySelector<HTMLButtonElement>("details button")!.click());
    await act(async () => container.querySelector<HTMLButtonElement>("details button")!.click());
    expect(mocks.importCredentials).toHaveBeenLastCalledWith("workbuddy_oauth", "cn", {}, false);
    await render("codebuddy_oauth");
    await render("workbuddy_oauth");
    await act(async () => finish({ preview: false, accounts: [{ id: "late-import" }] }));
    expect(selectAccount).not.toHaveBeenCalled();
    expect(container.querySelector<HTMLTextAreaElement>("textarea")!.value).toBe("");
    expect(container.querySelector<HTMLButtonElement>("details button")!.disabled).toBe(true);
    expect(container.querySelector<HTMLSelectElement>("#workbuddy_oauth-login-site")!.disabled).toBe(false);
  } finally {
    await act(async () => root.unmount()); client.clear(); container.remove();
  }
});
