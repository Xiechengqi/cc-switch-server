import { Activity, AlertTriangle, BarChart3, Coins, Filter } from "lucide-react";
import type { ReactNode } from "react";

import { useI18n } from "@/lib/i18n";

export type UsageTab = "logs" | "providers" | "models" | "pricing" | "limits";

export function UsageTabs({ active, onChange }: { active: UsageTab; onChange: (tab: UsageTab) => void }) {
  const { t } = useI18n();
  return (
    <div className="usage-tabs" role="tablist" aria-label={t("server.usage.views")}>
      <TabButton id="logs" active={active} onClick={onChange} icon={<Filter size={15} />}>
        {t("server.usage.logs")}
      </TabButton>
      <TabButton id="providers" active={active} onClick={onChange} icon={<Activity size={15} />}>
        {t("server.usage.providers")}
      </TabButton>
      <TabButton id="models" active={active} onClick={onChange} icon={<BarChart3 size={15} />}>
        {t("server.usage.models")}
      </TabButton>
      <TabButton id="pricing" active={active} onClick={onChange} icon={<Coins size={15} />}>
        {t("server.usage.pricing")}
      </TabButton>
      <TabButton id="limits" active={active} onClick={onChange} icon={<AlertTriangle size={15} />}>
        {t("server.usage.limits")}
      </TabButton>
    </div>
  );
}

function TabButton({
  id,
  active,
  icon,
  children,
  onClick,
}: {
  id: UsageTab;
  active: UsageTab;
  icon: ReactNode;
  children: ReactNode;
  onClick: (tab: UsageTab) => void;
}) {
  return (
    <button className={id === active ? "active" : ""} type="button" onClick={() => onClick(id)}>
      {icon}
      <span>{children}</span>
    </button>
  );
}
