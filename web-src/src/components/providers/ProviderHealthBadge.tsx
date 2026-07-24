import { cn } from "@/lib/utils";
import { useTranslation } from "react-i18next";
import type { ProviderHealth } from "@/types/proxy";

interface ProviderHealthBadgeProps {
  health: ProviderHealth;
  className?: string;
}

export function ProviderHealthBadge({
  health,
  className,
}: ProviderHealthBadgeProps) {
  const { t } = useTranslation();

  const getStatus = () => {
    if (health.probe_support === "unsupported") {
      return {
        labelKey: "health.unsupported",
        labelFallback: "暂不支持检测",
        color: "bg-muted-foreground/60",
        bgColor: "bg-muted",
        textColor: "text-muted-foreground",
      };
    }
    if (health.status === "healthy") {
      return {
        labelKey: "health.operational",
        labelFallback: "正常",
        color: "bg-emerald-500",
        bgColor: "bg-emerald-500/10",
        textColor: "text-emerald-700 dark:text-emerald-400",
      };
    }
    if (health.status === "degraded") {
      return {
        labelKey: "health.degraded",
        labelFallback: "响应较慢",
        color: "bg-amber-500",
        bgColor: "bg-amber-500/10",
        textColor: "text-amber-700 dark:text-amber-400",
      };
    }
    if (health.confirmation_pending) {
      return {
        labelKey: "health.confirmationPending",
        labelFallback: "等待二次确认",
        color: "bg-amber-500",
        bgColor: "bg-amber-500/10",
        textColor: "text-amber-700 dark:text-amber-400",
      };
    }
    if (health.status === "unhealthy") {
      return {
        labelKey: "health.unavailable",
        labelFallback: "异常",
        color: "bg-red-500",
        bgColor: "bg-red-500/10",
        textColor: "text-red-700 dark:text-red-400",
      };
    }
    return {
      labelKey: "health.unknown",
      labelFallback: "待检测",
      color: "bg-muted-foreground/60",
      bgColor: "bg-muted",
      textColor: "text-muted-foreground",
    };
  };

  const statusConfig = getStatus();
  const label = t(statusConfig.labelKey, {
    defaultValue: statusConfig.labelFallback,
  });

  const details = [
    health.checked_at
      ? t("health.checkedAt", {
          value: new Date(Number(health.checked_at)).toLocaleString(),
          defaultValue: `检测时间：${new Date(Number(health.checked_at)).toLocaleString()}`,
        })
      : t("health.notChecked", { defaultValue: "尚未检测" }),
    health.source
      ? t("health.source", {
          value: health.source,
          defaultValue: `来源：${health.source}`,
        })
      : null,
    health.model
      ? t("health.model", {
          value: health.model,
          defaultValue: `模型：${health.model}`,
        })
      : null,
    health.latency_ms != null
      ? t("health.latency", {
          value: health.latency_ms,
          defaultValue: `延迟：${health.latency_ms} ms`,
        })
      : null,
    health.status_code != null
      ? t("health.statusCode", {
          value: health.status_code,
          defaultValue: `HTTP：${health.status_code}`,
        })
      : null,
    health.last_error,
  ].filter((value): value is string => Boolean(value));

  return (
    <div
      className={cn(
        "inline-flex items-center gap-1.5 px-2 py-1 rounded-full text-xs font-medium",
        statusConfig.bgColor,
        statusConfig.textColor,
        className,
      )}
      title={details.join("\n")}
    >
      <div className={cn("w-2 h-2 rounded-full", statusConfig.color)} />
      <span>{label}</span>
    </div>
  );
}
