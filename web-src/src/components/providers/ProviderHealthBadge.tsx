import { cn } from "@/lib/utils";
import { useTranslation } from "react-i18next";

interface ProviderHealthBadgeProps {
  failureCount: number;
  isHealthy?: boolean;
  className?: string;
}

/**
 * 供应商健康状态徽章
 * 根据连续失败次数显示不同颜色的状态指示器
 */
export function ProviderHealthBadge({
  failureCount,
  isHealthy,
  className,
}: ProviderHealthBadgeProps) {
  const { t } = useTranslation();

  // 根据失败次数计算状态
  const getStatus = () => {
    if (failureCount === 0) {
      return {
        labelKey: "health.operational",
        labelFallback: "正常",
        color: "bg-green-500",
        // 使用更深/柔和的背景色，去除可能的白色内容感
        bgColor: "bg-green-500/10",
        textColor: "text-green-600 dark:text-green-400",
      };
    } else if (isHealthy !== false) {
      return {
        labelKey: "health.degraded",
        labelFallback: "降级",
        color: "bg-yellow-500",
        bgColor: "bg-yellow-500/10",
        textColor: "text-yellow-600 dark:text-yellow-400",
      };
    } else {
      return {
        labelKey: "health.unavailable",
        labelFallback: "异常",
        color: "bg-red-500",
        bgColor: "bg-red-500/10",
        textColor: "text-red-600 dark:text-red-400",
      };
    }
  };

  const statusConfig = getStatus();
  const label = t(statusConfig.labelKey, {
    defaultValue: statusConfig.labelFallback,
  });

  return (
    <div
      className={cn(
        "inline-flex items-center gap-1.5 px-2 py-1 rounded-full text-xs font-medium",
        statusConfig.bgColor,
        statusConfig.textColor,
        className,
      )}
      title={t("health.failedRequests", {
        count: failureCount,
        defaultValue: `失败请求 ${failureCount} 次`,
      })}
    >
      <div className={cn("w-2 h-2 rounded-full", statusConfig.color)} />
      <span>{label}</span>
    </div>
  );
}
