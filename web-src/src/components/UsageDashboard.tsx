import {
  Activity,
  AlertTriangle,
  BarChart3,
  Coins,
  Database,
  Eye,
  FileText,
  Filter,
  Loader2,
  Pencil,
  Plus,
  RefreshCw,
  RotateCcw,
  Save,
  Trash2,
  X,
} from "lucide-react";
import { FormEvent, ReactNode, useCallback, useEffect, useMemo, useState } from "react";

import { inferIconForText } from "@/config/iconInference";
import {
  AppKind,
  backfillUsageCosts,
  deleteModelPricing,
  loadUsageDashboardData,
  loadUsageLogDetail,
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
import { JsonPreview } from "@/components/JsonPreview";
import { ProviderIcon } from "@/components/ProviderIcon";

export type UsageTab = "logs" | "providers" | "models" | "pricing" | "limits";
type RangePreset = "24h" | "7d" | "30d" | "all" | "custom";

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

interface UsageFilterDraft {
  range: RangePreset;
  customFrom: string;
  customTo: string;
  app: "all" | AppKind;
  providerId: string;
  shareId: string;
  userEmail: string;
  sessionId: string;
  dataSource: string;
  health: "all" | "true" | "false";
  streamStatus: string;
  limit: string;
}

interface PricingDraft {
  mode: "create" | "edit";
  modelId: string;
  displayName: string;
  inputCostPerMillion: string;
  outputCostPerMillion: string;
  cacheReadCostPerMillion: string;
  cacheCreationCostPerMillion: string;
}

interface UsageDataSourceSummary {
  dataSource: string;
  requests: number;
  successes: number;
  failures: number;
  totalTokens: number;
  totalCostUsd: number;
  healthChecks: number;
}

const apps: Array<{ id: "all" | AppKind; label: string }> = [
  { id: "all", label: "All" },
  { id: "claude", label: "Claude" },
  { id: "codex", label: "Codex" },
  { id: "gemini", label: "Gemini" },
];

const rangeOptions: Array<{ id: RangePreset; label: string }> = [
  { id: "24h", label: "24h" },
  { id: "7d", label: "7d" },
  { id: "30d", label: "30d" },
  { id: "all", label: "All" },
  { id: "custom", label: "Custom" },
];

const pricingDefaultTemplates: ModelPricingEntry[] = [
  pricingTemplate("claude-sonnet-5", "Claude Sonnet 5", "3", "15", "0.30", "3.75"),
  pricingTemplate("claude-opus-4-8", "Claude Opus 4.8", "5", "25", "0.50", "6.25"),
  pricingTemplate("claude-sonnet-4-6", "Claude Sonnet 4.6", "3", "15", "0.30", "3.75"),
  pricingTemplate("claude-haiku-4-5", "Claude Haiku 4.5", "0.80", "4", "0.08", "1"),
  pricingTemplate("gpt-5-5", "GPT-5.5", "2", "10", "0.20", "2.50"),
  pricingTemplate("gpt-5-5-codex-low", "GPT-5.5 Codex Low", "2", "10", "0.20", "2.50"),
  pricingTemplate("gpt-5-5-codex-medium", "GPT-5.5 Codex Medium", "2", "10", "0.20", "2.50"),
  pricingTemplate("gpt-5-5-codex-high", "GPT-5.5 Codex High", "2", "10", "0.20", "2.50"),
  pricingTemplate("gemini-3-pro", "Gemini 3 Pro", "1.25", "10", "0.31", "1.25"),
  pricingTemplate("gemini-3-flash", "Gemini 3 Flash", "0.30", "2.50", "0.075", "0.30"),
  pricingTemplate("kimi-k2", "Kimi K2", "0.60", "2.50", "0", "0"),
  pricingTemplate("glm-5-2", "GLM 5.2", "0.50", "2", "0", "0"),
  pricingTemplate("deepseek-v4-pro", "DeepSeek V4 Pro", "0.50", "2", "0", "0"),
];

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
        <div className="section-title-row">
          <BarChart3 size={18} />
          <div>
            <h2>{t("server.usage.title")}</h2>
            <span>{tx(rangeLabel(filterDraft))} - {t("server.usage.logsLoaded", { count: data.logs.length })}</span>
          </div>
        </div>
        <div className="provider-toolbar-actions">
          {error && <span className="error-text">{error}</span>}
          {result && <span className="usage-result">{result}</span>}
          <button className="secondary-button" type="button" onClick={() => void refresh()}>
            <RefreshCw size={15} />
            <span>{t("common.refresh")}</span>
          </button>
          <button className="secondary-button" type="button" onClick={() => void runBackfill()} disabled={busy === "backfill"}>
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

      <TrendPanel
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
        <LogsPanel logs={data.logs} loading={loading} onDetail={(log) => setDetailId(log.requestId)} />
      )}
      {activeTab === "providers" && <ProviderStatsTable providers={data.providers} loading={loading} />}
      {activeTab === "models" && <ModelStatsTable models={data.models} loading={loading} />}
      {activeTab === "pricing" && (
        <PricingPanel
          models={data.pricing}
          busy={busy}
          onAdd={() => setPricingDraft(emptyPricingDraft())}
          onDefaults={() => setPricingDefaultsOpen(true)}
          onEdit={(model) => setPricingDraft(pricingDraftFromModel(model))}
          onDelete={setPricingDeleteId}
        />
      )}
      {activeTab === "limits" && <ProviderLimitsTable limits={visibleLimits} loading={loading} />}

      {detailId && <RequestDetailModal requestId={detailId} onClose={() => setDetailId(null)} />}

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
          onApplyMissing={() => void applyMissingPricingTemplates()}
          onEdit={(template) => {
            setPricingDefaultsOpen(false);
            setPricingDraft(pricingDraftFromDefault(template, hasPricingModel(data.pricing, template.modelId)));
          }}
          onClose={() => setPricingDefaultsOpen(false)}
        />
      )}
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

