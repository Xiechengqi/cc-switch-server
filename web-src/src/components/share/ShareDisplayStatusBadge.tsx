import { useTranslation } from "react-i18next";
import { Badge } from "@/components/ui/badge";
import { cn } from "@/lib/utils";
import type { ShareDisplayStatus } from "@/utils/shareUtils";

interface ShareDisplayStatusBadgeProps {
  status: ShareDisplayStatus;
}

const DISPLAY_STATUS_STYLES: Record<ShareDisplayStatus, string> = {
  not_created: "border-muted bg-muted/50 text-muted-foreground",
  not_configured:
    "border-amber-500/30 bg-amber-500/10 text-amber-700 dark:text-amber-300",
  sharing:
    "border-emerald-500/30 bg-emerald-500/10 text-emerald-700 dark:text-emerald-300",
  closed:
    "border-slate-500/30 bg-slate-500/10 text-slate-700 dark:text-slate-300",
  connecting:
    "border-amber-500/30 bg-amber-500/10 text-amber-700 dark:text-amber-300",
  connection_error:
    "border-red-500/30 bg-red-500/10 text-red-700 dark:text-red-300",
  expired: "border-muted bg-muted/50 text-muted-foreground",
  exhausted: "border-red-500/30 bg-red-500/10 text-red-700 dark:text-red-300",
};

export function ShareDisplayStatusBadge({
  status,
}: ShareDisplayStatusBadgeProps) {
  const { t } = useTranslation();

  return (
    <Badge
      variant="outline"
      className={cn(
        "rounded-full px-2.5 py-1 text-[11px] font-medium",
        DISPLAY_STATUS_STYLES[status],
      )}
    >
      {t(`share.displayStatuses.${status}`, {
        defaultValue: status.replace(/_/g, " "),
      })}
    </Badge>
  );
}
