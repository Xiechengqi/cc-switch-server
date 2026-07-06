import { inferIconForText } from "@/config/iconInference";
import type { AccountManagerCapability, AccountRecord } from "@/lib/server-legacy-api";

export interface BankedResetCredit {
  id?: string | null;
  status?: string | null;
  grantedAt?: string | null;
  expiresAt?: string | null;
  title?: string | null;
  description?: string | null;
  [key: string]: unknown;
}

export interface BankedResetSummary {
  account: AccountRecord;
  availableCount: number | null;
  nextExpiresAt?: string | null;
  readOnly: boolean;
  source?: string | null;
  queriedAt?: number | null;
  credits: BankedResetCredit[];
  raw: unknown;
}

export interface AccountRegressionBadge {
  label: string;
  value: string;
  tone: "success" | "warning" | "danger";
}

type AccountQuotaTier = NonNullable<NonNullable<AccountRecord["quota"]>["tiers"]>[number];
type Tx = (key: string, vars?: Record<string, string | number | boolean | null | undefined>) => string;

export function providerLabel(providerType: string): string {
  const labels: Record<string, string> = {
    claude: "Claude API",
    claude_auth: "Claude bearer relay",
    claude_oauth: "Claude OAuth",
    codex: "OpenAI/Codex",
    codex_oauth: "OpenAI OAuth",
    gemini: "Gemini API",
    gemini_cli: "Gemini OAuth/CLI",
    openrouter: "OpenRouter",
    github_copilot: "GitHub Copilot",
    deepseek_account: "DeepSeek Account",
    kiro_oauth: "Kiro OAuth",
    cursor_oauth: "Cursor OAuth",
    cursor_apikey: "Cursor API Key",
    antigravity_oauth: "Antigravity OAuth",
    agy_oauth: "Antigravity CLI",
    ollama_cloud: "Ollama Cloud",
    aws_bedrock: "AWS Bedrock",
    nvidia: "Nvidia",
    deepseek_api: "DeepSeek API Key",
  };
  return labels[providerType] || providerType.replace(/_/g, " ");
}

export function accountProviderIcon(providerType: string): { icon?: string; color?: string } {
  const normalized = providerType
    .replace(/_oauth|_cli|_account|_apikey|_api_key|_auth|_cloud/g, " ")
    .replace(/_/g, " ");
  const inferred = inferIconForText(providerType, normalized, providerLabel(providerType));
  return { icon: inferred.icon, color: inferred.iconColor };
}

export function credentialFlags(account: AccountRecord): string[] {
  const flags: string[] = [];
  if (account.accessToken) flags.push("access");
  if (account.refreshToken) flags.push("refresh");
  if (account.apiKey) flags.push("api key");
  if (account.idToken) flags.push("id token");
  return flags;
}

export function accountRegressionBadges(
  account: AccountRecord,
  capability?: AccountManagerCapability,
): AccountRegressionBadge[] {
  return [
    loginRegressionBadge(capability),
    refreshRegressionBadge(account, capability),
    tokenRegressionBadge(account),
    quotaRegressionBadge(account, capability),
  ];
}

export function formatQuotaPercent(account: AccountRecord): string {
  const quotaPercent = accountQuotaPercent(account);
  return quotaPercent == null ? "-" : `${quotaPercent.toFixed(1)}%`;
}

export function quotaTierSummary(account: AccountRecord): string | null {
  const tiers = account.quota?.tiers || [];
  if (!tiers.length) return null;
  return tiers
    .slice(0, 2)
    .map((tier) => {
      const usage = tier.used != null && tier.limit != null ? ` ${tier.used}/${tier.limit}` : "";
      const unit = tier.unit ? ` ${tier.unit}` : "";
      return `${tier.name}${usage}${unit}`;
    })
    .join("; ");
}

export function accountQuotaPercent(account: AccountRecord): number | null {
  if (account.quotaPercent != null) return account.quotaPercent;
  const utilization = account.quota?.tiers?.find((tier) => tier.utilization != null)?.utilization;
  return utilization == null ? null : utilization;
}

export function accountTierLine(tier: AccountQuotaTier, tx: Tx): string {
  const usage = tier.used != null && tier.limit != null
    ? `${formatCompactNumber(tier.used)}/${formatCompactNumber(tier.limit)}`
    : tier.utilization == null
      ? "-"
      : `${tier.utilization.toFixed(1)}%`;
  const unit = tier.unit ? ` ${tier.unit}` : "";
  const reset = tier.resetsAt == null ? "" : ` · ${resetCountdownLabel(tier.resetsAt, tx)}`;
  return `${usage}${unit}${reset}`;
}

