import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { CodeBuddySite, ManagedAuthAccount } from "@/lib/api/auth";
import type { BuddyProviderType } from "@/lib/api/providers";
import type { AccountManagerCapability } from "@/lib/server-legacy-api";
import { SERVER_DEFAULT_SETTINGS } from "@/lib/serverDefaultSettings";
import { oauthQuotaAccountKey } from "@/lib/query/oauthQuotaKeys";
import i18n from "@/i18n";
import { AuthCenterPanel } from "./AuthCenterPanel";

const mocks = vi.hoisted(() => ({
  loadCapabilities: vi.fn(), start: vi.fn(), poll: vi.fn(), cancel: vi.fn(),
  importCredentials: vi.fn(), listAccounts: vi.fn(), setDefault: vi.fn(),
  remove: vi.fn(), logout: vi.fn(), saveSettings: vi.fn(), error: vi.fn(),
  accounts: [] as ManagedAuthAccount[],
}));
vi.mock("sonner", () => ({ toast: { error: mocks.error, success: vi.fn() } }));
vi.mock("@/lib/server-legacy-api", async (original) => ({
  ...await original<object>(), loadAccountCapabilities: mocks.loadCapabilities,
}));
vi.mock("@/lib/api/auth", async (original) => ({
  ...await original<object>(), authImportBuddyCredentials: mocks.importCredentials,
  authListAccounts: mocks.listAccounts,
}));
vi.mock("@/lib/api", async (original) => ({
  ...await original<object>(),
  settingsApi: { get: async () => SERVER_DEFAULT_SETTINGS, save: mocks.saveSettings },
  authApi: {
    authGetStatus: async (provider: string) => ({
      provider, authenticated: mocks.accounts.some((account) => account.provider === provider),
      default_account_id: mocks.accounts.find((account) => account.provider === provider && account.is_default)?.id ?? null,
      accounts: mocks.accounts.filter((account) => account.provider === provider),
    }),
    authStartLogin: mocks.start, authPollForAccount: mocks.poll,
    authCancelLogin: mocks.cancel, authSetDefaultAccount: mocks.setDefault,
    authRemoveAccount: mocks.remove, authLogout: mocks.logout,
  },
}));

// Use native select events in jsdom; viewport and Radix pointer checks remain
// manual. The real AuthCenter, Buddy section, account hook and Accordion run.
vi.mock("@/components/ui/select", async () => {
  const React = await import("react");
  const SelectTrigger = () => null;
  return {
    Select: ({ children, value, onValueChange, disabled }: {
      children: React.ReactNode; value?: string; disabled?: boolean;
      onValueChange?: (value: string) => void;
    }) => {
      const trigger = React.Children.toArray(children).find((child) => React.isValidElement(child) && child.type === SelectTrigger);
      const props = React.isValidElement<Record<string, unknown>>(trigger) ? trigger.props : {};
      return <select {...props} value={value ?? ""} disabled={disabled}
        onChange={(event) => onValueChange?.(event.target.value)}>{children}</select>;
    },
    SelectTrigger, SelectValue: () => null,
    SelectContent: ({ children }: { children: React.ReactNode }) => <>{children}</>,
    SelectItem: ({ value, children }: { value: string; children: React.ReactNode }) => <option value={value}>{children}</option>,
  };
});

function capability(providerType: string, overrides: Partial<AccountManagerCapability> = {}): AccountManagerCapability {
  return {
    providerType, manager: "manual_token_store_with_native_refresh", managerKind: "native_oauth",
    support: "manual_token_store", status: "native_login_refresh",
    loginFlows: [{ kind: "browser_oauth", supportsCallback: false, supportsPoll: true, supportsCancel: true }],
    supportsStartLogin: true, supportsCallback: false, supportsImport: true, supportsDelete: true,
    supportsRefresh: true, supportsQuota: true, supportsRefreshPlan: true, supportsCachedQuota: true,
    supportsLiveQuotaRefresh: true, refreshCapability: "provider_dynamic", quotaCapability: "live_refresh",
    credentialOwnership: "managed_account", inferenceBindingSupported: true, deprecatedForInference: false,
    migrationTarget: null, subscriptionExpiryCapability: "automatic", ...overrides,
  };
}

