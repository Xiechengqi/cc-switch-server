import {
  Activity,
  AlertTriangle,
  BarChart3,
  Coins,
  Filter,
  Loader2,
  RefreshCw,
  RotateCcw,
} from "lucide-react";
import { ReactNode, useCallback, useEffect, useMemo, useState } from "react";

import {
  AppKind,
  backfillUsageCosts,
  deleteModelPricing,
  loadUsageDashboardData,
  ModelPricingEntry,
  ModelUsageStats,
  ProviderLimitStatus,
  ProviderUsageStats,
  saveModelPricing,
  UpdateModelPricingInput,
  UsageLog,
  UsageRollup,
  UsageStatsFilter,
  UsageTrendPoint,
} from "@/lib/api";
import { useI18n } from "@/lib/i18n";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { DataSourceBar, emptyDataSourceSummary, type UsageDataSourceSummary } from "@/components/usage/DataSourceBar";
import { UsageFilterBar, dateTimeInput, usageRangeLabel, type UsageFilterDraft } from "@/components/usage/UsageFilterBar";
import { ProviderLimitsGrid } from "@/components/usage/UsageLimitsGrid";
import { UsageLogsPanel } from "@/components/usage/UsageLogsPanel";
import {
  emptyPricingDraft,
  hasPricingModel,
  PricingDefaultsModal,
  pricingDefaultTemplates,
  pricingDraftFromDefault,
  pricingDraftFromModel,
  pricingInputFromModel,
  PricingModal,
  UsagePricingPanel,
  type PricingDraft,
} from "@/components/usage/UsagePricingPanel";
import { ModelRankingGrid, ProviderRankingGrid } from "@/components/usage/UsageRankingGrid";
import { UsageRequestDetailModal } from "@/components/usage/UsageRequestDetailModal";
import { UsageSummaryGrid } from "@/components/usage/UsageSummaryGrid";
import { UsageTrendPanel } from "@/components/usage/UsageTrendPanel";
import { freshInputTokens } from "@/components/usage/usageDisplay";

export type UsageTab = "logs" | "providers" | "models" | "pricing" | "limits";

export interface UsageInitialFocus {
  app: AppKind;
  providerId: string;
  tab: UsageTab;
  key: number;
}

interface UsageDashboardState {
  summary: UsageRollup;
  trends: UsageTrendPoint[];
  providers: ProviderUsageStats[];
  models: ModelUsageStats[];
  logs: UsageLog[];
  sourceLogs: UsageLog[];
  pricing: ModelPricingEntry[];
  limits: ProviderLimitStatus[];
}

const emptyState: UsageDashboardState = {
  summary: emptyRollup(),
  trends: [],
  providers: [],
  models: [],
  logs: [],
  sourceLogs: [],
  pricing: [],
  limits: [],
};

