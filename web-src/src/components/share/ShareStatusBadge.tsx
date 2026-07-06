import { useTranslation } from "react-i18next";
import { Badge } from "@/components/ui/badge";
import { cn } from "@/lib/utils";

interface ShareStatusBadgeProps {
  status?: string | null;
  kind?: "share" | "tunnel";
}

const SHARE_STATUS_STYLES: Record<string, string> = {
  active:
    "border-emerald-500/30 bg-emerald-500/10 text-emerald-700 dark:text-emerald-300",
  paused:
    "border-amber-500/30 bg-amber-500/10 text-amber-700 dark:text-amber-300",
  expired: "border-muted bg-muted/50 text-muted-foreground",
  exhausted: "border-red-500/30 bg-red-500/10 text-red-700 dark:text-red-300",
  running:
    "border-emerald-500/30 bg-emerald-500/10 text-emerald-700 dark:text-emerald-300",
  reconnecting:
    "border-amber-500/30 bg-amber-500/10 text-amber-700 dark:text-amber-300",
  stopped: "border-muted bg-muted/50 text-muted-foreground",
  offline: "border-red-500/30 bg-red-500/10 text-red-700 dark:text-red-300",
  unknown: "border-muted bg-muted/50 text-muted-foreground",
};

export function ShareStatusBadge({
  status,
  kind = "share",
}: ShareStatusBadgeProps) {
  const { t } = useTranslation();
  const normalized = (status || (kind === "tunnel" ? "unknown" : "active"))
    .toLowerCase()
    .trim();

  return (
    <Badge
      variant="outline"
      className={cn(
        "capitalize rounded-full px-2.5 py-1 text-[11px] font-medium",
        SHARE_STATUS_STYLES[normalized] ?? SHARE_STATUS_STYLES.unknown,
      )}
    >
      {t(`share.statuses.${normalized}`, {
        defaultValue: normalized.replace(/_/g, " "),
      })}
    </Badge>
  );
}