function account(product: BuddyProviderType, site: CodeBuddySite, suffix: string = site): ManagedAuthAccount {
  return {
    id: `${product}-${suffix}`, provider: product, authIdentityGeneration: 1,
    login: "opaque-id", displayName: `${product}-${suffix}@example.com`, avatar_url: null,
    authenticated_at: 1, is_default: site === "cn", github_domain: "", buddy: { site },
    subscriptionExpiry: { capability: "automatic", rule: null, ruleNextExpiresAt: null,
      automaticExpiresAt: null, legacyManualExpiresAt: null, manualExpiresAt: null,
      effectiveExpiresAt: null, source: null, kind: null },
  };
}

const products = ["codebuddy_oauth", "workbuddy_oauth"] as const;
const label = (product: BuddyProviderType) => product === "workbuddy_oauth" ? "WorkBuddy" : "CodeBuddy";
const namespace = (product: BuddyProviderType) => product === "workbuddy_oauth" ? "workBuddyOauth" : "codeBuddyOauth";
let container: HTMLDivElement;
let root: Root;
let client: QueryClient;

async function settle() {
  await act(async () => { await new Promise((resolve) => setTimeout(resolve, 0)); });
}
async function click(button: HTMLElement) {
  await act(async () => button.click());
  await settle();
}
async function mount(capabilities: AccountManagerCapability[] | null = products.map((product) => capability(product))) {
  if (capabilities) client.setQueryData(["account-capabilities"], capabilities);
  await act(async () => root.render(
    <QueryClientProvider client={client}><AuthCenterPanel serverMode /></QueryClientProvider>,
  ));
  await settle();
}
function trigger(product: BuddyProviderType) {
  return Array.from(container.querySelectorAll<HTMLButtonElement>("button"))
    .find((button) => button.querySelector("h3")?.textContent === `${label(product)} OAuth`)!;
}
function panel(product: BuddyProviderType) {
  return trigger(product).closest<HTMLDivElement>("div[data-state]")!;
}
function buttonWithin(element: HTMLElement, text: string) {
  const button = Array.from(element.querySelectorAll<HTMLButtonElement>("button"))
    .find((button) => button.textContent === text);
  expect(button, `missing button: ${text}`).toBeTruthy();
  return button!;
}
async function choose(product: BuddyProviderType, site: CodeBuddySite) {
  const select = panel(product).querySelector<HTMLSelectElement>("select")!;
  await act(async () => {
    select.value = site;
    select.dispatchEvent(new Event("change", { bubbles: true }));
  });
}
async function fill(element: HTMLInputElement | HTMLTextAreaElement, value: string) {
  await act(async () => {
    const prototype = element instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
    Object.getOwnPropertyDescriptor(prototype, "value")!.set!.call(element, value);
    element.dispatchEvent(new Event("input", { bubbles: true }));
  });
}

beforeEach(() => {
  vi.resetAllMocks();
  mocks.accounts = [];
  mocks.loadCapabilities.mockResolvedValue(products.map((product) => capability(product)));
  mocks.listAccounts.mockResolvedValue([]);
  mocks.cancel.mockResolvedValue(undefined);
  mocks.poll.mockResolvedValue(null);
  mocks.saveSettings.mockResolvedValue(true);
  mocks.start.mockImplementation(async (product) => ({
    provider: product, flow: "browser", device_code: `${product}-flow`, user_code: "",
    verification_uri: "https://example.com/oauth", expires_in: 600, interval: 2,
  }));
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  client.setQueryData(["settings"], SERVER_DEFAULT_SETTINGS);
  Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
});
afterEach(async () => {
  await act(async () => root.unmount());
  client.clear(); container.remove(); vi.restoreAllMocks();
});