export function UsageDashboard({ initialFocus }: { initialFocus?: UsageInitialFocus | null }) {
  const { t, tx } = useI18n();
  const [filterDraft, setFilterDraft] = useState<UsageFilterDraft>(defaultFilterDraft());
  const [data, setData] = useState<UsageDashboardState>(emptyState);
  const [activeTab, setActiveTab] = useState<UsageTab>("logs");
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [result, setResult] = useState<string | null>(null);
  const [detailId, setDetailId] = useState<string | null>(null);
  const [pricingDraft, setPricingDraft] = useState<PricingDraft | null>(null);
  const [pricingDefaultsOpen, setPricingDefaultsOpen] = useState(false);
  const [pricingDeleteId, setPricingDeleteId] = useState<string | null>(null);
  const [backfillConfirmOpen, setBackfillConfirmOpen] = useState(false);
  const [pricingDefaultsConfirmOpen, setPricingDefaultsConfirmOpen] = useState(false);

  const filter = useMemo(() => filterFromDraft(filterDraft), [filterDraft]);
  const dataSources = useMemo(() => dataSourceBreakdown(data.sourceLogs), [data.sourceLogs]);
  const visibleLimits = useMemo(
    () => filterProviderLimits(data.limits, filterDraft),
    [data.limits, filterDraft],
  );

  const refresh = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      setData(await loadUsageDashboardData(filter));
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setLoading(false);
    }
  }, [filter]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  useEffect(() => {
    if (!initialFocus?.providerId) return;
    setFilterDraft((draft) => ({
      ...draft,
      app: initialFocus.app,
      providerId: initialFocus.providerId,
      shareId: "",
      userEmail: "",
      sessionId: "",
    }));
    setActiveTab(initialFocus.tab);
  }, [initialFocus?.app, initialFocus?.key, initialFocus?.providerId, initialFocus?.tab]);

  async function runBackfill() {
    setBusy("backfill");
    setError(null);
    try {
      const updated = await backfillUsageCosts();
      setResult(tx("backfilled {{count}} usage records", { count: updated }));
      await refresh();
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setBusy(null);
    }
  }

  async function submitPricing(input: UpdateModelPricingInput) {
    setBusy("pricing");
    setError(null);
    try {
      const saved = await saveModelPricing(input);
      setResult(tx("saved {{model}}; backfilled {{count}}", {
        model: saved.model.modelId,
        count: saved.backfilled,
      }));
      setPricingDraft(null);
      await refresh();
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setBusy(null);
    }
  }

  async function applyPricingTemplate(template: ModelPricingEntry) {
    setBusy(`template:${template.modelId}`);
    setError(null);
    try {
      const saved = await saveModelPricing(pricingInputFromModel(template));
      setResult(tx("applied {{model}}; backfilled {{count}}", {
        model: saved.model.modelId,
        count: saved.backfilled,
      }));
      await refresh();
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setBusy(null);
    }
  }

  async function applyMissingPricingTemplates() {
    const missing = pricingDefaultTemplates.filter((template) => !hasPricingModel(data.pricing, template.modelId));
    if (!missing.length) {
      setResult(tx("all default pricing models already exist"));
      return;
    }
    setBusy("pricing-defaults");
    setError(null);
    try {
      let backfilled = 0;
      for (const template of missing) {
        const saved = await saveModelPricing(pricingInputFromModel(template));
        backfilled += saved.backfilled;
      }
      setResult(tx("applied {{count}} default pricing models; backfilled {{backfilled}}", {
        count: missing.length,
        backfilled,
      }));
      await refresh();
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setBusy(null);
    }
  }

  async function removePricing(modelId: string) {
    setBusy(`delete:${modelId}`);
    setError(null);
    try {
      const deleted = await deleteModelPricing(modelId);
      setResult(deleted ? tx("deleted {{model}}", { model: modelId }) : tx("{{model}} was not found", { model: modelId }));
      await refresh();
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setBusy(null);
    }
  }

  return (
    <div className="usage-dashboard">
      <div className="provider-toolbar">
        <div className="provider-toolbar-status">
          <span>{tx(usageRangeLabel(filterDraft))}</span>
          <span>{t("server.usage.logsLoaded", { count: data.logs.length })}</span>
        </div>
        <div className="provider-toolbar-actions">
          {error && <span className="error-text">{error}</span>}
          {result && <span className="usage-result">{result}</span>}
          <button className="secondary-button" type="button" onClick={() => void refresh()}>
            <RefreshCw size={15} />
            <span>{t("common.refresh")}</span>
          </button>
          <button className="secondary-button" type="button" onClick={() => setBackfillConfirmOpen(true)} disabled={busy === "backfill"}>
            {busy === "backfill" ? <Loader2 size={15} /> : <RotateCcw size={15} />}
            <span>{t("server.usage.backfillCosts")}</span>
          </button>
        </div>
      </div>

      <UsageFilterBar draft={filterDraft} onChange={setFilterDraft} />

      <DataSourceBar
        sources={dataSources}
        loading={loading}
        activeSource={filterDraft.dataSource.trim()}
        onSelect={(dataSource) => setFilterDraft((draft) => ({ ...draft, dataSource }))}
      />

      <UsageSummaryGrid summary={data.summary} loading={loading} />

      <UsageTrendPanel
        trends={data.trends}
        loading={loading}
        onSelectRange={(point) =>
          setFilterDraft((draft) => ({
            ...draft,
            range: "custom",
            customFrom: dateTimeInput(point.startMs),
            customTo: dateTimeInput(point.endMs),
          }))
        }
      />

      <div className="usage-tabs" role="tablist" aria-label={t("server.usage.views")}>
        <TabButton id="logs" active={activeTab} onClick={setActiveTab} icon={<Filter size={15} />}>
          {t("server.usage.logs")}
        </TabButton>
        <TabButton id="providers" active={activeTab} onClick={setActiveTab} icon={<Activity size={15} />}>
          {t("server.usage.providers")}
        </TabButton>
        <TabButton id="models" active={activeTab} onClick={setActiveTab} icon={<BarChart3 size={15} />}>
          {t("server.usage.models")}
        </TabButton>
        <TabButton id="pricing" active={activeTab} onClick={setActiveTab} icon={<Coins size={15} />}>
          {t("server.usage.pricing")}
        </TabButton>
        <TabButton id="limits" active={activeTab} onClick={setActiveTab} icon={<AlertTriangle size={15} />}>
          {t("server.usage.limits")}
        </TabButton>
      </div>

      {activeTab === "logs" && (
        <UsageLogsPanel logs={data.logs} loading={loading} onDetail={(log) => setDetailId(log.requestId)} />
      )}
      {activeTab === "providers" && <ProviderRankingGrid providers={data.providers} loading={loading} />}
      {activeTab === "models" && <ModelRankingGrid models={data.models} loading={loading} />}
      {activeTab === "pricing" && (
        <UsagePricingPanel
          models={data.pricing}
          busy={busy}
          onAdd={() => setPricingDraft(emptyPricingDraft())}
          onDefaults={() => setPricingDefaultsOpen(true)}
          onEdit={(model) => setPricingDraft(pricingDraftFromModel(model))}
          onDelete={setPricingDeleteId}
        />
      )}
      {activeTab === "limits" && <ProviderLimitsGrid limits={visibleLimits} loading={loading} />}

      {detailId && <UsageRequestDetailModal requestId={detailId} onClose={() => setDetailId(null)} />}

      {pricingDraft && (
        <PricingModal
          draft={pricingDraft}
          saving={busy === "pricing"}
          onChange={setPricingDraft}
          onClose={() => setPricingDraft(null)}
          onSubmit={(input) => void submitPricing(input)}
        />
      )}

      {pricingDefaultsOpen && (
        <PricingDefaultsModal
          models={data.pricing}
          busy={busy}
          onApply={(template) => void applyPricingTemplate(template)}
          onApplyMissing={() => setPricingDefaultsConfirmOpen(true)}
          onEdit={(template) => {
            setPricingDefaultsOpen(false);
            setPricingDraft(pricingDraftFromDefault(template, hasPricingModel(data.pricing, template.modelId)));
          }}
          onClose={() => setPricingDefaultsOpen(false)}
        />
      )}
      <ConfirmDialog
        isOpen={backfillConfirmOpen}
        title={tx("Backfill usage costs")}
        message={tx("Recalculate costs for existing usage records using current pricing rules? Historical cost values may change.")}
        confirmText={tx("Backfill")}
        onConfirm={() => {
          setBackfillConfirmOpen(false);
          void runBackfill();
        }}
        onCancel={() => setBackfillConfirmOpen(false)}
      />
      <ConfirmDialog
        isOpen={pricingDefaultsConfirmOpen}
        title={tx("Apply default pricing")}
        message={tx("Apply missing default pricing templates? Existing usage records may be backfilled with new costs.")}
        confirmText={tx("Apply Missing")}
        onConfirm={() => {
          setPricingDefaultsConfirmOpen(false);
          void applyMissingPricingTemplates();
        }}
        onCancel={() => setPricingDefaultsConfirmOpen(false)}
      />
      <ConfirmDialog
        isOpen={pricingDeleteId !== null}
        title={tx("Delete pricing")}
        message={tx("Delete pricing for {{model}}?", { model: pricingDeleteId || "-" })}
        confirmText={tx("Delete")}
        onConfirm={() => {
          const modelId = pricingDeleteId;
          setPricingDeleteId(null);
          if (modelId) void removePricing(modelId);
        }}
        onCancel={() => setPricingDeleteId(null)}
      />
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

function filterProviderLimits(limits: ProviderLimitStatus[], draft: UsageFilterDraft): ProviderLimitStatus[] {
  const app = draft.app === "all" ? "" : draft.app;
  const providerId = draft.providerId.trim().toLowerCase();
  if (!app && !providerId) return limits;
  return limits.filter((limit) => {
    if (app && limit.app !== app) return false;
    if (!providerId) return true;
    return [
      limit.providerId,
      limit.providerName,
      limit.providerType,
      limit.accountId,
      limit.accountEmail,
      ...limit.shares.map((share) => `${share.shareId} ${share.shareName} ${share.status}`),
    ]
      .filter(Boolean)
      .join(" ")
      .toLowerCase()
      .includes(providerId);
  });
}

function defaultFilterDraft(): UsageFilterDraft {
  return {
    range: "1d",
    customFrom: "",
    customTo: "",
    app: "all",
    providerId: "",
    shareId: "",
    userEmail: "",
    sessionId: "",
    dataSource: "",
    health: "all",
    streamStatus: "",
    limit: "100",
  };
}

function filterFromDraft(draft: UsageFilterDraft): UsageStatsFilter {
  const bounds = rangeBounds(draft);
  const filter: UsageStatsFilter = {
    ...bounds,
    limit: positiveInt(draft.limit) || 100,
    windowMs: trendWindowMs(bounds),
  };
  if (draft.app !== "all") filter.app = draft.app;
  if (draft.providerId.trim()) filter.providerId = draft.providerId.trim();
  if (draft.shareId.trim()) filter.shareId = draft.shareId.trim();
  if (draft.userEmail.trim()) filter.userEmail = draft.userEmail.trim();
  if (draft.sessionId.trim()) filter.sessionId = draft.sessionId.trim();
  if (draft.dataSource.trim()) filter.dataSource = draft.dataSource.trim();
  if (draft.health !== "all") filter.isHealthCheck = draft.health === "true";
  if (draft.streamStatus.trim()) filter.streamStatus = draft.streamStatus.trim();
  return filter;
}

function rangeBounds(draft: UsageFilterDraft): Pick<UsageStatsFilter, "fromMs" | "toMs"> {
  const now = Date.now();
  if (draft.range === "all") return {};
  if (draft.range === "custom") {
    return {
      fromMs: dateInputToMs(draft.customFrom),
      toMs: dateInputToMs(draft.customTo),
    };
  }
  if (draft.range === "today") {
    const start = new Date();
    start.setHours(0, 0, 0, 0);
    return { fromMs: start.getTime(), toMs: now };
  }
  const days = draft.range === "1d" ? 1 : draft.range === "7d" ? 7 : draft.range === "14d" ? 14 : 30;
  return { fromMs: now - days * 24 * 60 * 60 * 1000, toMs: now };
}

function trendWindowMs(bounds: Pick<UsageStatsFilter, "fromMs" | "toMs">): number {
  const duration = bounds.fromMs && bounds.toMs ? bounds.toMs - bounds.fromMs : 30 * 24 * 60 * 60 * 1000;
  if (duration <= 36 * 60 * 60 * 1000) return 60 * 60 * 1000;
  if (duration <= 10 * 24 * 60 * 60 * 1000) return 6 * 60 * 60 * 1000;
  return 24 * 60 * 60 * 1000;
}

function dateInputToMs(value: string): number | undefined {
  if (!value.trim()) return undefined;
  const parsed = new Date(value).getTime();
  return Number.isFinite(parsed) ? parsed : undefined;
}

function positiveInt(value: string): number | undefined {
  const parsed = Number.parseInt(value, 10);
  return Number.isFinite(parsed) && parsed > 0 ? parsed : undefined;
}

function emptyRollup(): UsageRollup {
  return {
    requests: 0,
    successes: 0,
    failures: 0,
    inputTokens: 0,
    outputTokens: 0,
    cacheReadTokens: 0,
    cacheCreationTokens: 0,
    totalTokens: 0,
    totalCostUsd: 0,
  };
}

function dataSourceBreakdown(logs: UsageLog[]): UsageDataSourceSummary[] {
  const summaries = new Map<string, UsageDataSourceSummary>();
  for (const log of logs) {
    const dataSource = (log.dataSource || "unknown").trim() || "unknown";
    const summary = summaries.get(dataSource) || emptyDataSourceSummary(dataSource);
    summary.requests += 1;
    if (log.statusCode >= 200 && log.statusCode < 300) {
      summary.successes += 1;
    } else {
      summary.failures += 1;
    }
    summary.totalTokens += log.totalTokens ?? freshInputTokens(log) + (log.outputTokens || 0) + (log.cacheReadTokens || 0) + (log.cacheCreationTokens || 0);
    summary.totalCostUsd += log.totalCostUsd || 0;
    if (log.isHealthCheck) summary.healthChecks += 1;
    summaries.set(dataSource, summary);
  }
  return [...summaries.values()].sort((left, right) => right.requests - left.requests || left.dataSource.localeCompare(right.dataSource));
}

function errorMessage(reason: unknown): string {
  return reason instanceof Error ? reason.message : String(reason);
}