function UsageFilterBar({
  draft,
  onChange,
}: {
  draft: UsageFilterDraft;
  onChange: (draft: UsageFilterDraft) => void;
}) {
  const { tx } = useI18n();
  const advancedCount = usageAdvancedFilterCount(draft);
  function patch(next: Partial<UsageFilterDraft>) {
    onChange({ ...draft, ...next });
  }
  function clearAdvanced() {
    patch({
      providerId: "",
      shareId: "",
      userEmail: "",
      sessionId: "",
      dataSource: "",
      health: "all",
      streamStatus: "",
      limit: "100",
    });
  }
  return (
    <section className="usage-filter-panel">
      <div className="usage-filter-primary">
        <div className="segmented usage-app-segment">
          {apps.map((app) => (
            <button
              key={app.id}
              className={draft.app === app.id ? "active" : ""}
              type="button"
              onClick={() => patch({ app: app.id })}
            >
              {tx(app.label)}
            </button>
          ))}
        </div>
        <div className="segmented usage-range-segment">
          {rangeOptions.map((range) => (
            <button
              key={range.id}
              className={draft.range === range.id ? "active" : ""}
              type="button"
              onClick={() => patch({ range: range.id })}
            >
              {tx(range.label)}
            </button>
          ))}
        </div>
      </div>
      {draft.range === "custom" && (
        <div className="usage-custom-range">
          <label>
            <span>{tx("From")}</span>
            <input
              type="datetime-local"
              value={draft.customFrom}
              onChange={(event) => patch({ customFrom: event.target.value })}
            />
          </label>
          <label>
            <span>{tx("To")}</span>
            <input
              type="datetime-local"
              value={draft.customTo}
              onChange={(event) => patch({ customTo: event.target.value })}
            />
          </label>
        </div>
      )}
      <details className="usage-advanced-filters" open={advancedCount > 0 || undefined}>
        <summary>
          <span>{tx("Advanced filters")}</span>
          <small>{tx("{{count}} active", { count: advancedCount })}</small>
        </summary>
        <div className="usage-advanced-grid">
          <label>
            <span>{tx("Provider ID")}</span>
            <input value={draft.providerId} onChange={(event) => patch({ providerId: event.target.value })} />
          </label>
          <label>
            <span>{tx("Share ID")}</span>
            <input value={draft.shareId} onChange={(event) => patch({ shareId: event.target.value })} />
          </label>
          <label>
            <span>{tx("User email")}</span>
            <input value={draft.userEmail} onChange={(event) => patch({ userEmail: event.target.value })} />
          </label>
          <label>
            <span>{tx("Session ID")}</span>
            <input value={draft.sessionId} onChange={(event) => patch({ sessionId: event.target.value })} />
          </label>
          <label>
            <span>{tx("Data source")}</span>
            <input value={draft.dataSource} onChange={(event) => patch({ dataSource: event.target.value })} />
          </label>
          <label>
            <span>{tx("Health check")}</span>
            <select value={draft.health} onChange={(event) => patch({ health: event.target.value as UsageFilterDraft["health"] })}>
              <option value="all">{tx("all")}</option>
              <option value="true">{tx("yes")}</option>
              <option value="false">{tx("no")}</option>
            </select>
          </label>
          <label>
            <span>{tx("Stream status")}</span>
            <select value={draft.streamStatus} onChange={(event) => patch({ streamStatus: event.target.value })}>
              <option value="">{tx("all")}</option>
              <option value="completed">{tx("completed")}</option>
              <option value="interrupted">{tx("interrupted")}</option>
              <option value="failed">{tx("failed")}</option>
            </select>
          </label>
          <label>
            <span>{tx("Limit")}</span>
            <input value={draft.limit} onChange={(event) => patch({ limit: event.target.value })} />
          </label>
        </div>
        <button className="secondary-button compact" type="button" onClick={clearAdvanced} disabled={advancedCount === 0}>
          {tx("Clear advanced filters")}
        </button>
      </details>
    </section>
  );
}

function usageAdvancedFilterCount(draft: UsageFilterDraft): number {
  return [
    draft.providerId.trim(),
    draft.shareId.trim(),
    draft.userEmail.trim(),
    draft.sessionId.trim(),
    draft.dataSource.trim(),
    draft.health !== "all" ? draft.health : "",
    draft.streamStatus.trim(),
    draft.limit.trim() && draft.limit.trim() !== "100" ? draft.limit.trim() : "",
  ].filter(Boolean).length;
}

function DataSourceBar({
  sources,
  loading,
  activeSource,
  onSelect,
}: {
  sources: UsageDataSourceSummary[];
  loading: boolean;
  activeSource: string;
  onSelect: (dataSource: string) => void;
}) {
  const { tx } = useI18n();
  if (loading) {
    return (
      <section className="usage-data-source-panel">
        <div className="section-title-row compact-title">
          <Database size={17} />
          <h2>{tx("Loaded Sources")}</h2>
        </div>
        <div className="provider-empty inline-empty">
          <Loader2 size={18} />
          <span>{tx("Loading sources")}</span>
        </div>
      </section>
    );
  }

  if (!sources.length) return null;

  const total = sources.reduce<UsageDataSourceSummary>(
    (next, source) => ({
      dataSource: "all",
      requests: next.requests + source.requests,
      successes: next.successes + source.successes,
      failures: next.failures + source.failures,
      totalTokens: next.totalTokens + source.totalTokens,
      totalCostUsd: next.totalCostUsd + source.totalCostUsd,
      healthChecks: next.healthChecks + source.healthChecks,
    }),
    emptyDataSourceSummary("all"),
  );

  return (
    <section className="usage-data-source-panel" aria-label={tx("Loaded usage sources")}>
      <div className="section-title-row compact-title">
        <Database size={17} />
        <h2>{tx("Loaded Sources")}</h2>
      </div>
      <div className="usage-data-source-list">
        <DataSourceChip
          source={total}
          active={!activeSource}
          label="All"
          onClick={() => onSelect("")}
        />
        {sources.map((source) => (
          <DataSourceChip
            key={source.dataSource}
            source={source}
            active={source.dataSource === activeSource}
            label={dataSourceLabel(source.dataSource)}
            onClick={() => onSelect(source.dataSource)}
          />
        ))}
      </div>
    </section>
  );
}

function DataSourceChip({
  source,
  active,
  label,
  onClick,
}: {
  source: UsageDataSourceSummary;
  active: boolean;
  label: string;
  onClick: () => void;
}) {
  const { tx } = useI18n();
  const failureRate = source.requests > 0 ? (source.failures / source.requests) * 100 : 0;
  return (
    <button className={active ? "usage-data-source-chip active" : "usage-data-source-chip"} type="button" onClick={onClick}>
      {source.dataSource === "all" || source.dataSource.includes("session") ? <FileText size={15} /> : <Database size={15} />}
      <span>
        <strong>{tx(label)}</strong>
        <small>{formatInt(source.requests)} req</small>
      </span>
      <span>
        <strong>{formatInt(source.totalTokens)}</strong>
        <small>{formatUsd(source.totalCostUsd, 4)}</small>
      </span>
      <span>
        <strong>{failureRate.toFixed(1)}%</strong>
        <small>{source.healthChecks ? `${formatInt(source.healthChecks)} ${tx("health")}` : tx("fail")}</small>
      </span>
    </button>
  );
}