describe.each(products)("%s in AuthCenter", (product) => {
  it.each(["cn", "intl"] as const)("allows first login at %s without a Provider", async (site) => {
    await mount();
    await click(trigger(product));
    const section = panel(product);
    expect(section.querySelectorAll("select")).toHaveLength(1);
    expect(section.textContent).toContain(i18n.t(`${namespace(product)}.authCenterSiteHint`));
    expect(section.textContent).not.toContain(i18n.t(`${namespace(product)}.siteHint`));
    await choose(product, site);
    await click(buttonWithin(section, i18n.t(`${namespace(product)}.loginWithCodeBuddy`)));
    expect(mocks.start).toHaveBeenCalledWith(product, undefined, undefined, undefined, undefined, site, undefined);
    expect(section.querySelector<HTMLSelectElement>("select")!.disabled).toBe(true);
    expect(section.querySelector<HTMLButtonElement>("details button")!.disabled).toBe(true);
    await click(buttonWithin(section, i18n.t("common.cancel")));
    expect(mocks.cancel).toHaveBeenCalledWith(product, `${product}-flow`);
    expect(section.querySelector<HTMLSelectElement>("select")!.disabled).toBe(false);
    expect(mocks.listAccounts).not.toHaveBeenCalled();
  });

  it.each(["cn", "intl"] as const)("previews and imports at %s, then updates account and quota views", async (site) => {
    await mount(); await click(trigger(product)); await choose(product, site);
    const section = panel(product);
    const payload = { uid: "fixture-uid", accessToken: "fixture-access", refreshToken: "fixture-refresh" };
    await fill(section.querySelector("textarea")!, JSON.stringify(payload));
    mocks.importCredentials.mockResolvedValueOnce({ preview: true, accounts: [{ id: "preview" }] });
    await click(section.querySelector<HTMLButtonElement>("details button")!);
    expect(mocks.importCredentials).toHaveBeenLastCalledWith(product, site, payload, true);
    expect(section.querySelector("details button")?.textContent).toBe(i18n.t(`${namespace(product)}.applyImport`, { count: 1 }));
    const imported = account(product, site);
    const quotaKey = oauthQuotaAccountKey(product, imported.id, imported.authIdentityGeneration);
    client.setQueryData(quotaKey, { remaining: 1 });
    mocks.importCredentials.mockImplementationOnce(async () => {
      mocks.accounts = [imported]; return { preview: false, accounts: [imported] };
    });
    await click(section.querySelector<HTMLButtonElement>("details button")!);
    expect(mocks.importCredentials).toHaveBeenLastCalledWith(product, site, payload, false);
    expect(section.textContent).toContain(imported.displayName);
    expect(section.querySelector("textarea")!.value).toBe("");
    expect(section.querySelectorAll("select")).toHaveLength(1);
    expect(client.getQueryState(quotaKey)?.isInvalidated).toBe(true);
    expect(mocks.saveSettings).not.toHaveBeenCalled();
  });

  it("refreshes the account list after successful OAuth polling", async () => {
    const added = account(product, "cn");
    const quotaKey = oauthQuotaAccountKey(product, added.id, added.authIdentityGeneration);
    client.setQueryData(quotaKey, { remaining: 10 });
    mocks.poll.mockImplementationOnce(async () => { mocks.accounts = [added]; return added; });
    await mount(); await click(trigger(product)); await choose(product, "cn");
    await click(buttonWithin(panel(product), i18n.t(`${namespace(product)}.loginWithCodeBuddy`)));
    await settle();
    expect(mocks.poll).toHaveBeenCalledWith(product, `${product}-flow`, undefined, undefined);
    expect(panel(product).textContent).toContain(added.displayName);
    expect(panel(product).querySelector<HTMLSelectElement>("select")!.disabled).toBe(false);
    expect(panel(product).querySelectorAll("select")).toHaveLength(1);
    expect(client.getQueryState(quotaKey)?.isInvalidated).toBe(true);
  });

  it("shows each account's actual site and isolates default/remove/logout operations", async () => {
    const other = products.find((item) => item !== product)!;
    const cn = account(product, "cn"); const intl = account(product, "intl");
    const unknown = { ...account(product, "intl", "unknown"), buddy: undefined };
    const otherAccount = account(other, "cn");
    mocks.accounts = [cn, intl, unknown, otherAccount];
    mocks.setDefault.mockImplementation(async (provider, id) => {
      mocks.accounts = mocks.accounts.map((item) => item.provider === provider ? { ...item, is_default: item.id === id } : item);
    });
    mocks.remove.mockImplementation(async (provider, id) => {
      mocks.accounts = mocks.accounts.filter((item) => item.provider !== provider || item.id !== id);
    });
    mocks.logout.mockImplementation(async (provider) => {
      mocks.accounts = mocks.accounts.filter((item) => item.provider !== provider);
    });
    await mount(); await click(trigger(product));
    const section = panel(product);
    const row = (name: string) => Array.from(section.querySelectorAll("span"))
      .find((span) => span.textContent === name)!.parentElement!.parentElement!;
    expect(row(cn.displayName!).textContent).toContain(i18n.t(`${namespace(product)}.cnSite`));
    expect(row(intl.displayName!).textContent).toContain(i18n.t(`${namespace(product)}.intlSite`));
    expect(row(unknown.displayName!).textContent).toContain(i18n.t(`${namespace(product)}.unknownSite`));
    expect(section.textContent).not.toContain(otherAccount.displayName);
    await choose(product, "intl");
    expect(row(cn.displayName!).textContent).toContain(i18n.t(`${namespace(product)}.cnSite`));
    await click(buttonWithin(row(intl.displayName!), i18n.t("accountAuth.setAsDefault")));
    expect(mocks.setDefault).toHaveBeenCalledWith(product, intl.id);
    expect(row(intl.displayName!).textContent).toContain(i18n.t("accountAuth.defaultAccount"));
    await click(row(cn.displayName!).querySelector<HTMLButtonElement>(`button[title="${i18n.t("accountAuth.removeAccount")}"]`)!);
    expect(mocks.remove).toHaveBeenCalledWith(product, cn.id);
    expect(section.textContent).not.toContain(cn.displayName);
    await click(buttonWithin(section, i18n.t("accountAuth.logoutAll")));
    expect(mocks.logout).toHaveBeenCalledWith(product);
    expect(mocks.accounts).toEqual([otherAccount]);
    expect(section.textContent).toContain(i18n.t("accountAuth.notAuthenticated"));
  });

  it("retains an account and reports an in-use deletion failure", async () => {
    const bound = account(product, "cn"); mocks.accounts = [bound];
    mocks.remove.mockRejectedValue(new Error("account is still referenced by 1 Provider(s)"));
    vi.spyOn(console, "error").mockImplementation(() => {});
    await mount(); await click(trigger(product));
    const section = panel(product);
    await click(section.querySelector<HTMLButtonElement>(`button[title="${i18n.t("accountAuth.removeAccount")}"]`)!);
    expect(section.textContent).toContain(bound.displayName);
    expect(section.textContent).toContain("account is still referenced");
    expect(mocks.error).toHaveBeenCalledWith("account is still referenced by 1 Provider(s)");
  });
});

