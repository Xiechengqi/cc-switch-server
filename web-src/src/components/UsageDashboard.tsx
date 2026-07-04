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

type UsageTab = "logs" | "providers" | "models" | "pricing" | "limits";
type RangePreset = "24h" | "7d" | "30d" | "all" | "custom";

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

export function UsageDashboard() {
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

  const filter = useMemo(() => filterFromDraft(filterDraft), [filterDraft]);
  const dataSources = useMemo(() => dataSourceBreakdown(data.sourceLogs), [data.sourceLogs]);

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
    if (!window.confirm(tx("Delete pricing for {{model}}?", { model: modelId }))) return;
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
        <LogsTable logs={data.logs} loading={loading} onDetail={(log) => setDetailId(log.requestId)} />
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
          onDelete={(modelId) => void removePricing(modelId)}
        />
      )}
      {activeTab === "limits" && <ProviderLimitsTable limits={data.limits} loading={loading} />}

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
  function patch(next: Partial<UsageFilterDraft>) {
    onChange({ ...draft, ...next });
  }
  return (
    <section className="usage-filter-panel">
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
      {draft.range === "custom" && (
        <>
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
        </>
      )}
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
    </section>
  );
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
  const cacheRate =
    summary.inputTokens + summary.cacheReadTokens + summary.cacheCreationTokens > 0
      ? (summary.cacheReadTokens /
          (summary.inputTokens + summary.cacheReadTokens + summary.cacheCreationTokens)) *
        100
      : 0;
  return (
    <div className="provider-summary-row">
      <SummaryTile label="Requests" value={loading ? "..." : formatInt(summary.requests)} />
      <SummaryTile label="Success" value={loading ? "..." : `${successRate.toFixed(1)}%`} />
      <SummaryTile label="Tokens" value={loading ? "..." : formatInt(summary.totalTokens)} />
      <SummaryTile label="Cache hit" value={loading ? "..." : `${cacheRate.toFixed(1)}%`} />
      <SummaryTile label="Cost" value={loading ? "..." : formatUsd(summary.totalCostUsd, 4)} />
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
  const maxTokens = Math.max(1, ...trends.map((point) => point.rollup.totalTokens));
  return (
    <section className="usage-trend-panel">
      <div className="section-heading">
        <div className="section-title-row compact-title">
          <Database size={17} />
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
        <div className="usage-trend-bars">
          {trends.map((point) => (
            <div key={`${point.startMs}:${point.endMs}`} className="usage-trend-bar">
              <button
                type="button"
                title={`${formatTime(point.startMs)} - ${formatInt(point.rollup.totalTokens)} ${tx("tokens")} - ${formatUsd(point.rollup.totalCostUsd, 4)}`}
                aria-label={tx("Filter {{time}}", { time: formatTime(point.startMs) })}
                onClick={() => onSelectRange(point)}
                style={{ height: `${Math.max(6, (point.rollup.totalTokens / maxTokens) * 100)}%` }}
              >
                <span>{formatInt(point.rollup.requests)}</span>
              </button>
            </div>
          ))}
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

function LogsTable({
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
    <div className="table-wrap usage-table">
      <table>
        <thead>
          <tr>
            <th>{tx("Time")}</th>
            <th>{tx("App")}</th>
            <th>{tx("Provider")}</th>
            <th>{tx("Model")}</th>
            <th>{tx("Tokens")}</th>
            <th>{tx("Cost")}</th>
            <th>{tx("Latency")}</th>
            <th>{tx("Status")}</th>
            <th>{tx("Source")}</th>
            <th>{tx("Detail")}</th>
          </tr>
        </thead>
        <tbody>
          {logs.length ? (
            logs.map((log) => (
              <tr key={log.requestId}>
                <td>{formatTime(log.createdAtMs)}</td>
                <td>{log.app}</td>
                <td title={log.providerId}>{log.providerName || log.providerId}</td>
                <td title={modelRoute(log)}>{modelRoute(log)}</td>
                <td>
                  <TokenCell log={log} />
                </td>
                <td>{log.totalCostUsd == null ? "-" : formatUsd(log.totalCostUsd, 5)}</td>
                <td>{formatLatency(log)}</td>
                <td>
                  <StatusPill tone={log.statusCode >= 200 && log.statusCode < 300 ? "success" : "danger"}>
                    {log.statusCode}
                  </StatusPill>
                </td>
                <td>{sourceText(log)}</td>
                <td>
                  <button className="icon-button" type="button" title={tx("Request detail")} aria-label={tx("Request detail")} onClick={() => onDetail(log)}>
                    <Eye size={15} />
                  </button>
                </td>
              </tr>
            ))
          ) : (
            <EmptyRow columns={10} label="No request logs" />
          )}
        </tbody>
      </table>
    </div>
  );
}

function ProviderStatsTable({ providers, loading }: { providers: ProviderUsageStats[]; loading: boolean }) {
  const { tx } = useI18n();
  if (loading) return <LoadingBlock label="Loading provider stats" />;
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
                <td title={provider.providerId}>{provider.providerName}</td>
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
                <td title={model.model}>{model.model}</td>
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
      <div className="table-wrap usage-table">
        <table>
        <thead>
          <tr>
              <th>{tx("Model")}</th>
              <th>{tx("Name")}</th>
              <th>{tx("Input /M")}</th>
              <th>{tx("Output /M")}</th>
              <th>{tx("Cache read /M")}</th>
              <th>{tx("Cache write /M")}</th>
              <th>{tx("Actions")}</th>
            </tr>
          </thead>
          <tbody>
            {models.length ? (
              models.map((model) => (
                <tr key={model.modelId}>
                  <td title={model.modelId}>{model.modelId}</td>
                  <td>{model.displayName}</td>
                  <td>{formatPriceString(model.inputCostPerMillion)}</td>
                  <td>{formatPriceString(model.outputCostPerMillion)}</td>
                  <td>{formatPriceString(model.cacheReadCostPerMillion)}</td>
                  <td>{formatPriceString(model.cacheCreationCostPerMillion)}</td>
                  <td>
                    <div className="provider-actions">
                      <button className="icon-button" type="button" title={tx("Edit pricing")} aria-label={tx("Edit pricing")} onClick={() => onEdit(model)}>
                        <Pencil size={15} />
                      </button>
                      <button
                        className="icon-button danger"
                        type="button"
                        title={tx("Delete pricing")}
                        aria-label={tx("Delete pricing")}
                        disabled={busy === `delete:${model.modelId}`}
                        onClick={() => onDelete(model.modelId)}
                      >
                        {busy === `delete:${model.modelId}` ? <Loader2 size={15} /> : <Trash2 size={15} />}
                      </button>
                    </div>
                  </td>
                </tr>
              ))
            ) : (
              <EmptyRow columns={7} label="No pricing models" />
            )}
          </tbody>
        </table>
      </div>
    </section>
  );
}

function ProviderLimitsTable({ limits, loading }: { limits: ProviderLimitStatus[]; loading: boolean }) {
  const { tx } = useI18n();
  if (loading) return <LoadingBlock label="Loading provider limits" />;
  return (
    <div className="table-wrap usage-table">
      <table>
        <thead>
          <tr>
            <th>{tx("Provider")}</th>
            <th>{tx("App")}</th>
            <th>{tx("Daily")}</th>
            <th>{tx("Monthly")}</th>
            <th>{tx("Quota")}</th>
            <th>{tx("Shares")}</th>
            <th>{tx("Warnings")}</th>
            <th>{tx("State")}</th>
          </tr>
        </thead>
        <tbody>
          {limits.length ? (
            limits.map((limit) => (
              <tr key={`${limit.app}:${limit.providerId}`}>
                <td title={limit.providerId}>{limit.providerName}</td>
                <td>{limit.app}</td>
                <td>{limitLine(limit.dailyUsageUsd, limit.dailyLimitUsd)}</td>
                <td>{limitLine(limit.monthlyUsageUsd, limit.monthlyLimitUsd)}</td>
                <td>{quotaLine(limit)}</td>
                <td>{limit.shares.length}</td>
                <td title={limit.warnings.join(", ")}>{limit.warnings.join(", ") || "-"}</td>
                <td>
                  <StatusPill tone={limit.blocked ? "danger" : limit.warnings.length ? "warning" : "success"}>
                    {tx(limit.blocked ? "blocked" : limit.warnings.length ? "warning" : "ok")}
                  </StatusPill>
                </td>
              </tr>
            ))
          ) : (
            <EmptyRow columns={8} label="No provider limits" />
          )}
        </tbody>
      </table>
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

function JsonPreview({ value }: { value: unknown }) {
  return <pre className="json-preview">{JSON.stringify(value, null, 2)}</pre>;
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

function TokenCell({ log }: { log: UsageLog }) {
  const freshInput = freshInputTokens(log);
  const cache = (log.cacheReadTokens || 0) + (log.cacheCreationTokens || 0);
  return (
    <div className="usage-token-cell">
      <strong>{formatInt(log.totalTokens ?? freshInput + (log.outputTokens || 0) + cache)}</strong>
      <span>
        in {formatInt(freshInput)} - out {formatInt(log.outputTokens)} - cache {formatInt(cache)}
      </span>
    </div>
  );
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

function limitLine(usage: number, limit?: number | null): string {
  return `${formatUsd(usage, 4)} / ${limit == null ? "-" : formatUsd(limit, 2)}`;
}

function quotaLine(limit: ProviderLimitStatus): string {
  const quota = limit.accountQuotaPercent == null ? "-" : `${limit.accountQuotaPercent.toFixed(1)}%`;
  const dispatch = limit.quotaDispatchLimitPercent == null ? "-" : `${limit.quotaDispatchLimitPercent.toFixed(1)}%`;
  return `${quota} / ${dispatch}`;
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

function formatInt(value?: number | null): string {
  if (value == null || !Number.isFinite(value)) return "0";
  return Math.trunc(value).toLocaleString();
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
