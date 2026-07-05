import { RotateCcw, Upload } from "lucide-react";

import { JsonPreview } from "@/components/JsonPreview";
import { KeyValue } from "@/components/KeyValue";
import { ProviderIcon } from "@/components/ProviderIcon";
import { StatusPill } from "@/components/StatusPill";
import {
  accountProviderIcon,
  bankedResetSummary,
  type BankedResetSummary,
  formatDateish,
  formatTime,
  providerLabel,
} from "@/components/settings/accountDisplay";
import type { AccountImportTemplate, AccountManagerCapability, AccountRecord } from "@/lib/api";
import { useI18n } from "@/lib/i18n";

export function CapabilityPanel({
  providerTypes,
  capabilitiesByType,
  templatesByType,
  onImport,
}: {
  providerTypes: string[];
  capabilitiesByType: Map<string, AccountManagerCapability>;
  templatesByType: Map<string, AccountImportTemplate>;
  onImport: (providerType: string) => void;
}) {
  const { tx } = useI18n();
  return (
    <section className="account-tool-panel auth-readiness-panel">
      <div className="section-heading">
        <h2>{tx("Provider readiness")}</h2>
        <span>{tx("{{count}} account providers", { count: providerTypes.length })}</span>
      </div>
      <div className="auth-readiness-grid">
        {providerTypes.map((providerType) => {
          const capability = capabilitiesByType.get(providerType);
          const template = templatesByType.get(providerType);
          const icon = accountProviderIcon(providerType);
          const statusTone = capability?.supportsRefresh || capability?.supportsImport ? "success" : "warning";
          return (
            <article className="auth-readiness-card" key={providerType}>
              <header>
                <div className="account-provider-title-row">
                  <span className="account-icon-frame compact">
                    <ProviderIcon icon={icon.icon} color={icon.color} name={providerLabel(providerType)} size={18} />
                  </span>
                  <div>
                    <h3>{providerLabel(providerType)}</h3>
                    <span>{tx(template?.credentialKind || "manual")}</span>
                  </div>
                </div>
                <StatusPill tone={statusTone}>
                  {tx(capability?.status || "manual_import_only")}
                </StatusPill>
              </header>
              <div className="auth-center-metrics">
                <KeyValue label="login" value={capability?.supportsStartLogin ? tx("OAuth") : tx("manual")} />
                <KeyValue label="refresh" value={capability?.supportsRefresh ? tx("ready") : tx("manual")} />
                <KeyValue label="quota" value={capability?.supportsQuota ? tx("ready") : tx("none")} />
              </div>
              <div className="capability-flags">
                <span>{tx(capability?.serverNativeStage || template?.credentialKind || "manual")}</span>
                <span>{tx(capability?.quotaStrategy || "quota-none")}</span>
                <span>{tx(capability?.supportsImport ? "import" : "read-only")}</span>
              </div>
              <details className="template-details">
                <summary>{tx("Import template")}</summary>
                <KeyValue label="required" value={(template?.requiredFields || []).join(", ") || "-"} />
                <KeyValue label="optional" value={(template?.optionalFields || []).join(", ") || "-"} />
                <p>{template?.notes || capability?.blockingReason || "-"}</p>
              </details>
              <button className="secondary-button compact" type="button" onClick={() => onImport(providerType)}>
                <Upload size={13} />
                <span>{tx("Import")}</span>
              </button>
            </article>
          );
        })}
      </div>
    </section>
  );
}

export function CodexBankedResetPanel({ accounts }: { accounts: AccountRecord[] }) {
  const { tx } = useI18n();
  const summaries = accounts
    .filter((account) => account.providerType === "codex_oauth" || account.providerType === "codex")
    .map(bankedResetSummary)
    .filter((item): item is BankedResetSummary => Boolean(item));
  return (
    <section className="account-tool-panel banked-reset-panel">
      <div className="section-heading">
        <h2>{tx("Codex Banked Reset")}</h2>
        <span>{summaries.length ? tx("{{count}} snapshots", { count: summaries.length }) : tx("read-only")}</span>
      </div>
      {summaries.length ? (
        <div className="banked-reset-list">
          {summaries.map((summary) => (
            <article className="banked-reset-card" key={summary.account.id}>
              <header>
                <div className="section-title-row compact-title">
                  <RotateCcw size={16} />
                  <div>
                    <h3>{summary.account.email || summary.account.id}</h3>
                    <span>{summary.readOnly ? tx("read-only snapshot") : summary.source || tx("snapshot")}</span>
                  </div>
                </div>
                <StatusPill tone={(summary.availableCount || 0) > 0 ? "success" : "warning"}>
                  {summary.availableCount ?? 0} reset
                </StatusPill>
              </header>
              <div className="provider-card-meta">
                <KeyValue label="available" value={summary.availableCount ?? "-"} />
                <KeyValue label="next expires" value={formatDateish(summary.nextExpiresAt)} />
                <KeyValue label="credits" value={summary.credits.length} />
                <KeyValue label="queried" value={formatTime(summary.queriedAt)} />
              </div>
              {summary.credits.length ? (
                <div className="banked-credit-list">
                  {summary.credits.slice(0, 4).map((credit, index) => (
                    <div className="banked-credit" key={String(credit.id || index)}>
                      <strong>{credit.title || credit.id || `credit ${index + 1}`}</strong>
                      <span>{[credit.status, formatDateish(credit.expiresAt)].filter(Boolean).join(" / ") || "-"}</span>
                    </div>
                  ))}
                </div>
              ) : (
                <div className="provider-card-result">{tx("No banked reset credits in the imported snapshot.")}</div>
              )}
              <details className="json-details">
                <summary>{tx("Snapshot JSON")}</summary>
                <JsonPreview value={summary.raw} redact />
              </details>
            </article>
          ))}
        </div>
      ) : (
        <div className="provider-empty compact-empty">
          <RotateCcw size={20} />
          <span>{tx("No Codex banked reset snapshot")}</span>
        </div>
      )}
    </section>
  );
}