it("keeps page-local site choices separate across products and Accordion remounts", async () => {
  await mount(); await click(trigger("codebuddy_oauth")); await click(trigger("workbuddy_oauth"));
  await choose("workbuddy_oauth", "cn");
  expect(panel("codebuddy_oauth").querySelector<HTMLSelectElement>("select")!.value).toBe("intl");
  await click(trigger("workbuddy_oauth")); await click(trigger("workbuddy_oauth"));
  expect(panel("workbuddy_oauth").querySelector<HTMLSelectElement>("select")!.value).toBe("cn");
  expect(mocks.saveSettings).not.toHaveBeenCalled();
  await act(async () => root.render(<div />));
  await mount(); await click(trigger("workbuddy_oauth"));
  expect(panel("workbuddy_oauth").querySelector<HTMLSelectElement>("select")!.value).toBe("intl");
});

it.each(products)("refreshes %s accounts after an import completes across Accordion remounts without restoring old form state", async (product) => {
  await mount(); await click(trigger(product)); await choose(product, "cn");
  await fill(panel(product).querySelector("textarea")!, "{}");
  mocks.importCredentials.mockResolvedValueOnce({ preview: true, accounts: [{ id: "preview" }] });
  await click(panel(product).querySelector<HTMLButtonElement>("details button")!);
  let finish!: (result: object) => void;
  mocks.importCredentials.mockReturnValueOnce(new Promise((resolve) => { finish = resolve; }));
  await click(panel(product).querySelector<HTMLButtonElement>("details button")!);
  await click(trigger(product)); await click(trigger(product));
  await choose(product, "intl");
  const imported = account(product, "cn");
  mocks.accounts = [imported];
  await act(async () => finish({ preview: false, accounts: [imported] })); await settle();
  const section = panel(product);
  expect(section.textContent).toContain(imported.displayName);
  expect(section.querySelector<HTMLSelectElement>("select")!.value).toBe("intl");
  expect(section.querySelector("textarea")!.value).toBe("");
  expect(section.querySelector("details button")!.textContent).toBe(i18n.t(`${namespace(product)}.previewImport`));
});

