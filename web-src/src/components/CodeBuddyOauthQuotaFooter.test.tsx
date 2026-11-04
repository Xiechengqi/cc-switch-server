import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "@/i18n";
import type { SubscriptionQuota } from "@/types/subscription";
import CodeBuddyOauthQuotaFooter from "./CodeBuddyOauthQuotaFooter";

const mocks = vi.hoisted(() => ({ quota: undefined as SubscriptionQuota | undefined, refetch: vi.fn(), refresh: vi.fn() }));
vi.mock("@/lib/query/subscription", () => ({
  useCodeBuddyOauthQuota: () => ({ data: mocks.quota, isFetching: false, refetch: mocks.refetch }),
  useSubscriptionQuota: vi.fn(),
}));
vi.mock("@/lib/api/subscription", () => ({ subscriptionApi: { refreshOauthQuota: mocks.refresh } }));
Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });

beforeEach(async () => {
  vi.spyOn(Date, "now").mockReturnValue(Date.parse("2026-11-04T00:00:00Z"));
  await i18n.changeLanguage("en");
  mocks.refresh.mockReset().mockResolvedValue({});
  mocks.refetch.mockReset().mockResolvedValue({});
  mocks.quota = {
    tool: "workbuddy_oauth", credentialStatus: "valid", credentialMessage: "Buddy",
    success: true, tiers: [{ name: "codebuddy_package_2", label: "CodeBuddy个人体验版",
      utilization: 0.1, used: 0.5, limit: 500, unit: "credits", resetsAt: "2026-11-24T12:00:00Z",
      expiresAt: "2035-03-01T00:00:00Z" }], extraUsage: null, error: null,
    queriedAt: 1793706177823,
    providerUsage: { status: "complete", scope: "account", source: "buddy_official_request_usage", timezone: "Asia/Shanghai",
      collectedAt: 1793704378549, rangeStart: "2026-10-04", rangeEnd: "2026-11-03", requestCount: 132,
      usageToday: 0.45, usage7Days: 0.45, usageThisMonth: 0.45, usageTotal: 0.45 },
  };
});

afterEach(() => vi.restoreAllMocks());

async function render(product: "workbuddy_oauth" | "codebuddy_oauth", inline = true) {
  const container = document.createElement("div"); document.body.appendChild(container);
  const root = createRoot(container);
  await act(async () => { root.render(<CodeBuddyOauthQuotaFooter inline={inline} meta={{ providerType: product,
    authBinding: { source: "managed_account", authProvider: product, accountId: "fixture-account", authIdentityGeneration: 1 } }} />); });
  return { container, close: async () => { await act(async () => root.unmount()); container.remove(); } };
}

describe.each(["workbuddy_oauth", "codebuddy_oauth"] as const)("%s quota", (product) => {
  it.each([true, false])("shows cycle countdowns and official usage in the same summary (inline=%s)", async (inline) => {
    const view = await render(product, inline);
    try {
      const text = view.container.textContent!;
      expect(text).toContain("CodeBuddy个人体验版");
      expect(text).toContain("Resource packages: CodeBuddy个人体验版 0.5/500 credits 20d12h · Official usage (account): today 0.45 · this month 0.45 credits");
      expect(view.container.querySelector("button")?.parentElement?.textContent).toContain("Official usage (account)");
      expect(text.match(/CodeBuddy个人体验版/g)).toHaveLength(1);
      expect(text).not.toMatch(/%|cycle ends|expires|7 days|requests|collected|2026-|UTC|including other shares/);
    } finally { await view.close(); }
  });

  it("keeps missing usage unknown and still displays a valid package snapshot", async () => {
    mocks.quota!.providerUsage = { status: "complete", usageToday: 0.45 };
    const view = await render(product);
    try {
      expect(view.container.textContent).toContain("0.5/500 credits");
      expect(view.container.textContent).toContain("Official usage is temporarily unavailable");
      expect(view.container.textContent).not.toContain("today 0.00");
      expect(view.container.textContent).not.toContain("collected");
    } finally { await view.close(); }
  });

  it("manual refresh keeps the bound product and forces both upstream snapshots", async () => {
    const view = await render(product);
    try {
      await act(async () => view.container.querySelector("button")!.click());
      expect(mocks.refresh).toHaveBeenCalledWith(product, "fixture-account", product, undefined, undefined, true, 1);
      expect(mocks.refetch).toHaveBeenCalled();
    } finally { await view.close(); }
  });

  it("keeps the official ledger visible when resource package refresh failed", async () => {
    mocks.quota!.success = false;
    mocks.quota!.error = "Package refresh unavailable";
    const view = await render(product);
    try {
      expect(view.container.textContent).toContain("today 0.45");
      expect(view.container.textContent).not.toContain("0.5/500 credits");
    } finally { await view.close(); }
  });

  it("labels the core accounting values in Chinese", async () => {
    await i18n.changeLanguage("zh");
    mocks.quota!.tiers[0].resetsAt = null;
    mocks.quota!.tiers[0].expiresAt = "2026-11-24T12:00:00Z";
    const view = await render(product);
    try {
      expect(view.container.textContent).toContain("资源包： CodeBuddy个人体验版 0.5/500 credits 20d12h · 官方用量（账号）：今日 0.45 · 本月 0.45 credits");
      expect(view.container.textContent).not.toMatch(/采集|请求数|7 日|到期|周期结束|2026-|132 次/);
    } finally { await view.close(); }
  });
});
