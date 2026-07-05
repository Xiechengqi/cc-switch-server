import { AccountRecord, AppKind, Provider, ProviderMatrixEntry } from "@/lib/api";

export function appLabel(app: AppKind): string {
  switch (app) {
    case "claude":
      return "Claude";
    case "codex":
      return "Codex";
    case "gemini":
      return "Gemini";
    default:
      return app;
  }
}


export function asRecord(value: unknown): Record<string, unknown> {
  return value && typeof value === "object" && !Array.isArray(value)
    ? { ...(value as Record<string, unknown>) }
    : {};
}

export function getString(value: unknown): string | null {
  return typeof value === "string" && value.trim() ? value : null;
}

function env(provider: Provider): Record<string, unknown> {
  return asRecord(asRecord(provider.settingsConfig).env);
}

export function setting(provider: Provider, keys: string[]): string | null {
  const settings = asRecord(provider.settingsConfig);
  const environment = env(provider);
  for (const key of keys) {
    const direct = getString(settings[key]);
    if (direct) return direct;
    const nested = getString(environment[key]);
    if (nested) return nested;
  }
  return null;
}

export function baseUrlFromProvider(provider: Provider, app: AppKind): string | null {
  const keys =
    app === "claude"
      ? ["ANTHROPIC_BASE_URL", "BASE_URL", "baseUrl", "base_url"]
      : app === "codex"
        ? ["OPENAI_BASE_URL", "CODEX_BASE_URL", "BASE_URL", "baseUrl", "base_url"]
        : ["GOOGLE_GEMINI_BASE_URL", "GEMINI_BASE_URL", "BASE_URL", "baseUrl", "base_url"];
  return setting(provider, keys);
}

export function modelFromProvider(provider: Provider): string | null {
  return setting(provider, ["model", "MODEL"]);
}

export function apiFormatFromProvider(provider: Provider): string | null {
  return (
    getString(provider.meta?.apiFormat) ||
    setting(provider, ["apiFormat", "api_format"])
  );
}

export function accountSummary(account: AccountRecord): string {
  const parts = [
    account.email || account.id,
    account.subscriptionLevel || null,
    account.quotaPercent == null ? null : `${account.quotaPercent.toFixed(1)}%`,
  ].filter(Boolean);
  return parts.join(" · ");
}

export function limitLine(usage: number, limit?: number | null): string {
  if (limit == null) return `${formatUsd(usage)} / -`;
  return `${formatUsd(usage)} / ${formatUsd(limit)}`;
}

export function formatUsd(value: number): string {
  if (!Number.isFinite(value)) return "-";
  if (Math.abs(value) >= 1) return `$${value.toFixed(2)}`;
  return `$${value.toFixed(4)}`;
}

export function apiKeyFromProvider(provider: Provider, entry: ProviderMatrixEntry): string | null {
  return setting(provider, [entry.defaults.key]);
}