function UsageSummaryGrid({ summary, loading }: { summary: UsageRollup; loading: boolean }) {
  const successRate = summary.requests > 0 ? (summary.successes / summary.requests) * 100 : 0;
  const failureRate = summary.requests > 0 ? (summary.failures / summary.requests) * 100 : 0;
  const cacheRate =
    summary.inputTokens + summary.cacheReadTokens + summary.cacheCreationTokens > 0
      ? (summary.cacheReadTokens /
          (summary.inputTokens + summary.cacheReadTokens + summary.cacheCreationTokens)) *
        100
      : 0;
  return (
    <div className="usage-summary-grid">
      <UsageMetricCard
        label="requests"
        value={loading ? "..." : formatInt(summary.requests)}
        detail={loading ? "..." : `${formatInt(summary.successes)} ok / ${formatInt(summary.failures)} failed`}
        progress={successRate}
      />
      <UsageMetricCard
        label="success"
        value={loading ? "..." : `${successRate.toFixed(1)}%`}
        detail={loading ? "..." : `${failureRate.toFixed(1)}% fail`}
        progress={successRate}
      />
      <UsageMetricCard
        label="tokens"
        value={loading ? "..." : formatInt(summary.totalTokens)}
        detail={loading ? "..." : `${formatInt(summary.inputTokens)} in / ${formatInt(summary.outputTokens)} out`}
        progress={100}
      />
      <UsageMetricCard
        label="cache hit"
        value={loading ? "..." : `${cacheRate.toFixed(1)}%`}
        detail={loading ? "..." : `${formatInt(summary.cacheReadTokens)} read`}
        progress={cacheRate}
      />
      <UsageMetricCard
        label="cost"
        value={loading ? "..." : formatUsd(summary.totalCostUsd, 4)}
        detail={loading ? "..." : `${formatUsd(summary.requests ? summary.totalCostUsd / summary.requests : 0, 6)} / req`}
        progress={100}
      />
    </div>
  );
}

function UsageMetricCard({
  label,
  value,
  detail,
  progress,
}: {
  label: string;
  value: string;
  detail: string;
  progress: number;
}) {
  const { tx } = useI18n();
  return (
    <div className="usage-metric-card">
      <span>{tx(label)}</span>
      <strong>{value}</strong>
      <small>{detail}</small>
      <div className="usage-metric-progress" aria-hidden="true">
        <span style={{ width: `${Math.max(0, Math.min(100, progress))}%` }} />
      </div>
    </div>
  );
}

function TrendPanel({
  trends,
  loading,
  onSelectRange,
}: {
  trends: UsageTrendPoint[];
  loading: boolean;
  onSelectRange: (point: UsageTrendPoint) => void;
}) {
  const { tx } = useI18n();
  const chartData = useMemo(
    () =>
      trends.map((point) => ({
        key: `${point.startMs}:${point.endMs}`,
        label: compactTime(point.startMs),
        startMs: point.startMs,
        endMs: point.endMs,
        requests: point.rollup.requests,
        tokens: point.rollup.totalTokens,
        cost: Number(point.rollup.totalCostUsd.toFixed(6)),
        successRate: point.rollup.requests ? (point.rollup.successes / point.rollup.requests) * 100 : 0,
      })),
    [trends],
  );
  const maxTokens = Math.max(1, ...chartData.map((point) => point.tokens));
  const maxRequests = Math.max(1, ...chartData.map((point) => point.requests));
  const maxCost = Math.max(0.000001, ...chartData.map((point) => point.cost));
  return (
    <section className="usage-trend-panel">
      <div className="section-heading">
        <div className="section-title-row compact-title">
          <BarChart3 size={17} />
          <h2>{tx("Trend")}</h2>
        </div>
        <span>{tx("{{count}} buckets", { count: trends.length })}</span>
      </div>
      {loading ? (
        <div className="provider-empty">
          <Loader2 size={22} />
          <span>{tx("Loading usage trend")}</span>
        </div>
      ) : trends.length ? (
        <div className="usage-chart-card">
          <div className="usage-chart-legend" aria-hidden="true">
            <span className="tokens">{tx("Tokens")}</span>
            <span className="requests">{tx("Requests")}</span>
            <span className="cost">{tx("Cost")}</span>
          </div>
          <svg className="usage-svg-chart" viewBox="0 0 760 260" role="img" aria-label={tx("Usage trend chart")}>
            {[40, 85, 130, 175, 220].map((y) => (
              <line key={y} className="usage-chart-grid-line" x1="42" x2="742" y1={y} y2={y} />
            ))}
            <text className="usage-chart-axis-label" x="42" y="24">{compactNumber(maxTokens)}</text>
            <text className="usage-chart-axis-label" x="708" y="24">{formatUsd(maxCost, 3)}</text>
            {chartData.map((point, index) => {
              const slot = 700 / Math.max(1, chartData.length);
              const groupX = 46 + index * slot;
              const barWidth = Math.max(3, Math.min(14, slot / 5));
              const tokenHeight = Math.max(2, (point.tokens / maxTokens) * 178);
              const requestHeight = Math.max(2, (point.requests / maxRequests) * 178);
              const costHeight = Math.max(2, (point.cost / maxCost) * 178);
              const labelEvery = Math.max(1, Math.ceil(chartData.length / 8));
              const title = `${formatTime(point.startMs)} - ${formatInt(point.tokens)} ${tx("tokens")} - ${formatUsd(point.cost, 4)}`;
              return (
                <g
                  key={point.key}
                  className="usage-chart-group"
                  role="button"
                  tabIndex={0}
                  aria-label={tx("Filter {{time}}", { time: formatTime(point.startMs) })}
                  onClick={() => {
                    const trend = trends.find((item) => `${item.startMs}:${item.endMs}` === point.key);
                    if (trend) onSelectRange(trend);
                  }}
                  onKeyDown={(event) => {
                    if (event.key !== "Enter" && event.key !== " ") return;
                    event.preventDefault();
                    const trend = trends.find((item) => `${item.startMs}:${item.endMs}` === point.key);
                    if (trend) onSelectRange(trend);
                  }}
                >
                  <title>{title}</title>
                  <rect className="usage-chart-hover-target" x={groupX - 4} y="30" width={Math.max(10, slot)} height="210" rx="6" />
                  <rect className="usage-chart-bar tokens" x={groupX} y={220 - tokenHeight} width={barWidth} height={tokenHeight} rx="3" />
                  <rect className="usage-chart-bar requests" x={groupX + barWidth + 2} y={220 - requestHeight} width={barWidth} height={requestHeight} rx="3" />
                  <rect className="usage-chart-bar cost" x={groupX + (barWidth + 2) * 2} y={220 - costHeight} width={barWidth} height={costHeight} rx="3" />
                  {index % labelEvery === 0 && (
                    <text className="usage-chart-tick-label" x={groupX} y="244">
                      {point.label}
                    </text>
                  )}
                </g>
              );
            })}
          </svg>
        </div>
      ) : (
        <div className="provider-empty">
          <BarChart3 size={22} />
          <span>{tx("No usage trend data")}</span>
        </div>
      )}
    </section>
  );
}

