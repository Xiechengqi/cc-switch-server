import { useTranslation } from "react-i18next";
import { Card, CardContent } from "@/components/ui/card";

interface ShareStatsBarProps {
  totalShares: number;
  activeShares: number;
  runningTunnels: number;
  exhaustedShares: number;
  expiringSoon: number;
}

export function ShareStatsBar({
  totalShares,
  activeShares,
  runningTunnels,
  exhaustedShares,
  expiringSoon,
}: ShareStatsBarProps) {
  const { t } = useTranslation();

  const items = [
    { label: t("share.stats.total"), value: totalShares },
    { label: t("share.stats.active"), value: activeShares },
    { label: t("share.stats.runningTunnels"), value: runningTunnels },
    { label: t("share.stats.exhausted"), value: exhaustedShares },
    { label: t("share.stats.expiringSoon"), value: expiringSoon },
  ];

  return (
    <div className="grid gap-3 sm:grid-cols-2 xl:grid-cols-5">
      {items.map((item) => (
        <Card key={item.label} className="border-border-default/70 bg-card/80">
          <CardContent className="flex items-center justify-between px-4 py-4">
            <div>
              <div className="text-xs uppercase tracking-[0.16em] text-muted-foreground">
                {item.label}
              </div>
              <div className="mt-2 text-2xl font-semibold">{item.value}</div>
            </div>
          </CardContent>
        </Card>
      ))}
    </div>
  );
}
