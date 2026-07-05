import { inferIconForText } from "@/config/iconInference";
import type { ModelUsageStats, ProviderUsageStats } from "@/lib/api";
import { useI18n } from "@/lib/i18n";
import { KeyValue } from "@/components/KeyValue";
import { LoadingBlock } from "@/components/LoadingBlock";
import { ProviderIcon } from "@/components/ProviderIcon";
import { StatusPill } from "@/components/StatusPill";
import { UsageMiniMetric } from "@/components/usage/UsageMiniMetric";
import { formatInt, formatMaybeMs, formatTime, formatUsd, modelStatsRoute, successRate } from "@/components/usage/usageDisplay";

export function ProviderRankingGrid({ providers, loading }: { providers: ProviderUsageStats[]; loading: boolean }) {
  const { tx } = useI18n();
  if (loading) return <LoadingBlock label="Loading provider stats" />;
  const maxTokens = Math.max(0, ...providers.map((provider) => provider.rollup.totalTokens || 0));
  return (
    <div className="usage-ranking-grid">
      {providers.length ? (
        providers.map((provider, index) => (
          <ProviderRankingCard
            key={`${provider.app}:${provider.providerId}`}
            provider={provider}
            index={index}
            maxTokens={maxTokens}
          />
        ))
      ) : (
        <div className="provider-empty">{tx("No provider stats")}</div>
      )}
    </div>
  );
}

function ProviderRankingCard({
  provider,
  index,
  maxTokens,
}: {
  provider: ProviderUsageStats;
  index: number;
  maxTokens: number;
}) {
  const { tx } = useI18n();
  const icon = inferIconForText(`${provider.providerType} ${provider.providerName}`);
  return (
    <article className="usage-ranking-card">
      <header>
        <div className="usage-ranking-title">
          <span className="provider-icon-frame">
            <ProviderIcon
              icon={icon.icon}
              color={icon.iconColor}
              name={provider.providerName || provider.providerId}
              size={22}
            />
          </span>
          <div>
            <strong title={provider.providerId}>{provider.providerName || provider.providerId}</strong>
            <span>{`${provider.providerType} / ${provider.providerId}`}</span>
          </div>
        </div>
        <StatusPill tone={index === 0 ? "success" : "warning"}>{tx("Rank {{rank}}", { rank: index + 1 })}</StatusPill>
      </header>
      <UsageRankCell
        label={tx("Token share")}
        subtitle={tx("Total tokens {{tokens}}", { tokens: formatInt(provider.rollup.totalTokens) })}
        tokens={provider.rollup.totalTokens}
        maxTokens={maxTokens}
      />
      <div className="usage-log-metrics">
        <UsageMiniMetric label="requests" value={formatInt(provider.rollup.requests)} detail={tx(provider.app)} />
        <UsageMiniMetric label="success" value={successRate(provider.rollup)} detail={tx("success rate")} />
        <UsageMiniMetric label="cost" value={formatUsd(provider.rollup.totalCostUsd, 4)} detail={provider.providerType || "-"} />
        <UsageMiniMetric label="avg latency" value={formatMaybeMs(provider.avgDurationMs)} detail={tx("first token {{value}}", { value: formatMaybeMs(provider.avgFirstTokenMs) })} />
      </div>
      <footer>
        <KeyValue label="last request" value={formatTime(provider.lastRequestAtMs)} />
      </footer>
    </article>
  );
}

export function ModelRankingGrid({ models, loading }: { models: ModelUsageStats[]; loading: boolean }) {
  const { tx } = useI18n();
  if (loading) return <LoadingBlock label="Loading model stats" />;
  const maxTokens = Math.max(0, ...models.map((model) => model.rollup.totalTokens || 0));
  return (
    <div className="usage-ranking-grid">
      {models.length ? (
        models.map((model, index) => (
          <ModelRankingCard
            key={`${model.app}:${model.model}:${model.pricingModel || ""}`}
            model={model}
            index={index}
            maxTokens={maxTokens}
          />
        ))
      ) : (
        <div className="provider-empty">{tx("No model stats")}</div>
      )}
    </div>
  );
}

function ModelRankingCard({
  model,
  index,
  maxTokens,
}: {
  model: ModelUsageStats;
  index: number;
  maxTokens: number;
}) {
  const { tx } = useI18n();
  const icon = inferIconForText(`${model.app} ${model.model} ${model.pricingModel || ""}`);
  return (
    <article className="usage-ranking-card">
      <header>
        <div className="usage-ranking-title">
          <span className="provider-icon-frame">
            <ProviderIcon
              icon={icon.icon}
              color={icon.iconColor}
              name={model.model}
              size={22}
            />
          </span>
          <div>
            <strong title={model.model}>{model.model}</strong>
            <span title={modelStatsRoute(model)}>{modelStatsRoute(model)}</span>
          </div>
        </div>
        <StatusPill tone={index === 0 ? "success" : "warning"}>{tx("Rank {{rank}}", { rank: index + 1 })}</StatusPill>
      </header>
      <UsageRankCell
        label={tx("Token share")}
        subtitle={tx("Total tokens {{tokens}}", { tokens: formatInt(model.rollup.totalTokens) })}
        tokens={model.rollup.totalTokens}
        maxTokens={maxTokens}
      />
      <div className="usage-log-metrics">
        <UsageMiniMetric label="requests" value={formatInt(model.rollup.requests)} detail={tx(model.app)} />
        <UsageMiniMetric label="tokens" value={formatInt(model.rollup.totalTokens)} detail={tx("total tokens")} />
        <UsageMiniMetric label="cost" value={formatUsd(model.rollup.totalCostUsd, 4)} detail={model.pricingModel || "-"} />
        <UsageMiniMetric
          label="avg/request"
          value={formatUsd(model.rollup.requests ? model.rollup.totalCostUsd / model.rollup.requests : 0, 6)}
          detail={tx("cost per request")}
        />
      </div>
      <footer>
        <KeyValue label="last request" value={formatTime(model.lastRequestAtMs)} />
      </footer>
    </article>
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