function LogsPanel({
  logs,
  loading,
  onDetail,
}: {
  logs: UsageLog[];
  loading: boolean;
  onDetail: (log: UsageLog) => void;
}) {
  const { tx } = useI18n();
  if (loading) return <LoadingBlock label="Loading request logs" />;
  return (
    <section className="usage-panel-card usage-activity-panel">
      <div className="section-heading">
        <div className="section-title-row compact-title">
          <Filter size={17} />
          <h2>{tx("Request Logs")}</h2>
        </div>
        <span>{tx("{{count}} entries", { count: logs.length })}</span>
      </div>
      {logs.length ? (
        <div className="usage-log-list">
          {logs.map((log) => (
            <UsageLogCard key={log.requestId} log={log} onDetail={onDetail} />
          ))}
        </div>
      ) : (
        <div className="provider-empty">
          <Filter size={22} />
          <span>{tx("No request logs")}</span>
        </div>
      )}
    </section>
  );
}

function UsageLogCard({ log, onDetail }: { log: UsageLog; onDetail: (log: UsageLog) => void }) {
  const { tx } = useI18n();
  const icon = inferIconForText(log.providerType, log.providerName, log.providerId, log.app);
  const ok = log.statusCode >= 200 && log.statusCode < 300;
  const cache = (log.cacheReadTokens || 0) + (log.cacheCreationTokens || 0);
  const totalTokens = log.totalTokens ?? freshInputTokens(log) + (log.outputTokens || 0) + cache;
  const tokenDetail = tx("in {{input}} / out {{output}}", {
    input: formatInt(freshInputTokens(log)),
    output: formatInt(log.outputTokens),
  });
  return (
    <article className="usage-log-card">
      <header>
        <div className="usage-log-title">
          <span className="provider-icon-frame">
            <ProviderIcon
              icon={icon.icon}
              color={icon.iconColor}
              name={log.providerName || log.providerId || log.app}
              size={22}
            />
          </span>
          <div>
            <strong title={log.providerId}>{log.providerName || log.providerId}</strong>
            <span title={modelRoute(log)}>{modelRoute(log)}</span>
          </div>
        </div>
        <div className="usage-log-status">
          <StatusPill tone={ok ? "success" : "danger"}>{log.statusCode}</StatusPill>
          <small>{formatTime(log.createdAtMs)}</small>
        </div>
      </header>
      <div className="usage-log-metrics">
        <UsageMiniMetric label="tokens" value={formatInt(totalTokens)} detail={tokenDetail} />
        <UsageMiniMetric label="cost" value={log.totalCostUsd == null ? "-" : formatUsd(log.totalCostUsd, 5)} detail={log.pricingModel || "-"} />
        <UsageMiniMetric label="latency" value={formatLatency(log)} detail={tx(log.isStreaming ? "streaming" : "non-stream")} />
        <UsageMiniMetric label="source" value={sourceText(log) || "-"} detail={log.sessionId || log.requestAgent || "-"} />
      </div>
      <footer>
        <div className="usage-log-tags">
          <span>{tx(log.app)}</span>
          {log.isHealthCheck && <span>{tx("health")}</span>}
          {log.shareName || log.shareId ? <span>{log.shareName || log.shareId}</span> : null}
          {log.userEmail && <span>{log.userEmail}</span>}
          {log.streamStatus && <span>{tx(log.streamStatus)}</span>}
        </div>
        <button className="secondary-button compact-action" type="button" onClick={() => onDetail(log)}>
          <Eye size={15} />
          <span>{tx("Detail")}</span>
        </button>
      </footer>
    </article>
  );
}

function UsageMiniMetric({ label, value, detail }: { label: string; value: ReactNode; detail: ReactNode }) {
  const { tx } = useI18n();
  return (
    <div className="usage-mini-metric">
      <span>{tx(label)}</span>
      <strong>{value}</strong>
      <small>{detail}</small>
    </div>
  );
}

function ProviderStatsTable({ providers, loading }: { providers: ProviderUsageStats[]; loading: boolean }) {
  const { tx } = useI18n();
  if (loading) return <LoadingBlock label="Loading provider stats" />;
  const maxTokens = Math.max(0, ...providers.map((provider) => provider.rollup.totalTokens || 0));
  return (
    <div className="table-wrap usage-table">
      <table>
        <thead>
          <tr>
            <th>{tx("Provider")}</th>
            <th>{tx("App")}</th>
            <th>{tx("Requests")}</th>
            <th>{tx("Success")}</th>
            <th>{tx("Tokens")}</th>
            <th>{tx("Cost")}</th>
            <th>{tx("Avg latency")}</th>
            <th>{tx("First token")}</th>
            <th>{tx("Last request")}</th>
          </tr>
        </thead>
        <tbody>
          {providers.length ? (
            providers.map((provider) => (
              <tr key={`${provider.app}:${provider.providerId}`}>
                <td title={provider.providerId}>
                  <UsageRankCell
                    label={provider.providerName}
                    subtitle={`${provider.providerType} / ${provider.providerId}`}
                    tokens={provider.rollup.totalTokens}
                    maxTokens={maxTokens}
                  />
                </td>
                <td>{provider.app}</td>
                <td>{formatInt(provider.rollup.requests)}</td>
                <td>{successRate(provider.rollup)}</td>
                <td>{formatInt(provider.rollup.totalTokens)}</td>
                <td>{formatUsd(provider.rollup.totalCostUsd, 4)}</td>
                <td>{formatMaybeMs(provider.avgDurationMs)}</td>
                <td>{formatMaybeMs(provider.avgFirstTokenMs)}</td>
                <td>{formatTime(provider.lastRequestAtMs)}</td>
              </tr>
            ))
          ) : (
            <EmptyRow columns={9} label="No provider stats" />
          )}
        </tbody>
      </table>
    </div>
  );
}

