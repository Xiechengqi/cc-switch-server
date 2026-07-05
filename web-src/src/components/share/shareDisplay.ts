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
