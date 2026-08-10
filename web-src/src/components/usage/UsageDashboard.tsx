import { useMemo, useState } from "react";
import { Activity, BarChart3, LayoutGrid, ListFilter, RefreshCw, Share2 } from "lucide-react";
import { useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";

import { ProviderIcon } from "@/components/ProviderIcon";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { useUsageEventBridge } from "@/hooks/useUsageEventBridge";
import { usageKeys, useUsageFacets } from "@/lib/query/usage";
import { getUsageRangePresetLabel, resolveUsageRange } from "@/lib/usageRange";
import { cn } from "@/lib/utils";
import {
  USAGE_APPS,
  type UsageApp,
  type UsageAppFilter,
  type UsageFilters,
  type UsageOutcome,
  type UsageRangeSelection,
  type UsageState,
} from "@/types/usage";
import { getLocaleFromLanguage } from "./format";
import { ModelStatsTable } from "./ModelStatsTable";
import { ProviderStatsTable } from "./ProviderStatsTable";
import { RequestDetailPanel } from "./RequestDetailPanel";
import { RequestLogTable } from "./RequestLogTable";
import { ShareUsageTable } from "./ShareUsageTable";
import { UsageDateRangePicker } from "./UsageDateRangePicker";
import { UsageHero } from "./UsageHero";
import { UsageTrendChart } from "./UsageTrendChart";

const APP_ICONS: Record<UsageApp, string> = { claude: "claude", codex: "openai", gemini: "gemini" };
const ALL = "__all__";
const REFRESH_INTERVALS = [0, 10_000, 30_000, 60_000] as const;

function encoded(value?: string | null) {
  return value ? `v:${value}` : ALL;
}

function decoded(value: string) {
  return value === ALL ? undefined : value.slice(2);
}

export function UsageDashboard() {
  const { t, i18n } = useTranslation();
  const queryClient = useQueryClient();
  const [range, setRange] = useState<UsageRangeSelection>({ preset: "today" });
  const [app, setApp] = useState<UsageAppFilter>("all");
  const [bundleId, setBundleId] = useState<string>();
  const [shareId, setShareId] = useState<string>();
  const [userEmail, setUserEmail] = useState<string>();
  const [modelKey, setModelKey] = useState<string>();
  const [outcome, setOutcome] = useState<UsageOutcome>();
  const [usageState, setUsageState] = useState<UsageState>();
  const [refreshIntervalMs, setRefreshIntervalMs] = useState(30_000);
  const [requestId, setRequestId] = useState<string>();

  useUsageEventBridge();

  const parsedModel = useMemo(() => {
    if (!modelKey) return undefined;
    try {
      const [modelApp, actualModel] = JSON.parse(modelKey) as [UsageApp, string];
      return { app: modelApp, actualModel };
    } catch {
      return undefined;
    }
  }, [modelKey]);
  const filters = useMemo<UsageFilters>(() => ({
    app: app === "all" ? parsedModel?.app : app,
    bundleId,
    shareId,
    userEmail,
    actualModel: parsedModel?.actualModel,
    outcome,
    usageState,
  }), [app, bundleId, outcome, parsedModel, shareId, usageState, userEmail]);

  const facets = useUsageFacets({
    range,
    options: { refetchInterval: refreshIntervalMs || false },
  }).data?.data;
  const locale = getLocaleFromLanguage(i18n.resolvedLanguage || i18n.language || "en");
  const resolvedRange = resolveUsageRange(range);
  const rangeLabel = range.preset === "custom"
    ? `${new Date(resolvedRange.startDate * 1_000).toLocaleString(locale)} - ${new Date(resolvedRange.endDate * 1_000).toLocaleString(locale)}`
    : getUsageRangePresetLabel(range.preset, t);
  const requestTableKey = JSON.stringify([
    range.preset,
    range.customStartDate ?? null,
    range.customEndDate ?? null,
    range.liveEndTime ?? false,
    filters.app ?? null,
    filters.bundleId ?? null,
    filters.shareId ?? null,
    filters.userEmail ?? null,
    filters.actualModel ?? null,
    filters.outcome ?? null,
    filters.usageState ?? null,
  ]);

  const changeApp = (next: UsageAppFilter) => {
    setApp(next);
    if (parsedModel && parsedModel.app !== next) setModelKey(undefined);
  };

  const changeModel = (value: string) => {
    const nextModelKey = decoded(value);
    setModelKey(nextModelKey);
    if (!nextModelKey) return;
    try {
      const [modelApp] = JSON.parse(nextModelKey) as [UsageApp, string];
      setApp(modelApp);
    } catch {
      setModelKey(undefined);
    }
  };

  return (
    <div className="space-y-6 pb-8">
      <header className="flex flex-col gap-4 xl:flex-row xl:items-end xl:justify-between">
        <div>
          <h2 className="text-2xl font-semibold">{t("usage.title", "使用统计")}</h2>
          <p className="mt-1 text-sm text-muted-foreground">{t("usage.subtitle", "查看经 Router Share 进入 Server 的请求、Token 与路由链路。")}</p>
        </div>
        <div className="flex flex-wrap items-center gap-2">
          <div className="flex h-9 items-center rounded-md border bg-background p-1">
            {(["all", ...USAGE_APPS] as UsageAppFilter[]).map((value) => {
              const label = value === "all" ? t("common.all", "全部") : t(`usage.appFilter.${value}`, value);
              return <button key={value} type="button" title={label} aria-label={label} onClick={() => changeApp(value)} className={cn("flex h-7 min-w-8 items-center justify-center rounded px-2", app === value ? "bg-muted text-foreground" : "text-muted-foreground hover:text-foreground")}>
                {value === "all" ? <LayoutGrid className="h-4 w-4" /> : <ProviderIcon icon={APP_ICONS[value]} name={value} size={16} />}
              </button>;
            })}
          </div>

          <FilterSelect value={encoded(bundleId)} onChange={(value) => setBundleId(decoded(value))} placeholder={t("usage.allProviders", "所有供应商")}>
            {(facets?.bundles ?? []).map((item) => <SelectItem key={item.bundleId} value={encoded(item.bundleId)}>{item.providerName}</SelectItem>)}
          </FilterSelect>
          <FilterSelect value={encoded(shareId)} onChange={(value) => setShareId(decoded(value))} placeholder={t("usage.allShares", "所有 Share")}>
            {(facets?.shares ?? []).map((item) => <SelectItem key={item.shareId} value={encoded(item.shareId)}>{item.shareName || item.shareSlug || item.shareId}</SelectItem>)}
          </FilterSelect>
          <FilterSelect value={encoded(userEmail)} onChange={(value) => setUserEmail(decoded(value))} placeholder={t("usage.allUsers", "所有用户")}>
            {(facets?.users ?? []).map((item) => <SelectItem key={item.userEmail} value={encoded(item.userEmail)}>{item.userEmail}</SelectItem>)}
          </FilterSelect>
          <FilterSelect value={modelKey ? encoded(modelKey) : ALL} onChange={changeModel} placeholder={t("usage.allModels", "所有模型")}>
            {(facets?.models ?? []).filter((item) => app === "all" || item.app === app).map((item) => {
              const key = JSON.stringify([item.app, item.actualModel]);
              return <SelectItem key={key} value={encoded(key)}>{item.app} · {item.actualModel}</SelectItem>;
            })}
          </FilterSelect>
          <FilterSelect value={encoded(outcome)} onChange={(value) => setOutcome(decoded(value) as UsageOutcome | undefined)} placeholder={t("usage.allOutcomes", "所有结果")}>
            {(facets?.outcomes ?? []).map((item) => <SelectItem key={item.value} value={encoded(item.value)}>{t(`usage.outcomeValue.${item.value}`, item.value)}</SelectItem>)}
          </FilterSelect>
          <FilterSelect value={encoded(usageState)} onChange={(value) => setUsageState(decoded(value) as UsageState | undefined)} placeholder={t("usage.allUsageStates", "所有 Usage 状态")}>
            {(facets?.usageStates ?? []).map((item) => <SelectItem key={item.value} value={encoded(item.value)}>{t(`usage.usageState.${item.value}`, item.value)}</SelectItem>)}
          </FilterSelect>

          <Select value={String(refreshIntervalMs)} onValueChange={(value) => { setRefreshIntervalMs(Number(value)); void queryClient.invalidateQueries({ queryKey: usageKeys.all }); }}>
            <SelectTrigger className="h-9 w-[92px]" title={t("usage.refreshInterval", "刷新间隔")}><span className="flex items-center gap-2"><RefreshCw className="h-3.5 w-3.5" /><SelectValue /></span></SelectTrigger>
            <SelectContent>{REFRESH_INTERVALS.map((value) => <SelectItem key={value} value={String(value)}>{value === 0 ? t("usage.refreshOff", "关闭") : `${value / 1_000}s`}</SelectItem>)}</SelectContent>
          </Select>
          <UsageDateRangePicker selection={range} triggerLabel={rangeLabel} onApply={setRange} />
        </div>
      </header>

      <UsageHero range={range} filters={filters} refreshIntervalMs={refreshIntervalMs} />
      <UsageTrendChart range={range} filters={filters} rangeLabel={rangeLabel} refreshIntervalMs={refreshIntervalMs} />

      <Tabs defaultValue="requests">
        <TabsList>
          <TabsTrigger value="requests" className="gap-2"><ListFilter className="h-4 w-4" />{t("usage.requestLogs", "请求记录")}</TabsTrigger>
          <TabsTrigger value="providers" className="gap-2"><Activity className="h-4 w-4" />{t("usage.providerStats", "供应商")}</TabsTrigger>
          <TabsTrigger value="models" className="gap-2"><BarChart3 className="h-4 w-4" />{t("usage.modelStats", "模型")}</TabsTrigger>
          <TabsTrigger value="shares" className="gap-2"><Share2 className="h-4 w-4" />{t("usage.shareUsers", "Share / 用户")}</TabsTrigger>
        </TabsList>
        <TabsContent value="requests" className="mt-4"><RequestLogTable key={requestTableKey} range={range} filters={filters} refreshIntervalMs={refreshIntervalMs} onSelect={setRequestId} /></TabsContent>
        <TabsContent value="providers" className="mt-4"><ProviderStatsTable range={range} filters={filters} refreshIntervalMs={refreshIntervalMs} /></TabsContent>
        <TabsContent value="models" className="mt-4"><ModelStatsTable range={range} filters={filters} refreshIntervalMs={refreshIntervalMs} /></TabsContent>
        <TabsContent value="shares" className="mt-4"><ShareUsageTable range={range} filters={filters} refreshIntervalMs={refreshIntervalMs} /></TabsContent>
      </Tabs>

      {requestId && <RequestDetailPanel requestId={requestId} onClose={() => setRequestId(undefined)} />}
    </div>
  );
}

function FilterSelect({ value, onChange, placeholder, children }: { value: string; onChange: (value: string) => void; placeholder: string; children: React.ReactNode }) {
  return <Select value={value} onValueChange={onChange}>
    <SelectTrigger className="h-9 w-[132px]" title={placeholder}><SelectValue /></SelectTrigger>
    <SelectContent className="max-w-[320px]"><SelectItem value={ALL}>{placeholder}</SelectItem>{children}</SelectContent>
  </Select>;
}