function ModelStatsTable({ models, loading }: { models: ModelUsageStats[]; loading: boolean }) {
  const { tx } = useI18n();
  if (loading) return <LoadingBlock label="Loading model stats" />;
  const maxTokens = Math.max(0, ...models.map((model) => model.rollup.totalTokens || 0));
  return (
    <div className="table-wrap usage-table">
      <table>
        <thead>
          <tr>
            <th>{tx("Model")}</th>
            <th>{tx("App")}</th>
            <th>{tx("Requests")}</th>
            <th>{tx("Tokens")}</th>
            <th>{tx("Cost")}</th>
            <th>{tx("Avg/request")}</th>
            <th>{tx("Route")}</th>
            <th>{tx("Last request")}</th>
          </tr>
        </thead>
        <tbody>
          {models.length ? (
            models.map((model) => (
              <tr key={`${model.app}:${model.model}:${model.pricingModel || ""}`}>
                <td title={model.model}>
                  <UsageRankCell
                    label={model.model}
                    subtitle={modelStatsRoute(model)}
                    tokens={model.rollup.totalTokens}
                    maxTokens={maxTokens}
                  />
                </td>
                <td>{model.app}</td>
                <td>{formatInt(model.rollup.requests)}</td>
                <td>{formatInt(model.rollup.totalTokens)}</td>
                <td>{formatUsd(model.rollup.totalCostUsd, 4)}</td>
                <td>{formatUsd(model.rollup.requests ? model.rollup.totalCostUsd / model.rollup.requests : 0, 6)}</td>
                <td title={modelStatsRoute(model)}>{modelStatsRoute(model)}</td>
                <td>{formatTime(model.lastRequestAtMs)}</td>
              </tr>
            ))
          ) : (
            <EmptyRow columns={8} label="No model stats" />
          )}
        </tbody>
      </table>
    </div>
  );
}

function UsageRankCell({
  label,
  subtitle,
  tokens,
  maxTokens,
}: {
  label: string;
  subtitle: string;
  tokens: number;
  maxTokens: number;
}) {
  const { tx } = useI18n();
  const percent = maxTokens > 0 ? Math.max(4, Math.min(100, (tokens / maxTokens) * 100)) : 0;
  return (
    <div className="usage-rank-cell">
      <div>
        <strong>{label || "-"}</strong>
        <span>{subtitle || "-"}</span>
      </div>
      <div className="usage-rank-meter" aria-label={tx("tokens")}>
        <span style={{ width: `${percent}%` }} />
      </div>
      <small>{tx("{{count}} tokens", { count: formatInt(tokens) })}</small>
    </div>
  );
}

function PricingPanel({
  models,
  busy,
  onAdd,
  onDefaults,
  onEdit,
  onDelete,
}: {
  models: ModelPricingEntry[];
  busy: string | null;
  onAdd: () => void;
  onDefaults: () => void;
  onEdit: (model: ModelPricingEntry) => void;
  onDelete: (modelId: string) => void;
}) {
  const { tx } = useI18n();
  const configuredCacheModels = models.filter(
    (model) => Number(model.cacheReadCostPerMillion) > 0 || Number(model.cacheCreationCostPerMillion) > 0,
  ).length;
  return (
    <section className="usage-panel-card">
      <div className="section-heading">
        <div className="section-title-row compact-title">
          <Coins size={17} />
          <h2>{tx("Model Pricing")}</h2>
        </div>
        <div className="provider-toolbar-actions">
          <button className="secondary-button" type="button" onClick={onDefaults}>
            <Database size={15} />
            <span>{tx("Defaults")}</span>
          </button>
          <button className="primary-button" type="button" onClick={onAdd}>
            <Plus size={15} />
            <span>{tx("Add Pricing")}</span>
          </button>
        </div>
      </div>
      <div className="usage-pricing-summary">
        <UsageMiniMetric label="models" value={formatInt(models.length)} detail={tx("configured")} />
        <UsageMiniMetric label="cache pricing" value={formatInt(configuredCacheModels)} detail={tx("models")} />
        <UsageMiniMetric label="defaults" value={formatInt(pricingDefaultTemplates.length)} detail={tx("templates")} />
      </div>
      {models.length ? (
        <div className="usage-pricing-grid">
          {models.map((model) => (
            <PricingCard
              key={model.modelId}
              model={model}
              deleting={busy === `delete:${model.modelId}`}
              onEdit={onEdit}
              onDelete={onDelete}
            />
          ))}
        </div>
      ) : (
        <div className="provider-empty">
          <Coins size={22} />
          <span>{tx("No pricing models")}</span>
        </div>
      )}
    </section>
  );
}

function PricingCard({
  model,
  deleting,
  onEdit,
  onDelete,
}: {
  model: ModelPricingEntry;
  deleting: boolean;
  onEdit: (model: ModelPricingEntry) => void;
  onDelete: (modelId: string) => void;
}) {
  const { tx } = useI18n();
  const icon = inferIconForText(model.modelId, model.displayName);
  return (
    <article className="usage-pricing-card">
      <header>
        <span className="provider-icon-frame">
          <ProviderIcon icon={icon.icon} color={icon.iconColor} name={model.displayName || model.modelId} size={22} />
        </span>
        <div>
          <strong>{model.displayName || model.modelId}</strong>
          <span title={model.modelId}>{model.modelId}</span>
        </div>
      </header>
      <div className="usage-rate-grid">
        <KeyValue label="input" value={formatPriceString(model.inputCostPerMillion)} />
        <KeyValue label="output" value={formatPriceString(model.outputCostPerMillion)} />
        <KeyValue label="cache read" value={formatPriceString(model.cacheReadCostPerMillion)} />
        <KeyValue label="cache write" value={formatPriceString(model.cacheCreationCostPerMillion)} />
      </div>
      <footer>
        <span>{tx("per million tokens")}</span>
        <div className="provider-actions">
          <button className="icon-button" type="button" title={tx("Edit pricing")} aria-label={tx("Edit pricing")} onClick={() => onEdit(model)}>
            <Pencil size={15} />
          </button>
          <button
            className="icon-button danger"
            type="button"
            title={tx("Delete pricing")}
            aria-label={tx("Delete pricing")}
            disabled={deleting}
            onClick={() => onDelete(model.modelId)}
          >
            {deleting ? <Loader2 size={15} /> : <Trash2 size={15} />}
          </button>
        </div>
      </footer>
    </article>
  );
}

function ProviderLimitsTable({ limits, loading }: { limits: ProviderLimitStatus[]; loading: boolean }) {
  const { tx } = useI18n();
  if (loading) return <LoadingBlock label="Loading provider limits" />;
  return (
    <section className="usage-panel-card">
      <div className="section-heading">
        <div className="section-title-row compact-title">
          <AlertTriangle size={17} />
          <h2>{tx("Provider Limits")}</h2>
        </div>
        <span>{tx("{{count}} providers", { count: limits.length })}</span>
      </div>
      {limits.length ? (
        <div className="usage-limit-grid">
          {limits.map((limit) => (
            <ProviderLimitCard key={`${limit.app}:${limit.providerId}`} limit={limit} />
          ))}
        </div>
      ) : (
        <div className="provider-empty">
          <AlertTriangle size={22} />
          <span>{tx("No provider limits")}</span>
        </div>
      )}
    </section>
  );
}

