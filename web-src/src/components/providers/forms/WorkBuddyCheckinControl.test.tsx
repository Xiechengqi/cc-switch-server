import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "@/i18n";
import type { ManagedAuthAccount, WorkBuddyCheckinView } from "@/lib/api/auth";
import { WorkBuddyCheckinControl } from "./WorkBuddyCheckinControl";

const mocks = vi.hoisted(() => ({ get: vi.fn(), set: vi.fn(), run: vi.fn() }));
vi.mock("@/lib/api/auth", () => ({ authGetCheckin: mocks.get, authSetCheckinPolicy: mocks.set, authCheckin: mocks.run }));

const a = { id: "account-cn", provider: "workbuddy_oauth", authIdentityGeneration: 3, buddy: { site: "cn" } } as ManagedAuthAccount;
const initial: WorkBuddyCheckinView = { supported: true, reason: null, enabled: false, time: "09:00", timezone: "Asia/Shanghai", revision: 0,
  todayCheckedIn: false, pending: false, lastResult: null, nextRetryAt: null, quotaPending: false };
let container: HTMLDivElement;
let root: Root;
let client: QueryClient;
async function settle() { await act(async () => { await new Promise((r) => setTimeout(r, 0)); }); }
async function mount(editable = true, account = a) {
  await act(async () => root.render(<QueryClientProvider client={client}>
    <WorkBuddyCheckinControl key={`${account.id}:${account.authIdentityGeneration}`} account={account} editable={editable} />
    {editable ? <WorkBuddyCheckinControl account={account} /> : null}
  </QueryClientProvider>));
  await settle(); await settle();
}
beforeEach(() => {
  vi.resetAllMocks(); mocks.get.mockResolvedValue(initial);
  mocks.set.mockImplementation(async (_id, _gen, revision, enabled, time) => ({ ...initial, revision: revision + 1, enabled, time }));
  mocks.run.mockResolvedValue({ ...initial, todayCheckedIn: true, lastResult: { status: "success", atMs: Date.now(), message: "checked_in" } });
  container = document.createElement("div"); document.body.appendChild(container); root = createRoot(container);
  client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
});
afterEach(async () => { await act(async () => root.unmount()); client.clear(); container.remove(); });

describe("WorkBuddy account check-in", () => {
  it("loads cached state once for both views without signing in or opting in", async () => {
    await mount();
    expect(mocks.get).toHaveBeenCalledTimes(1); expect(mocks.get).toHaveBeenCalledWith(a.id, 3);
    expect(mocks.run).not.toHaveBeenCalled(); expect(mocks.set).not.toHaveBeenCalled();
    expect(container.querySelector('[role="switch"]')?.getAttribute("aria-checked")).toBe("false");
    expect(container.querySelector<HTMLInputElement>('input[type="time"]')?.value).toBe("09:00");
  });
  it("saves an explicit opt-in with identity generation and policy revision", async () => {
    await mount();
    await act(async () => (container.querySelector('[role="switch"]') as HTMLButtonElement).click()); await settle();
    expect(mocks.set).toHaveBeenCalledWith(a.id, 3, 0, true, "09:00");
    expect(container.querySelector('[role="switch"]')?.getAttribute("aria-checked")).toBe("true");
    expect(mocks.run).not.toHaveBeenCalled();
  });
  it("manual completion updates both views and cannot immediately submit again", async () => {
    await mount();
    const button = Array.from(container.querySelectorAll("button")).find((b) => b.textContent === i18n.t("workBuddyCheckin.now"))!;
    await act(async () => button.click()); await settle(); await settle();
    expect(mocks.run).toHaveBeenCalledTimes(1); expect(mocks.run).toHaveBeenCalledWith(a.id, 3);
    expect(button.disabled).toBe(true);
    expect(Array.from(container.querySelectorAll('[role="status"]')).every((p) => p.textContent?.includes(i18n.t("workBuddyCheckin.status.checked")))).toBe(true);
  });
  it("read-only and unsupported-site views have no check-in actions", async () => {
    await mount(false);
    expect(container.querySelector("button, input")).toBeNull();
    mocks.get.mockResolvedValue({ ...initial, supported: false, reason: "unsupported_site" });
    await mount(true, { ...a, id: "account-intl", buddy: { site: "intl" } });
    expect(container.querySelector("button, input")).toBeNull();
    expect(container.textContent).toContain(i18n.t("workBuddyCheckin.unsupported"));
    expect(mocks.run).not.toHaveBeenCalled(); expect(mocks.set).not.toHaveBeenCalled();
  });
  it("honors a retry delay even when the previous failure was not an uncertain POST", async () => {
    mocks.get.mockResolvedValue({ ...initial, nextRetryAt: Date.now() + 3_600_000,
      lastResult: { status: "retryable_error", atMs: Date.now(), message: "rate_limited" } });
    await mount();
    const button = Array.from(container.querySelectorAll("button")).find((b) => b.textContent === i18n.t("workBuddyCheckin.now"))!;
    expect(button.disabled).toBe(true); expect(mocks.run).not.toHaveBeenCalled();
  });
});
