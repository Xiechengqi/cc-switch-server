import type { ModelUsageStats, UsageLog, UsageRollup } from "@/lib/api";

export function freshInputTokens(log: UsageLog): number {
  const input = log.inputTokens || 0;
  const cacheRead = log.cacheReadTokens || 0;
  if ((log.app === "codex" || log.app === "gemini") && input >= cacheRead) {
    return input - cacheRead;
  }
  return input;
}

export function modelRoute(log: UsageLog): string {
  const requested = log.requestedModel || log.model || "-";
  const actual = log.actualModel || log.model || "-";
  if (requested === actual) return actual;
  return `${requested} -> ${actual}`;
}

export function modelStatsRoute(model: ModelUsageStats): string {
  const requested = model.requestedModel || "-";
  const actual = model.actualModel || model.model;
  const pricing = model.pricingModel && model.pricingModel !== model.model ? ` - pricing ${model.pricingModel}` : "";
  return `${requested} -> ${actual}${pricing}`;
}

export function sourceText(log: UsageLog): string {
  return [log.dataSource, log.shareName || log.shareId, log.userEmail, log.streamStatus]
    .filter(Boolean)
    .join(" - ") || "-";
}

export function successRate(rollup: UsageRollup): string {
  return rollup.requests > 0 ? `${((rollup.successes / rollup.requests) * 100).toFixed(1)}%` : "-";
}

export function formatLatency(log: UsageLog): string {
  const firstToken = log.firstTokenMs == null ? "" : ` - ft ${Math.round(log.firstTokenMs)}ms`;
  return `${Math.round(log.durationMs || 0)}ms${firstToken}`;
}

export function formatMaybeMs(value?: number | null): string {
  return value == null ? "-" : `${Math.round(value)}ms`;
}

export function limitPercent(usage: number, limit?: number | null): number {
  if (!limit || limit <= 0) return 0;
  return (usage / limit) * 100;
}

export function formatTime(value?: number | null): string {
  if (value == null || value <= 0) return "-";
  return new Date(value).toLocaleString();
}

export function formatInt(value?: number | null): string {
  if (value == null || !Number.isFinite(value)) return "0";
  return Math.trunc(value).toLocaleString();
}

export function compactTime(value?: number | null): string {
  if (value == null || value <= 0) return "-";
  return new Date(value).toLocaleDateString(undefined, {
    month: "short",
    day: "numeric",
    hour: "2-digit",
  });
}

export function compactNumber(value: number): string {
  if (!Number.isFinite(value)) return "0";
  if (Math.abs(value) >= 1_000_000) return `${(value / 1_000_000).toFixed(1)}M`;
  if (Math.abs(value) >= 1_000) return `${(value / 1_000).toFixed(1)}K`;
  return `${Math.round(value)}`;
}

export function formatUsd(value: number, digits: number): string {
  if (!Number.isFinite(value)) return "-";
  return `$${value.toFixed(digits)}`;
}
