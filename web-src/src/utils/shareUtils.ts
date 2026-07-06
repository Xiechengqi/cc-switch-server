import type { ShareRecord, TunnelConfig, TunnelInfo } from "@/lib/api";
import type { Settings } from "@/types";
export type ShareAction = "enable" | "disable" | "delete" | "connectInfo";

export type ShareTunnelRuntimeStatus =
  | "running"
  | "reconnecting"
  | "stopped"
  | "offline"
  | "unknown";

export type ShareDisplayStatus =
  | "not_created"
  | "not_configured"
  | "sharing"
  | "closed"
  | "connecting"
  | "connection_error"
  | "expired"
  | "exhausted";

export function formatShareStatus(status: string): string {
  return status.replace(/_/g, " ");
}

export const UNLIMITED_TOKEN_LIMIT = -1;
export const UNLIMITED_PARALLEL_LIMIT = -1;
export const DEFAULT_PARALLEL_LIMIT = 3;
export const MIN_PARALLEL_LIMIT = 3;

export function isUnlimitedTokenLimit(tokenLimit?: number | null): boolean {
  return tokenLimit === UNLIMITED_TOKEN_LIMIT;
}

export function isUnlimitedParallelLimit(
  parallelLimit?: number | null,
): boolean {
  return parallelLimit === UNLIMITED_PARALLEL_LIMIT;
}

export function formatCompactTokenCount(value?: number | null): string {
  const amount = value ?? 0;
  if (amount >= 1_000_000) {
    return `${(amount / 1_000_000).toFixed(1).replace(/\.0$/, "")}M`;
  }
  if (amount >= 10_000) {
    return `${(amount / 1_000).toFixed(1).replace(/\.0$/, "")}k`;
  }
  return String(amount);
}

export function formatShareTokenUsage(
  share: Pick<ShareRecord, "tokenLimit" | "tokensUsed">,
): string {
  if (isUnlimitedTokenLimit(share.tokenLimit)) {
    return `${formatCompactTokenCount(share.tokensUsed)}/∞`;
  }
  return `${formatCompactTokenCount(share.tokensUsed)}/${formatCompactTokenCount(share.tokenLimit)}`;
}

export function getShareUsageRatio(
  share: Pick<ShareRecord, "tokenLimit" | "tokensUsed">,
): number {
  if (
    !share.tokenLimit ||
    share.tokenLimit <= 0 ||
    isUnlimitedTokenLimit(share.tokenLimit)
  ) {
    return 0;
  }
  return Math.max(0, Math.min(share.tokensUsed / share.tokenLimit, 1));
}

export function isTunnelConfigured(settings?: Settings | null): boolean {
  const config = getTunnelConfigFromSettings(settings);
  return Boolean(config.domain);
}

export function getTunnelConfigFromSettings(
  settings?: Settings | null,
): TunnelConfig {
  return {
    domain: settings?.shareRouterDomain ?? "jptokenswitch.cc",
  };
}

export function buildDefaultShareSubdomain(shareId: string): string {
  return `share-${shareId.slice(0, 8)}`;
}

export function resolveShareTunnelInfo(
  share: Pick<ShareRecord, "id" | "subdomain" | "tunnelUrl">,
  config?: TunnelConfig | null,
): { subdomain: string; tunnelUrl: string } {
  const subdomain = share.subdomain || buildDefaultShareSubdomain(share.id);
  if (share.tunnelUrl) {
    return {
      subdomain,
      tunnelUrl: share.tunnelUrl,
    };
  }
  if (!config?.domain) {
    return {
      subdomain,
      tunnelUrl: "",
    };
  }

  const host = config.domain.split(":")[0] ?? config.domain;
  const isLocal =
    host === "localhost" || host === "127.0.0.1" || host === "0.0.0.0";
  const protocol = isLocal ? "http" : "https";
  return {
    subdomain,
    tunnelUrl: `${protocol}://${subdomain}.${config.domain}`,
  };
}

export function getShareTunnelRuntimeStatus(
  share: Pick<ShareRecord, "status" | "tunnelUrl">,
  tunnelStatus?: TunnelInfo | null,
): ShareTunnelRuntimeStatus {
  if (share.status !== "active") {
    return share.tunnelUrl ? "stopped" : "unknown";
  }
  if (tunnelStatus?.healthy) {
    return "running";
  }
  if (tunnelStatus && !tunnelStatus.healthy) {
    return "reconnecting";
  }
  return "offline";
}

export function getShareDisplayStatus(
  share: Pick<ShareRecord, "status" | "tunnelUrl"> | null | undefined,
  tunnelConfigured: boolean,
  tunnelStatus?: TunnelInfo | null,
): ShareDisplayStatus {
  if (!share) {
    return "not_created";
  }

  if (share.status === "paused") {
    return "closed";
  }
  if (share.status === "expired") {
    return "expired";
  }
  if (share.status === "exhausted") {
    return "exhausted";
  }
  if (!tunnelConfigured) {
    return "not_configured";
  }
  if (share.status !== "active") {
    return "connection_error";
  }
  if (tunnelStatus?.healthy) {
    return "sharing";
  }
  if (tunnelStatus && !tunnelStatus.healthy) {
    return "connecting";
  }
  if (share.tunnelUrl) {
    return "connecting";
  }
  return "connecting";
}

export const PERMANENT_EXPIRES_AT = "2099-12-31T23:59:59Z";

export function isPermanentExpiry(value?: string | null): boolean {
  if (!value) return false;
  const date = new Date(value);
  return !Number.isNaN(date.getTime()) && date.getUTCFullYear() >= 2099;
}

export function permanentExpiresInSecs(): number {
  const target = new Date(PERMANENT_EXPIRES_AT).getTime();
  const now = Date.now();
  return Math.max(1, Math.floor((target - now) / 1000));
}

export function maskSensitive(value?: string | null, visible = 4): string {
  if (!value) return "";
  if (value.length <= visible) return "*".repeat(value.length);
  return `${"*".repeat(Math.max(4, value.length - visible))}${value.slice(-visible)}`;
}

export function formatUtcDateTime(value?: string | number | null): string {
  if (value == null || value === "") return "-";
  const date = typeof value === "number" ? new Date(value) : new Date(value);
  if (Number.isNaN(date.getTime())) return "-";
  const parts = new Intl.DateTimeFormat(undefined, {
    timeZone: "UTC",
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
    hour12: false,
  }).formatToParts(date);
  const pick = (type: string) =>
    parts.find((part) => part.type === type)?.value ?? "00";
  return `${pick("year")}-${pick("month")}-${pick("day")} ${pick("hour")}:${pick("minute")}:${pick("second")} UTC`;
}

export function isShareActionAllowed(
  share: ShareRecord,
  action: ShareAction,
  tunnelConfigured: boolean,
  tunnelStatus?: TunnelInfo | null,
): boolean {
  switch (action) {
    case "enable":
      return tunnelConfigured && (share.status !== "active" || !tunnelStatus);
    case "disable":
      return share.status === "active";
    case "delete":
    case "connectInfo":
      return true;
    default:
      return false;
  }
}