function ProviderLimitCard({ limit }: { limit: ProviderLimitStatus }) {
  const { tx } = useI18n();
  const icon = inferIconForText(limit.providerType, limit.providerName, limit.providerId, limit.app);
  const stateTone = limit.blocked ? "danger" : limit.warnings.length ? "warning" : "success";
  return (
    <article className="usage-limit-card">
      <header>
        <div className="usage-log-title">
          <span className="provider-icon-frame">
            <ProviderIcon icon={icon.icon} color={icon.iconColor} name={limit.providerName || limit.providerId} size={22} />
          </span>
          <div>
            <strong title={limit.providerId}>{limit.providerName || limit.providerId}</strong>
            <span>{`${limit.providerType} / ${limit.app}`}</span>
          </div>
        </div>
        <StatusPill tone={stateTone}>{tx(limit.blocked ? "blocked" : limit.warnings.length ? "warning" : "ok")}</StatusPill>
      </header>
      <div className="usage-limit-meter-stack">
        <LimitMeter label="daily" usage={formatUsd(limit.dailyUsageUsd, 4)} limit={limit.dailyLimitUsd == null ? "-" : formatUsd(limit.dailyLimitUsd, 2)} percent={limitPercent(limit.dailyUsageUsd, limit.dailyLimitUsd)} exceeded={limit.dailyExceeded} />
        <LimitMeter label="monthly" usage={formatUsd(limit.monthlyUsageUsd, 4)} limit={limit.monthlyLimitUsd == null ? "-" : formatUsd(limit.monthlyLimitUsd, 2)} percent={limitPercent(limit.monthlyUsageUsd, limit.monthlyLimitUsd)} exceeded={limit.monthlyExceeded} />
        <LimitMeter label="quota" usage={limit.accountQuotaPercent == null ? "-" : `${limit.accountQuotaPercent.toFixed(1)}%`} limit={limit.quotaDispatchLimitPercent == null ? "-" : `${limit.quotaDispatchLimitPercent.toFixed(1)}%`} percent={limit.accountQuotaPercent ?? 0} exceeded={limit.quotaDispatchExceeded} />
      </div>
      <div className="usage-limit-meta">
        <KeyValue label="account" value={limit.accountEmail || limit.accountId || "-"} />
        <KeyValue label="shares" value={formatInt(limit.shares.length)} />
        <KeyValue label="quota refreshed" value={formatTime(limit.accountQuotaRefreshedAt)} />
      </div>
      {(limit.warnings.length || limit.accountLastRefreshError) && (
        <div className="usage-log-tags warning-tags">
          {limit.warnings.map((warning) => (
            <span key={warning}>{warning}</span>
          ))}
          {limit.accountLastRefreshError && <span>{limit.accountLastRefreshError}</span>}
        </div>
      )}
      {limit.shares.length > 0 && (
        <div className="usage-share-strip">
          {limit.shares.slice(0, 4).map((share) => (
            <span key={share.shareId} title={share.warnings.join(", ")}>
              {share.shareName || share.shareId}
            </span>
          ))}
          {limit.shares.length > 4 && <span>{tx("+{{count}} more", { count: limit.shares.length - 4 })}</span>}
        </div>
      )}
    </article>
  );
}

function LimitMeter({
  label,
  usage,
  limit,
  percent,
  exceeded,
}: {
  label: string;
  usage: string;
  limit: string;
  percent: number;
  exceeded: boolean;
}) {
  const { tx } = useI18n();
  const width = Math.max(0, Math.min(100, percent));
  return (
    <div className={exceeded ? "usage-limit-meter exceeded" : "usage-limit-meter"}>
      <div>
        <span>{tx(label)}</span>
        <strong>{usage}</strong>
        <small>{tx("limit")}: {limit}</small>
      </div>
      <div className="usage-rank-meter" aria-label={tx(label)}>
        <span style={{ width: `${width}%` }} />
      </div>
    </div>
  );
}

function RequestDetailModal({ requestId, onClose }: { requestId: string; onClose: () => void }) {
  const [log, setLog] = useState<UsageLog | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let active = true;
    setLog(null);
    setError(null);
    loadUsageLogDetail(requestId)
      .then((next) => {
        if (active) setLog(next);
      })
      .catch((reason) => {
        if (active) setError(errorMessage(reason));
      });
    return () => {
      active = false;
    };
  }, [requestId]);

  return (
    <SimpleModal title="Request Detail" subtitle={requestId} onClose={onClose}>
      {error && <div className="form-error">{error}</div>}
      {log ? (
        <div className="modal-form-stack">
          <div className="provider-card-meta">
            <KeyValue label="provider" value={log.providerName || log.providerId} />
            <KeyValue label="app" value={log.app} />
            <KeyValue label="model" value={modelRoute(log)} />
            <KeyValue label="pricing" value={log.pricingModel || "-"} />
            <KeyValue label="status" value={log.statusCode} />
            <KeyValue label="duration" value={formatLatency(log)} />
            <KeyValue label="tokens" value={formatInt(log.totalTokens)} />
            <KeyValue label="cost" value={log.totalCostUsd == null ? "-" : formatUsd(log.totalCostUsd, 6)} />
            <KeyValue label="share" value={log.shareName || log.shareId || "-"} />
            <KeyValue label="user" value={log.userEmail || "-"} />
            <KeyValue label="session" value={log.sessionId || "-"} />
            <KeyValue label="source" value={sourceText(log)} />
          </div>
          <JsonPreview value={log} />
        </div>
      ) : (
        <LoadingBlock label="Loading request detail" />
      )}
    </SimpleModal>
  );
}

function PricingDefaultsModal({
  models,
  busy,
  onApply,
  onApplyMissing,
  onEdit,
  onClose,
}: {
  models: ModelPricingEntry[];
  busy: string | null;
  onApply: (template: ModelPricingEntry) => void;
  onApplyMissing: () => void;
  onEdit: (template: ModelPricingEntry) => void;
  onClose: () => void;
}) {
  const { tx } = useI18n();
  const missingCount = pricingDefaultTemplates.filter((template) => !hasPricingModel(models, template.modelId)).length;
  return (
    <SimpleModal title="Default Pricing" subtitle={`${pricingDefaultTemplates.length} model templates`} onClose={onClose}>
      <div className="modal-form-stack">
        <div className="modal-inline-footer">
          <span className="usage-result">{tx("{{count}} missing", { count: missingCount })}</span>
          <button className="secondary-button" type="button" onClick={onApplyMissing} disabled={!!busy || missingCount === 0}>
            {busy === "pricing-defaults" ? <Loader2 size={15} /> : <RotateCcw size={15} />}
            <span>{tx("Apply Missing")}</span>
          </button>
        </div>
        <div className="pricing-default-grid">
          {pricingDefaultTemplates.map((template) => {
            const exists = hasPricingModel(models, template.modelId);
            return (
              <div key={template.modelId} className="pricing-default-card">
                <header>
                  <div>
                    <strong>{template.displayName}</strong>
                    <span>{template.modelId}</span>
                  </div>
                  <StatusPill tone={exists ? "success" : "warning"}>{tx(exists ? "exists" : "missing")}</StatusPill>
                </header>
                <div className="pricing-default-rates">
                  <KeyValue label="input" value={formatPriceString(template.inputCostPerMillion)} />
                  <KeyValue label="output" value={formatPriceString(template.outputCostPerMillion)} />
                  <KeyValue label="cache read" value={formatPriceString(template.cacheReadCostPerMillion)} />
                  <KeyValue label="cache write" value={formatPriceString(template.cacheCreationCostPerMillion)} />
                </div>
                <footer>
                  <button className="secondary-button" type="button" onClick={() => onEdit(template)}>
                    <Pencil size={15} />
                    <span>{tx("Edit")}</span>
                  </button>
                  <button
                    className="primary-button"
                    type="button"
                    onClick={() => onApply(template)}
                    disabled={!!busy}
                  >
                    {busy === `template:${template.modelId}` ? <Loader2 size={15} /> : <Save size={15} />}
                    <span>{tx("Apply")}</span>
                  </button>
                </footer>
              </div>
            );
          })}
        </div>
      </div>
    </SimpleModal>
  );
}

