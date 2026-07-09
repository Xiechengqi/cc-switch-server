import { useTranslation } from "react-i18next";
import type { ShareRecord } from "@/lib/api";
import { cn } from "@/lib/utils";
import {
  getProviderCardShareDisplayStatus,
  type ShareDisplayStatus,
} from "@/utils/shareUtils";

const COMPACT_STATUS_STYLES: Record<ShareDisplayStatus, string> = {
  not_created: "bg-muted text-muted-foreground",
  not_configured:
    "bg-amber-100 text-amber-700 dark:bg-amber-900/40 dark:text-amber-300",
  sharing:
    "bg-emerald-100 text-emerald-700 dark:bg-emerald-900/40 dark:text-emerald-300",
  closed: "bg-slate-100 text-slate-700 dark:bg-slate-900/40 dark:text-slate-300",
  connecting:
    "bg-amber-100 text-amber-700 dark:bg-amber-900/40 dark:text-amber-300",
  connection_error:
    "bg-red-100 text-red-700 dark:bg-red-900/40 dark:text-red-300",
  expired: "bg-muted text-muted-foreground",
  exhausted: "bg-red-100 text-red-700 dark:bg-red-900/40 dark:text-red-300",
};

interface ProviderShareStatusTagProps {
  share: ShareRecord;
}

export function ProviderShareStatusTag({ share }: ProviderShareStatusTagProps) {
  const { t } = useTranslation();
  const status = getProviderCardShareDisplayStatus(share);

  return (
    <span
      className={cn(
        "inline-flex items-center rounded-md px-1.5 py-0.5 text-[10px] font-semibold",
        COMPACT_STATUS_STYLES[status],
      )}
    >
      {t(`share.displayStatuses.${status}`, {
        defaultValue: status.replace(/_/g, " "),
      })}
    </span>
  );
}
