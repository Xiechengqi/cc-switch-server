import { AppKind, ShareBinding, ShareRecord } from "@/lib/api";

const appLabels: Record<AppKind, string> = {
  claude: "Claude",
  codex: "Codex",
  gemini: "Gemini",
};

const appOrder: AppKind[] = ["claude", "codex", "gemini"];

export function shareName(share: ShareRecord): string {
  return share.displayName || share.id;
}

export function appLabel(app: AppKind): string {
  return appLabels[app] || app;
}

export function providerKey(app: AppKind, providerId: string): string {
  return `${app}:${providerId}`;
}

export function shareActionLabel(action: "pause" | "resume" | "startTunnel" | "stopTunnel" | "resetUsage"): string {
  const labels: Record<typeof action, string> = {
    pause: "Pause",
    resume: "Resume",
    startTunnel: "Start tunnel",
    stopTunnel: "Stop tunnel",
    resetUsage: "Reset usage",
  };
  return labels[action];
}

export function shareBindings(share: ShareRecord): ShareBinding[] {
  const seen = new Set<string>();
  const bindings: ShareBinding[] = [];
  for (const binding of share.bindings || []) {
    if (binding.providerId && !seen.has(binding.app)) {
      seen.add(binding.app);
      bindings.push(binding);
    }
  }
  if (share.providerId && !seen.has(share.app)) {
    bindings.unshift({ app: share.app, providerId: share.providerId, providerType: share.providerType });
  }
  return appOrder.flatMap((app) => bindings.filter((binding) => binding.app === app));
}

export function shareUsage(share: ShareRecord): string {
  if (!share.tokenLimit) return `${share.tokensUsed || 0} tokens`;
  return `${share.tokensUsed || 0}/${share.tokenLimit}`;
}

export function shareUsageRatio(share: ShareRecord): number {
  if (!share.tokenLimit) return 0;
  return Math.max(0, Math.min(1, (share.tokensUsed || 0) / share.tokenLimit));
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

export function formatTokens(value?: number | null): string {
  if (value == null) return "-";
  return new Intl.NumberFormat(undefined, { maximumFractionDigits: 0 }).format(value);
}

export function formatUsd(value?: number | null): string {
  if (value == null) return "-";
  if (Math.abs(value) < 0.01 && value !== 0) return `$${value.toFixed(6)}`;
  return `$${value.toFixed(4)}`;
}

export function formatDuration(value?: number | null): string {
  if (value == null) return "-";
  if (value < 1000) return `${Math.round(value)}ms`;
  return `${(value / 1000).toFixed(2)}s`;
}
