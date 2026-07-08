import type { MouseEvent, ReactNode } from "react";
import { Clock, RefreshCw } from "lucide-react";
import { useTranslation } from "react-i18next";
import { cn } from "@/lib/utils";

export function ProviderInUseTag() {
  const { t } = useTranslation();

  return (
    <span className="inline-flex items-center rounded-md bg-blue-100 px-1.5 py-0.5 text-[10px] font-semibold text-blue-600 dark:bg-blue-900/50 dark:text-blue-400">
      {t("provider.inUse", { defaultValue: "使用中" })}
    </span>
  );
}

interface ProviderQuotaMetaRowProps {
  showInUse?: boolean;
  timeLabel: string;
  loading?: boolean;
  onRefresh: (event: MouseEvent<HTMLButtonElement>) => void;
  refreshTitle: string;
  leading?: ReactNode;
  className?: string;
}

export function ProviderQuotaMetaRow({
  showInUse = false,
  timeLabel,
  loading = false,
  onRefresh,
  refreshTitle,
  leading,
  className,
}: ProviderQuotaMetaRowProps) {
  return (
    <div className={cn("flex items-center gap-2 justify-end", className)}>
      {leading}
      {showInUse ? <ProviderInUseTag /> : null}
      <span className="flex items-center gap-1 text-[10px] text-muted-foreground/70">
        <Clock size={10} />
        {timeLabel}
      </span>
      <button
        type="button"
        onClick={onRefresh}
        disabled={loading}
        className="flex-shrink-0 rounded p-1 text-muted-foreground transition-colors hover:bg-muted disabled:opacity-50"
        title={refreshTitle}
      >
        <RefreshCw size={12} className={loading ? "animate-spin" : ""} />
      </button>
    </div>
  );
}