export function quotaRefreshedLabel(value: number | null | undefined, tx: Tx): string {
  if (value == null) return tx("not refreshed");
  return tx("refreshed {{time}}", { time: formatRelativePast(value, tx) });
}

export function quotaCountdownLabel(value: number, tx: Tx): string {
  const countdown = formatCountdown(value);
  return countdown ? tx("in {{time}}", { time: countdown }) : formatTime(value);
}

export function resetCountdownLabel(value: number, tx: Tx): string {
  const countdown = formatCountdown(value);
  return countdown ? tx("resets in {{time}}", { time: countdown }) : formatTime(value);
}

export function clampPercent(value: number): number {
  if (!Number.isFinite(value)) return 0;
  return Math.max(0, Math.min(100, value));
}

export function bankedResetSummary(account: AccountRecord): BankedResetSummary | null {
  const source =
    valueAt(account.quota?.extraUsage, ["bankedReset", "codexBankedReset"]) ??
    valueAt(account.raw, [
      "bankedReset",
      "banked_reset",
      "codexBankedReset",
      "codex_banked_reset",
      "rateLimitResetCredits",
      "rate_limit_reset_credits",
    ]);
  if (!source) return null;
  const record = asRecord(source);
  const credits = bankedResetCredits(source);
  const availableCount =
    numberValue(record?.availableCount) ??
    numberValue(record?.available_count) ??
    numberValue(record?.available) ??
    credits.filter((credit) => String(credit.status || "available").toLowerCase() === "available").length;
  const nextExpiresAt =
    stringValue(record?.nextExpiresAt) ||
    stringValue(record?.next_expires_at) ||
    nextCreditExpiry(credits);
  return {
    account,
    availableCount,
    nextExpiresAt,
    readOnly: Boolean(record?.readOnly ?? record?.read_only ?? true),
    source: stringValue(record?.source),
    queriedAt: numberValue(record?.queriedAt ?? record?.queried_at),
    credits,
    raw: source,
  };
}

export function formatTime(value?: number | null): string {
  if (!value) return "-";
  const millis = value < 10_000_000_000 ? value * 1000 : value;
  const date = new Date(millis);
  if (Number.isNaN(date.getTime())) return "-";
  return new Intl.DateTimeFormat(undefined, {
    dateStyle: "medium",
    timeStyle: "short",
  }).format(date);
}

export function formatDateish(value?: string | number | null): string {
  if (!value) return "-";
  if (typeof value === "number") return formatTime(value);
  const parsed = Date.parse(value);
  if (!Number.isFinite(parsed)) return value;
  return formatTime(parsed);
}

function loginRegressionBadge(capability?: AccountManagerCapability): AccountRegressionBadge {
  if (capability?.supportsStartLogin) {
    return { label: "login", value: "native", tone: "success" };
  }
  if (capability?.supportsImport) {
    return { label: "login", value: "import", tone: "warning" };
  }
  return { label: "login", value: "gated", tone: "warning" };
}

function refreshRegressionBadge(
  account: AccountRecord,
  capability?: AccountManagerCapability,
): AccountRegressionBadge {
  if (account.lastRefreshError) {
    return { label: "refresh", value: "error", tone: "danger" };
  }
  if (capability?.supportsRefresh && account.refreshToken) {
    return { label: "refresh", value: "ready", tone: "success" };
  }
  if (capability?.supportsRefresh) {
    return { label: "refresh", value: "no-token", tone: "warning" };
  }
  return { label: "refresh", value: "manual", tone: "warning" };
}

function tokenRegressionBadge(account: AccountRecord): AccountRegressionBadge {
  const expiry = normalizeTimestamp(account.expiresAt);
  if (expiry == null) {
    return {
      label: "token",
      value: account.accessToken || account.apiKey ? "no-expiry" : "missing",
      tone: account.accessToken || account.apiKey ? "warning" : "danger",
    };
  }
  const remaining = expiry - Date.now();
  if (remaining <= 0) return { label: "token", value: "expired", tone: "danger" };
  if (remaining <= 24 * 60 * 60 * 1000) return { label: "token", value: "soon", tone: "warning" };
  return { label: "token", value: "valid", tone: "success" };
}

