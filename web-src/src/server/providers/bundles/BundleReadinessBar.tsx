import { useTranslation } from "react-i18next";

import { cn } from "@/lib/utils";
import type { CoreProviderApp } from "@/server/providerRegistry";
import { APP_LABELS, AppLogo } from "./bundleApps";
import type { BundleGap, BundleReadiness } from "./bundleReadiness";

export type ReadinessTone = "ready" | "gap" | "off";

export function readinessTone(surface: {
  enabled: boolean;
  gap: BundleGap | null;
}): ReadinessTone {
  if (!surface.enabled) return "off";
  return surface.gap ? "gap" : "ready";
}

export function StatusDot({
  tone,
  className,
}: {
  tone: ReadinessTone;
  className?: string;
}) {
  return (
    <span
      aria-hidden
      className={cn(
        "h-1.5 w-1.5 shrink-0 rounded-full",
        tone === "ready" && "bg-emerald-500",
        tone === "gap" && "bg-destructive",
        tone === "off" && "bg-muted-foreground/40",
        className,
      )}
    />
  );
}

export function useGapLabel() {
  const { t } = useTranslation();
  return (gap: BundleGap): string => {
    switch (gap) {
      case "account":
        return t("providerBundle.gapAccount", { defaultValue: "缺账号" });
      case "endpoint":
        return t("providerBundle.gapEndpoint", { defaultValue: "缺地址" });
      case "credential":
        return t("providerBundle.gapCredential", { defaultValue: "缺凭据" });
      default:
        return t("providerBundle.gapModel", { defaultValue: "缺模型" });
    }
  };
}

const TONE_CLASS: Record<ReadinessTone, string> = {
  ready:
    "border-emerald-500/30 bg-emerald-500/10 text-emerald-700 dark:text-emerald-300",
  gap: "border-destructive/30 bg-destructive/10 text-destructive",
  off: "border-border bg-muted/40 text-muted-foreground",
};

/**
 * The one line that answers "can I save this yet, and if not, where is the hole".
 * Each chip is also the fastest way into the Surface it describes — the page is long
 * enough that hunting for the field behind a red border was the slow part.
 */
export function BundleReadinessBar({
  readiness,
  onSelect,
}: {
  readiness: BundleReadiness;
  onSelect: (app: CoreProviderApp) => void;
}) {
  const { t } = useTranslation();
  const gapLabel = useGapLabel();
  if (!readiness.surfaces.length) return null;
  return (
    <div className="flex flex-wrap items-center gap-2">
      <span className="text-xs text-muted-foreground">
        {t("providerBundle.readiness", { defaultValue: "状态" })}
      </span>
      {readiness.surfaces.map((surface) => {
        const tone = readinessTone(surface);
        return (
          <button
            key={surface.app}
            type="button"
            onClick={() => onSelect(surface.app)}
            className={cn(
              "inline-flex h-7 min-w-0 items-center gap-1.5 rounded-full border px-2.5 text-xs transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring",
              TONE_CLASS[tone],
            )}
          >
            <AppLogo app={surface.app} size={13} muted={tone === "off"} />
            <span className="font-medium">{APP_LABELS[surface.app]}</span>
            <span className="truncate opacity-80">
              {tone === "off"
                ? t("providerBundle.surfaceDisabled", {
                    defaultValue: "未启用",
                  })
                : surface.gap
                  ? gapLabel(surface.gap)
                  : t("providerBundle.readinessReady", {
                      defaultValue: "就绪",
                    })}
            </span>
          </button>
        );
      })}
    </div>
  );
}