function PricingModal({
  draft,
  saving,
  onChange,
  onClose,
  onSubmit,
}: {
  draft: PricingDraft;
  saving: boolean;
  onChange: (draft: PricingDraft) => void;
  onClose: () => void;
  onSubmit: (input: UpdateModelPricingInput) => void;
}) {
  const { tx } = useI18n();
  const [error, setError] = useState<string | null>(null);
  function patch(next: Partial<PricingDraft>) {
    onChange({ ...draft, ...next });
  }
  function submit(event: FormEvent) {
    event.preventDefault();
    const validation = validatePricingDraft(draft);
    if (validation) {
      setError(tx(validation));
      return;
    }
    onSubmit({
      modelId: draft.modelId.trim(),
      displayName: draft.displayName.trim(),
      inputCostPerMillion: draft.inputCostPerMillion.trim(),
      outputCostPerMillion: draft.outputCostPerMillion.trim(),
      cacheReadCostPerMillion: draft.cacheReadCostPerMillion.trim(),
      cacheCreationCostPerMillion: draft.cacheCreationCostPerMillion.trim(),
    });
  }
  return (
    <SimpleModal title={draft.mode === "create" ? "Add Pricing" : "Edit Pricing"} subtitle={draft.modelId || "new model"} onClose={onClose}>
      <form className="modal-form-stack" onSubmit={submit}>
        {error && <div className="form-error">{error}</div>}
        <label>
          <span>{tx("Model ID")}</span>
          <input value={draft.modelId} disabled={draft.mode === "edit"} onChange={(event) => patch({ modelId: event.target.value })} />
        </label>
        <label>
          <span>{tx("Display name")}</span>
          <input value={draft.displayName} onChange={(event) => patch({ displayName: event.target.value })} />
        </label>
        <label>
          <span>{tx("Input cost /M")}</span>
          <input inputMode="decimal" value={draft.inputCostPerMillion} onChange={(event) => patch({ inputCostPerMillion: event.target.value })} />
        </label>
        <label>
          <span>{tx("Output cost /M")}</span>
          <input inputMode="decimal" value={draft.outputCostPerMillion} onChange={(event) => patch({ outputCostPerMillion: event.target.value })} />
        </label>
        <label>
          <span>{tx("Cache read cost /M")}</span>
          <input inputMode="decimal" value={draft.cacheReadCostPerMillion} onChange={(event) => patch({ cacheReadCostPerMillion: event.target.value })} />
        </label>
        <label>
          <span>{tx("Cache creation cost /M")}</span>
          <input inputMode="decimal" value={draft.cacheCreationCostPerMillion} onChange={(event) => patch({ cacheCreationCostPerMillion: event.target.value })} />
        </label>
        <footer className="modal-inline-footer">
          <button className="secondary-button" type="button" onClick={onClose}>
            {tx("Cancel")}
          </button>
          <button className="primary-button" type="submit" disabled={saving}>
            {saving ? <Loader2 size={15} /> : <Save size={15} />}
            <span>{tx("Save Pricing")}</span>
          </button>
        </footer>
      </form>
    </SimpleModal>
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

function SummaryTile({ label, value }: { label: string; value: ReactNode }) {
  const { tx } = useI18n();
  return (
    <div className="summary-tile">
      <span>{tx(label)}</span>
      <strong>{value}</strong>
    </div>
  );
}

function KeyValue({ label, value }: { label: string; value: ReactNode }) {
  const { tx } = useI18n();
  return (
    <div className="compact-kv">
      <span>{tx(label)}</span>
      <strong>{value}</strong>
    </div>
  );
}

function StatusPill({
  children,
  tone,
}: {
  children: ReactNode;
  tone: "success" | "warning" | "danger";
}) {
  return <span className={`status-pill ${tone}`}>{children}</span>;
}

function SimpleModal({
  title,
  subtitle,
  children,
  onClose,
}: {
  title: string;
  subtitle?: string;
  children: ReactNode;
  onClose: () => void;
}) {
  const { tx } = useI18n();
  return (
    <div className="modal-backdrop" role="presentation">
      <section className="provider-form-modal simple-modal">
        <header>
          <div>
            <h2>{tx(title)}</h2>
            {subtitle && <p>{tx(subtitle)}</p>}
          </div>
          <button className="icon-button" type="button" onClick={onClose} aria-label={tx("Close")}>
            <X size={15} />
          </button>
        </header>
        <div className="simple-modal-body">{children}</div>
      </section>
    </div>
  );
}

function LoadingBlock({ label }: { label: string }) {
  const { tx } = useI18n();
  return (
    <div className="provider-empty">
      <Loader2 size={22} />
      <span>{tx(label)}</span>
    </div>
  );
}

function EmptyRow({ columns, label }: { columns: number; label: string }) {
  const { tx } = useI18n();
  return (
    <tr>
      <td className="empty-cell" colSpan={columns}>
        {tx(label)}
      </td>
    </tr>
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
    range: "24h",
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
  const days = draft.range === "24h" ? 1 : draft.range === "7d" ? 7 : 30;
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

function dateTimeInput(value: number): string {
  const date = new Date(value);
  if (!Number.isFinite(date.getTime())) return "";
  const offsetMs = date.getTimezoneOffset() * 60 * 1000;
  return new Date(date.getTime() - offsetMs).toISOString().slice(0, 16);
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

function pricingTemplate(
  modelId: string,
  displayName: string,
  inputCostPerMillion: string,
  outputCostPerMillion: string,
  cacheReadCostPerMillion: string,
  cacheCreationCostPerMillion: string,
): ModelPricingEntry {
  return {
    modelId,
    displayName,
    inputCostPerMillion,
    outputCostPerMillion,
    cacheReadCostPerMillion,
    cacheCreationCostPerMillion,
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

function emptyDataSourceSummary(dataSource: string): UsageDataSourceSummary {
  return {
    dataSource,
    requests: 0,
    successes: 0,
    failures: 0,
    totalTokens: 0,
    totalCostUsd: 0,
    healthChecks: 0,
  };
}

function dataSourceLabel(dataSource: string): string {
  const labels: Record<string, string> = {
    codex_db: "Codex DB",
    codex_session: "Codex Session",
    direct: "Direct",
    gemini_session: "Gemini Session",
    health: "Health",
    market: "Market",
    opencode_session: "OpenCode Session",
    proxy: "Proxy",
    session_log: "Session Log",
    share_market: "Share Market",
    unknown: "Unknown",
  };
  return labels[dataSource] || dataSource;
}

function rangeLabel(draft: UsageFilterDraft): string {
  if (draft.range !== "custom") return draft.range;
  return `${draft.customFrom || "start"} -> ${draft.customTo || "now"}`;
}

function freshInputTokens(log: UsageLog): number {
  const input = log.inputTokens || 0;
  const cacheRead = log.cacheReadTokens || 0;
  if ((log.app === "codex" || log.app === "gemini") && input >= cacheRead) {
    return input - cacheRead;
  }
  return input;
}

function modelRoute(log: UsageLog): string {
  const requested = log.requestedModel || log.model || "-";
  const actual = log.actualModel || log.model || "-";
  if (requested === actual) return actual;
  return `${requested} -> ${actual}`;
}

function modelStatsRoute(model: ModelUsageStats): string {
  const requested = model.requestedModel || "-";
  const actual = model.actualModel || model.model;
  const pricing = model.pricingModel && model.pricingModel !== model.model ? ` - pricing ${model.pricingModel}` : "";
  return `${requested} -> ${actual}${pricing}`;
}

function sourceText(log: UsageLog): string {
  return [log.dataSource, log.shareName || log.shareId, log.userEmail, log.streamStatus]
    .filter(Boolean)
    .join(" - ") || "-";
}

function successRate(rollup: UsageRollup): string {
  return rollup.requests > 0 ? `${((rollup.successes / rollup.requests) * 100).toFixed(1)}%` : "-";
}

function limitPercent(usage: number, limit?: number | null): number {
  if (!limit || limit <= 0) return 0;
  return (usage / limit) * 100;
}

function formatLatency(log: UsageLog): string {
  const firstToken = log.firstTokenMs == null ? "" : ` - ft ${Math.round(log.firstTokenMs)}ms`;
  return `${Math.round(log.durationMs || 0)}ms${firstToken}`;
}

function formatMaybeMs(value?: number | null): string {
  return value == null ? "-" : `${Math.round(value)}ms`;
}

function formatTime(value?: number | null): string {
  if (value == null || value <= 0) return "-";
  return new Date(value).toLocaleString();
}

function compactTime(value?: number | null): string {
  if (value == null || value <= 0) return "-";
  return new Date(value).toLocaleDateString(undefined, {
    month: "short",
    day: "numeric",
    hour: "2-digit",
  });
}

function formatInt(value?: number | null): string {
  if (value == null || !Number.isFinite(value)) return "0";
  return Math.trunc(value).toLocaleString();
}

function compactNumber(value: number): string {
  if (!Number.isFinite(value)) return "0";
  if (Math.abs(value) >= 1_000_000) return `${(value / 1_000_000).toFixed(1)}M`;
  if (Math.abs(value) >= 1_000) return `${(value / 1_000).toFixed(1)}K`;
  return `${Math.round(value)}`;
}

function formatUsd(value: number, digits: number): string {
  if (!Number.isFinite(value)) return "-";
  return `$${value.toFixed(digits)}`;
}

function formatPriceString(value: string): string {
  const parsed = Number.parseFloat(value);
  return Number.isFinite(parsed) ? `$${parsed.toFixed(4)}` : `$${value}`;
}

function emptyPricingDraft(): PricingDraft {
  return {
    mode: "create",
    modelId: "",
    displayName: "",
    inputCostPerMillion: "0",
    outputCostPerMillion: "0",
    cacheReadCostPerMillion: "0",
    cacheCreationCostPerMillion: "0",
  };
}

function pricingDraftFromModel(model: ModelPricingEntry): PricingDraft {
  return {
    mode: "edit",
    modelId: model.modelId,
    displayName: model.displayName,
    inputCostPerMillion: model.inputCostPerMillion,
    outputCostPerMillion: model.outputCostPerMillion,
    cacheReadCostPerMillion: model.cacheReadCostPerMillion,
    cacheCreationCostPerMillion: model.cacheCreationCostPerMillion,
  };
}

function pricingDraftFromDefault(model: ModelPricingEntry, exists: boolean): PricingDraft {
  return {
    mode: exists ? "edit" : "create",
    modelId: model.modelId,
    displayName: model.displayName,
    inputCostPerMillion: model.inputCostPerMillion,
    outputCostPerMillion: model.outputCostPerMillion,
    cacheReadCostPerMillion: model.cacheReadCostPerMillion,
    cacheCreationCostPerMillion: model.cacheCreationCostPerMillion,
  };
}

function pricingInputFromModel(model: ModelPricingEntry): UpdateModelPricingInput {
  return {
    modelId: model.modelId,
    displayName: model.displayName,
    inputCostPerMillion: model.inputCostPerMillion,
    outputCostPerMillion: model.outputCostPerMillion,
    cacheReadCostPerMillion: model.cacheReadCostPerMillion,
    cacheCreationCostPerMillion: model.cacheCreationCostPerMillion,
  };
}

function hasPricingModel(models: ModelPricingEntry[], modelId: string): boolean {
  const normalized = modelId.trim().toLowerCase();
  return models.some((model) => model.modelId.trim().toLowerCase() === normalized);
}

function validatePricingDraft(draft: PricingDraft): string | null {
  if (!draft.modelId.trim()) return "model id is required";
  if (!draft.displayName.trim()) return "display name is required";
  const values = [
    draft.inputCostPerMillion,
    draft.outputCostPerMillion,
    draft.cacheReadCostPerMillion,
    draft.cacheCreationCostPerMillion,
  ];
  return values.every(isNonNegativeDecimal) ? null : "prices must be non-negative decimals";
}

function isNonNegativeDecimal(value: string): boolean {
  const trimmed = value.trim();
  if (!/^\d+(?:\.\d+)?$/.test(trimmed)) return false;
  const parsed = Number.parseFloat(trimmed);
  return Number.isFinite(parsed) && parsed >= 0;
}

function errorMessage(reason: unknown): string {
  return reason instanceof Error ? reason.message : String(reason);
}
