import type { AppKind, FailoverSnapshot, RouterConfigView, RouterStatusResponse } from "@/lib/server-legacy-api";

export const APP_KINDS: AppKind[] = ["claude", "codex", "gemini"];

export interface RouterDraft {
  url: string;
  apiBase: string;
  domain: string;
  region: string;
  sshHost: string;
  sshUser: string;
  custom: boolean;
}

export interface TunnelDraft {
  tunnelSubdomain: string;
  tunnelStatus: string;
}

export interface ProxyDraft {
  url: string;
  clear: boolean;
  followSystemProxy: boolean;
}

export interface EmailDraft {
  email: string;
  code: string;
}

export interface FailoverDraft {
  enabled: boolean;
  providerQueue: string[];
  failureThreshold: string;
  openDurationSeconds: string;
  halfOpenMaxProbes: string;
}

export function routerDraftFrom(router: RouterConfigView): RouterDraft {
  return {
    url: router.url || "",
    apiBase: router.apiBase || "",
    domain: router.domain || "",
    region: router.region || "",
    sshHost: router.sshHost || "",
    sshUser: router.sshUser || "",
    custom: router.custom,
  };
}

export function tunnelDraftFrom(tunnel: { tunnelSubdomain?: string | null; tunnelStatus?: string | null }): TunnelDraft {
  return {
    tunnelSubdomain: tunnel.tunnelSubdomain || "",
    tunnelStatus: tunnel.tunnelStatus || "",
  };
}

export function emptyRouterDraft(): RouterDraft {
  return { url: "", apiBase: "", domain: "", region: "", sshHost: "", sshUser: "", custom: false };
}

export function emptyTunnelDraft(): TunnelDraft {
  return { tunnelSubdomain: "", tunnelStatus: "" };
}

export function emptyProxyDraft(): ProxyDraft {
  return { url: "", clear: false, followSystemProxy: true };
}

export function emptyEmailDraft(): EmailDraft {
  return { email: "", code: "" };
}

export function emptyFailoverDrafts(): Record<AppKind, FailoverDraft> {
  return APP_KINDS.reduce(
    (drafts, app) => {
      drafts[app] = failoverDraftFrom();
      return drafts;
    },
    {} as Record<AppKind, FailoverDraft>,
  );
}

export function failoverDraftsFrom(snapshot: FailoverSnapshot): Record<AppKind, FailoverDraft> {
  return APP_KINDS.reduce(
    (drafts, app) => {
      drafts[app] = failoverDraftFrom(snapshot.apps[app]);
      return drafts;
    },
    {} as Record<AppKind, FailoverDraft>,
  );
}

export function failoverDraftFrom(config?: FailoverSnapshot["apps"][AppKind]): FailoverDraft {
  return {
    enabled: Boolean(config?.enabled),
    providerQueue: [...(config?.providerQueue || [])],
    failureThreshold: String(config?.failureThreshold ?? 2),
    openDurationSeconds: String(Math.max(1, Math.round((config?.openDurationMs ?? 300000) / 1000))),
    halfOpenMaxProbes: String(config?.halfOpenMaxProbes ?? 1),
  };
}

export function positiveInteger(value: string, fallback: number): number {
  const parsed = Number.parseInt(value, 10);
  if (!Number.isFinite(parsed) || parsed < 1) return fallback;
  return parsed;
}

export function appLabel(app: AppKind): string {
  if (app === "claude") return "Claude Code";
  if (app === "codex") return "Codex";
  return "Gemini";
}

export function routerState(status?: RouterStatusResponse): string {
  if (!status) return "loading";
  if (status.lastError) return status.lastError;
  return status.registered ? "registered" : "not registered";
}

export function routerStatusText(status: RouterStatusResponse): string {
  return status.registered ? `heartbeat ok; pending logs ${status.pendingRequestLogSync}` : "heartbeat recorded locally";
}

export function isClientTunnelRunning(status?: string | null): boolean {
  const normalized = status?.trim().toLowerCase();
  return Boolean(normalized && !["stopped", "ended", "error", "failed"].includes(normalized));
}

export function formatTime(value?: number | null): string {
  if (value == null || value <= 0) return "-";
  return new Date(value).toLocaleString();
}

export function errorMessage(reason: unknown): string {
  return reason instanceof Error ? reason.message : String(reason);
}
