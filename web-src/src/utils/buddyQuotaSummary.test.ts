import { describe, expect, it } from "vitest";
import i18n from "@/i18n";
import type { QuotaTier } from "@/types/subscription";
import { buildBuddyQuotaSummary } from "./buddyQuotaSummary";

const now = Date.parse("2026-11-06T00:00:00Z");
const deadline = (days: number, hours = 0) => new Date(now + (days * 24 + hours) * 3_600_000).toISOString();
const t = i18n.getFixedT("zh");
const tier = (label: string, used = 0, limit = 100, expiresAt: string | null = deadline(29)): QuotaTier => ({
  name: "codebuddy_package_1", label, used, limit, expiresAt, resetsAt: null, utilization: used / limit * 100, unit: "credits",
});
const checkin = "CodeBuddy个人版国内运营裂变包";

describe("Buddy display-only quota summary", () => {
  it("summarizes the reported account and excludes the exhausted earliest batch", () => {
    const tiers = [tier("CodeBuddy个人版拉新权益包", 291.97, 1500, deadline(28, 4)),
      tier(checkin, 0, 100, deadline(28, 23)), tier(checkin, 0, 100, deadline(29, 23)),
      tier("CodeBuddy个人体验版", 500, 500, deadline(24, 13))];
    const before = JSON.stringify(tiers);
    const result = buildBuddyQuotaSummary(tiers, t, now);
    expect(result.text).toBe("注册积分奖励 291.97/1500 credits · 签到 0/200 credits · 基础积分 500/500 credits · 最近到期 28d4h");
    expect(result.details).toContain("剩余 1208.03 credits");
    expect(result.details.match(new RegExp(checkin, "g"))).toHaveLength(2);
    expect(JSON.stringify(tiers)).toBe(before);
  });

  it("sums actual partial usage, keeps duplicate batches, and chooses expiry before merging", () => {
    const result = buildBuddyQuotaSummary([tier(checkin, 12.34, 80, deadline(2)), tier(checkin, 1.11, 120, deadline(1)), tier(checkin, 1.11, 120, deadline(1))], t, now);
    expect(result.text).toBe("签到 14.56/320 credits · 最近到期 1d0h");
  });

  it("uses only exact known names with whitespace tolerance and leaves unknown plans separate", () => {
    const result = buildBuddyQuotaSummary([tier("CodeBuddy 个人体验版"), tier("CodeBuddy个人版新裂变包"), tier("Free Plan Subscription"), tier("Free Plan Subscription"), tier("constructor")], t, now);
    expect(result.text).toContain("基础积分");
    expect(result.text).toContain("CodeBuddy个人版新裂变包");
    expect(result.text).toContain("constructor 0/100 credits");
    expect(result.text.match(/Free Plan Subscription/g)).toHaveLength(2);
    expect(buildBuddyQuotaSummary([{ ...tier(checkin), name: "premium" }], t, now).text).toContain(checkin);
  });

  it("never replaces expiry with reset and reports incomplete deadline evidence", () => {
    const unknown = { ...tier(checkin, 0, 100, null), resetsAt: deadline(1) };
    expect(buildBuddyQuotaSummary([unknown], t, now).text).toBe("签到 0/100 credits · 到期时间未知");
    const known = tier(checkin, 0, 100, deadline(10));
    expect(buildBuddyQuotaSummary([unknown, known], t, now).text).toContain("最近已知到期 10d0h");
    expect(buildBuddyQuotaSummary([known], t, now, true).text).toContain("最近已知到期");
    expect(buildBuddyQuotaSummary([{ ...known, resetsAt: deadline(1) }], t, now).text).toContain("最近到期 10d0h");
    expect(buildBuddyQuotaSummary([unknown], t, now).details).toContain("周期重置：");
  });

  it("does not use exhausted balances or invalid dates as expiry candidates", () => {
    expect(buildBuddyQuotaSummary([tier(checkin, 100)], t, now).text).toBe("签到 100/100 credits");
    expect(buildBuddyQuotaSummary([tier(checkin, 0, 100, "bad")], t, now).text).toContain("到期时间未知");
    expect(buildBuddyQuotaSummary([tier(checkin, 100, 100, null)], t, now).text).not.toContain("到期");
  });

  it("excludes expired batches at the exact boundary and marks the snapshot pending refresh", () => {
    const expired = tier(checkin, 20, 100, deadline(0));
    expect(buildBuddyQuotaSummary([expired, tier(checkin)], t, now).text).toBe("签到 0/100 credits · 最近到期 29d0h · 到期状态待刷新");
    expect(buildBuddyQuotaSummary([expired], t, now).text).toBe("到期状态待刷新");
    expect(buildBuddyQuotaSummary([expired], t, now - 1).text).toContain("最近到期 1m");
  });

  it("does not merge unknown units or invalid monetary values, or infer balance from rounded percent", () => {
    const result = buildBuddyQuotaSummary([tier(checkin), { ...tier(checkin), unit: "USD" }, { ...tier(checkin), used: NaN, utilization: 0.01 }], t, now);
    expect(result.text).toContain("签到 0/100 credits · 签到 0/100 USD · 签到 <0.1%");
    expect(result.text).toContain("最近已知到期");
    expect(buildBuddyQuotaSummary([{ ...tier(checkin), used: 99.9999, utilization: 100 }], t, now).text).toContain("最近到期");
  });
});