function quotaRegressionBadge(
  account: AccountRecord,
  capability?: AccountManagerCapability,
): AccountRegressionBadge {
  if (!capability?.supportsQuota) {
    return { label: "quota", value: "gated", tone: "warning" };
  }
  if (!account.quota && account.quotaPercent == null) {
    return { label: "quota", value: "missing", tone: "warning" };
  }
  const refreshedAt = normalizeTimestamp(account.quotaRefreshedAt);
  if (refreshedAt == null) {
    return { label: "quota", value: "snapshot", tone: "warning" };
  }
  const age = Date.now() - refreshedAt;
  if (age <= 24 * 60 * 60 * 1000) {
    return { label: "quota", value: "fresh", tone: "success" };
  }
  if (age <= 7 * 24 * 60 * 60 * 1000) {
    return { label: "quota", value: "aged", tone: "warning" };
  }
  return { label: "quota", value: "stale", tone: "danger" };
}

function normalizeTimestamp(value?: number | null): number | null {
  if (value == null || !Number.isFinite(value)) return null;
  return value < 10_000_000_000 ? value * 1000 : value;
}

function formatRelativePast(value: number, tx: Tx): string {
  const millis = normalizeTimestamp(value);
  if (millis == null) return "-";
  const diff = Date.now() - millis;
  if (!Number.isFinite(diff) || diff < 0) return formatTime(millis);
  if (diff < 60_000) return tx("just now");
  if (diff < 3_600_000) return tx("{{count}}m ago", { count: Math.max(1, Math.round(diff / 60_000)) });
  if (diff < 86_400_000) return tx("{{count}}h ago", { count: Math.max(1, Math.round(diff / 3_600_000)) });
  if (diff < 604_800_000) return tx("{{count}}d ago", { count: Math.max(1, Math.round(diff / 86_400_000)) });
  return formatTime(millis);
}

function formatCountdown(value: number): string | null {
  const millis = normalizeTimestamp(value);
  if (millis == null) return null;
  const diff = millis - Date.now();
  if (!Number.isFinite(diff) || diff <= 0) return null;
  const minutes = Math.max(1, Math.floor(diff / 60_000));
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.floor(minutes / 60);
  const remainingMinutes = minutes % 60;
  if (hours < 24) return remainingMinutes ? `${hours}h${remainingMinutes}m` : `${hours}h`;
  const days = Math.floor(hours / 24);
  const remainingHours = hours % 24;
  return remainingHours ? `${days}d${remainingHours}h` : `${days}d`;
}

function formatCompactNumber(value: number): string {
  if (!Number.isFinite(value)) return "-";
  if (Math.abs(value) >= 1_000_000) return `${(value / 1_000_000).toFixed(1)}m`;
  if (Math.abs(value) >= 1_000) return `${(value / 1_000).toFixed(1)}k`;
  return Number.isInteger(value) ? String(value) : value.toFixed(1);
}

function bankedResetCredits(source: unknown): BankedResetCredit[] {
  const record = asRecord(source);
  const rawCredits =
    arrayValue(record?.credits) ??
    arrayValue(record?.remainingCredits) ??
    arrayValue(record?.remaining_credits) ??
    arrayValue(source) ??
    [];
  return rawCredits
    .map((item) => asRecord(item))
    .filter((item): item is Record<string, unknown> => Boolean(item))
    .map((item) => ({
      ...item,
      id: stringValue(item.id),
      status: stringValue(item.status),
      grantedAt: stringValue(item.grantedAt ?? item.granted_at),
      expiresAt: stringValue(item.expiresAt ?? item.expires_at),
      title: stringValue(item.title),
      description: stringValue(item.description),
    }));
}

function nextCreditExpiry(credits: BankedResetCredit[]): string | null {
  const candidates = credits
    .filter((credit) => String(credit.status || "available").toLowerCase() === "available")
    .map((credit) => credit.expiresAt)
    .filter((value): value is string => Boolean(value))
    .map((value) => ({ value, ms: Date.parse(value) }))
    .filter((item) => Number.isFinite(item.ms))
    .sort((left, right) => left.ms - right.ms);
  return candidates[0]?.value || null;
}

function valueAt(source: unknown, keys: string[]): unknown {
  const record = asRecord(source);
  if (!record) return undefined;
  for (const key of keys) {
    if (record[key] !== undefined) return record[key];
  }
  return undefined;
}

function asRecord(value: unknown): Record<string, unknown> | null {
  return value && typeof value === "object" && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : null;
}

function arrayValue(value: unknown): unknown[] | null {
  return Array.isArray(value) ? value : null;
}

function stringValue(value: unknown): string | null {
  return typeof value === "string" && value.trim() ? value : null;
}

function numberValue(value: unknown): number | null {
  if (typeof value === "number" && Number.isFinite(value)) return value;
  if (typeof value === "string" && value.trim()) {
    const parsed = Number(value);
    return Number.isFinite(parsed) ? parsed : null;
  }
  return null;
}