it.each([
  {}, { inferenceBindingSupported: false }, { credentialOwnership: "metadata_only" },
  { deprecatedForInference: true }, { supportsStartLogin: false, supportsImport: false },
])("hides missing or ineligible Buddy capabilities: %j", async (overrides) => {
  const capabilities = Object.keys(overrides).length
    ? products.map((product) => capability(product, overrides as Partial<AccountManagerCapability>)) : [];
  await mount(capabilities);
  for (const product of products) expect(trigger(product)).toBeUndefined();
  expect(mocks.start).not.toHaveBeenCalled();
});

it.each(products)("shows only %s when it is the sole eligible Buddy capability", async (product) => {
  await mount([capability(product)]);
  expect(trigger(product)).toBeTruthy();
  expect(trigger(products.find((item) => item !== product)!)).toBeUndefined();
});

it("suspends entries while capabilities load and supports retry after failure", async () => {
  let reject!: (reason: Error) => void;
  mocks.loadCapabilities.mockReturnValueOnce(new Promise((_, fail) => { reject = fail; }));
  await mount(null);
  expect(container.textContent).toContain(i18n.t("settings.authCenter.capabilityLoading"));
  expect(trigger("workbuddy_oauth")).toBeUndefined();
  await act(async () => reject(new Error("offline"))); await settle();
  expect(container.textContent).toContain(i18n.t("settings.authCenter.capabilityLoadFailed"));
  expect(trigger("codebuddy_oauth")).toBeUndefined();
  await click(buttonWithin(container, i18n.t("common.retry")));
  expect(trigger("codebuddy_oauth")).toBeTruthy(); expect(trigger("workbuddy_oauth")).toBeTruthy();
});

it.each([true, false])("invalidates only live Buddy quota roots when saving settings (WorkBuddy live=%s)", async (workBuddyLive) => {
  const keys = products.map((product) => oauthQuotaAccountKey(product, `${product}-cn`, 1));
  for (const key of keys) client.setQueryData(key, { remaining: 10 });
  const cachedOnly = oauthQuotaAccountKey("cursor_oauth", "cursor-account", 1);
  client.setQueryData(cachedOnly, { remaining: 10 });
  await mount([
    capability("codebuddy_oauth"),
    capability("workbuddy_oauth", {
      supportsLiveQuotaRefresh: workBuddyLive,
      quotaCapability: workBuddyLive ? "live_refresh" : "cached_only",
    }),
  ]);
  await click(container.querySelector<HTMLButtonElement>("button[aria-expanded]")!);
  await fill(container.querySelector<HTMLInputElement>("#oauth-quota-refresh-interval")!, "45");
  await click(buttonWithin(container, i18n.t("common.save")));
  expect(mocks.saveSettings).toHaveBeenCalledWith(expect.objectContaining({ oauthQuotaRefreshIntervalMinutes: 45 }));
  expect(client.getQueryState(keys[0])?.isInvalidated).toBe(true);
  expect(client.getQueryState(keys[1])?.isInvalidated).toBe(workBuddyLive);
  expect(client.getQueryState(cachedOnly)?.isInvalidated).toBe(false);
});
