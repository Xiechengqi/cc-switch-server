import { CheckCircle2, FileJson, Loader2, RefreshCw, ShieldCheck, Trash2, Upload } from "lucide-react";
import { useState } from "react";

import { ConfirmDialog } from "@/components/ConfirmDialog";
import { IconAction } from "@/components/IconAction";
import { JsonPreview } from "@/components/JsonPreview";
import { KeyValue } from "@/components/KeyValue";
import { ProviderIcon } from "@/components/ProviderIcon";
import { StatusPill } from "@/components/StatusPill";
import {
  accountProviderIcon,
  accountQuotaPercent,
  accountRegressionBadges,
  accountTierLine,
  clampPercent,
  credentialFlags,
  formatQuotaPercent,
  formatTime,
  providerLabel,
  quotaCountdownLabel,
  quotaRefreshedLabel,
  quotaTierSummary,
} from "@/components/settings/accountDisplay";
import type {
  AccountImportTemplate,
  AccountManagerCapability,
  AccountQuotaResponse,
  AccountRecord,
  AccountRefreshPlanResponse,
} from "@/lib/api";
import { useI18n } from "@/lib/i18n";

export type AccountAction = "refresh" | "quota" | "forceQuota" | "plan" | "delete";

export type AccountDetail =
  | { kind: "quota"; value: AccountQuotaResponse }
  | { kind: "plan"; value: AccountRefreshPlanResponse };

export function AuthCenterOverview({
  providerTypes,
  accounts,
  capabilitiesByType,
  templatesByType,
  loading,
  onImport,
}: {
  providerTypes: string[];
  accounts: AccountRecord[];
  capabilitiesByType: Map<string, AccountManagerCapability>;
  templatesByType: Map<string, AccountImportTemplate>;
  loading: boolean;
  onImport: (providerType: string) => void;
}) {
  const { tx } = useI18n();
  if (loading) {
    return (
      <section className="auth-center-overview">
        <div className="provider-empty inline-empty">
          <Loader2 size={18} />
          <span>{tx("Loading auth center")}</span>
        </div>
      </section>
    );
  }
  if (!providerTypes.length) return null;
  return (
    <section className="auth-center-overview">
      <div className="section-heading">
        <div className="section-title-row compact-title">
          <ShieldCheck size={17} />
          <div>
            <h2>{tx("Auth Center")}</h2>
            <span>{tx("Accounts, login methods, refresh, and quota readiness")}</span>
          </div>
        </div>
      </div>
      <div className="auth-center-grid">
        {providerTypes.map((providerType) => {
          const providerAccounts = accounts.filter((account) => account.providerType === providerType);
          const capability = capabilitiesByType.get(providerType);
          const template = templatesByType.get(providerType);
          return (
            <AuthCenterProviderCard
              key={providerType}
              providerType={providerType}
              accounts={providerAccounts}
              capability={capability}
              template={template}
              onImport={() => onImport(providerType)}
            />
          );
        })}
      </div>
    </section>
  );
}

export function AccountGroup({
  providerType,
  accounts,
  capability,
  busyId,
  resultById,
  detailById,
  onAction,
}: {
  providerType: string;
  accounts: AccountRecord[];
  capability?: AccountManagerCapability;
  busyId: string | null;
  resultById: Record<string, string>;
  detailById: Record<string, AccountDetail>;
  onAction: (account: AccountRecord, action: AccountAction) => void;
}) {
  const { tx } = useI18n();
  const icon = accountProviderIcon(providerType);
  return (
    <section className="account-group">
      <header>
        <div className="account-provider-title-row">
          <span className="account-icon-frame">
            <ProviderIcon icon={icon.icon} color={icon.color} name={providerLabel(providerType)} size={22} />
          </span>
          <div>
            <h3>{providerLabel(providerType)}</h3>
            <span>{tx(capability?.status || "manual_import_only")}</span>
          </div>
        </div>
        <StatusPill tone={capability?.supportsRefresh ? "success" : "warning"}>
          {tx(capability?.supportsRefresh ? "refresh-ready" : "manual")}
        </StatusPill>
      </header>
      <div className="account-card-grid">
        {accounts.map((account) => (
          <AccountCard
            key={account.id}
            account={account}
            capability={capability}
            busyId={busyId}
            result={resultById[account.id]}
            detail={detailById[account.id]}
            onAction={(action) => onAction(account, action)}
          />
        ))}
      </div>
    </section>
  );
}

function AuthCenterProviderCard({
  providerType,
  accounts,
  capability,
  template,
  onImport,
}: {
  providerType: string;
  accounts: AccountRecord[];
  capability?: AccountManagerCapability;
  template?: AccountImportTemplate;
  onImport: () => void;
}) {
  const { tx } = useI18n();
  const icon = accountProviderIcon(providerType);
  const quotaReady = accounts.filter((account) => account.quota || account.quotaPercent != null).length;
  const errors = accounts.filter((account) => account.lastRefreshError).length;
  const statusTone = errors ? "danger" : capability?.supportsRefresh || capability?.supportsImport ? "success" : "warning";
  return (
    <article className="auth-center-card">
      <header>
        <div className="account-provider-title-row">
          <span className="account-icon-frame compact">
            <ProviderIcon icon={icon.icon} color={icon.color} name={providerLabel(providerType)} size={18} />
          </span>
          <div>
            <h3>{providerLabel(providerType)}</h3>
            <span>{template?.credentialKind || capability?.serverNativeStage || tx("manual")}</span>
          </div>
        </div>
        <StatusPill tone={statusTone}>
          {errors ? tx("error") : capability?.status || tx("ready")}
        </StatusPill>
      </header>
      <div className="auth-center-metrics">
        <KeyValue label="accounts" value={accounts.length} />
        <KeyValue label="quota" value={`${quotaReady}/${accounts.length || "-"}`} />
        <KeyValue label="refresh" value={capability?.supportsRefresh ? tx("ready") : tx("manual")} />
      </div>
      <div className="capability-flags">
        <span>{capability?.supportsStartLogin ? tx("OAuth") : tx("manual import")}</span>
        <span>{capability?.supportsQuota ? tx("quota") : tx("no-quota")}</span>
        <span>{template ? tx("template") : tx("no-template")}</span>
      </div>
      <button className="secondary-button compact" type="button" onClick={onImport}>
        <Upload size={13} />
        <span>{tx(accounts.length ? "Import another" : "Import")}</span>
      </button>
    </article>
  );
}

function AccountCard({
  account,
  capability,
  busyId,
  result,
  detail,
  onAction,
}: {
  account: AccountRecord;
  capability?: AccountManagerCapability;
  busyId: string | null;
  result?: string;
  detail?: AccountDetail;
  onAction: (action: AccountAction) => void;
}) {
  const { tx } = useI18n();
  const [deleteConfirmOpen, setDeleteConfirmOpen] = useState(false);
  const busyPrefix = `${account.id}:`;
  const credentials = credentialFlags(account);
  const icon = accountProviderIcon(account.providerType);
  return (
    <>
      <article className="account-card">
        <header>
          <div className="account-provider-title-row">
            <span className="account-icon-frame compact">
              <ProviderIcon icon={icon.icon} color={icon.color} name={providerLabel(account.providerType)} size={18} />
            </span>
            <div>
              <h4>{account.email || account.id}</h4>
              <span>{account.id}</span>
            </div>
          </div>
          <StatusPill tone={account.lastRefreshError ? "danger" : "success"}>
            {tx(account.lastRefreshError ? "error" : "ready")}
          </StatusPill>
        </header>
        <div className="provider-card-meta">
          <KeyValue label="plan" value={account.subscriptionLevel || "-"} />
          <KeyValue label="quota" value={formatQuotaPercent(account)} />
          <KeyValue label="expires" value={formatTime(account.expiresAt)} />
          <KeyValue label="refresh at" value={formatTime(account.quotaNextRefreshAt)} />
        </div>
        <AccountRegressionStrip account={account} capability={capability} />
        <div className="credential-badges">
          {credentials.length ? credentials.map((item) => <span key={item}>{item}</span>) : <span>{tx("no credential flag")}</span>}
        </div>
        <div className="provider-card-result">
          {result || account.lastRefreshError || quotaTierSummary(account) || capability?.blockingReason || tx("account imported")}
        </div>
        <AccountQuotaFooter account={account} capability={capability} />
        {detail && (
          <details className="json-details">
            <summary>{tx(detail.kind === "plan" ? "Refresh plan" : "Quota result")}</summary>
            <JsonPreview value={detail.value} redact />
          </details>
        )}
        <div className="provider-actions">
          <IconAction
            title="Refresh account"
            onClick={() => onAction("refresh")}
            busy={busyId === `${busyPrefix}refresh`}
            disabled={!capability?.supportsRefresh}
            wrap={false}
          >
            <RefreshCw size={15} />
          </IconAction>
          <IconAction
            title="Quota snapshot"
            onClick={() => onAction("quota")}
            busy={busyId === `${busyPrefix}quota`}
            disabled={!capability?.supportsQuota}
            wrap={false}
          >
            <FileJson size={15} />
          </IconAction>
          <IconAction
            title="Refresh quota"
            onClick={() => onAction("forceQuota")}
            busy={busyId === `${busyPrefix}forceQuota`}
            disabled={!capability?.supportsQuota}
            wrap={false}
          >
            <CheckCircle2 size={15} />
          </IconAction>
          <IconAction
            title="Refresh plan"
            onClick={() => onAction("plan")}
            busy={busyId === `${busyPrefix}plan`}
            disabled={!capability?.supportsRefreshPlan}
            wrap={false}
          >
            <ShieldCheck size={15} />
          </IconAction>
          <IconAction
            title="Delete"
            onClick={() => setDeleteConfirmOpen(true)}
            busy={busyId === `${busyPrefix}delete`}
            danger
            wrap={false}
          >
            <Trash2 size={15} />
          </IconAction>
        </div>
      </article>
      <ConfirmDialog
        isOpen={deleteConfirmOpen}
        title={tx("Delete account")}
        message={tx("Delete account {{account}}?", { account: account.email || account.id })}
        confirmText={tx("Delete")}
        onConfirm={() => {
          setDeleteConfirmOpen(false);
          onAction("delete");
        }}
        onCancel={() => setDeleteConfirmOpen(false)}
      />
    </>
  );
}

function AccountQuotaFooter({
  account,
  capability,
}: {
  account: AccountRecord;
  capability?: AccountManagerCapability;
}) {
  const { tx } = useI18n();
  const quotaPercent = accountQuotaPercent(account);
  const tiers = account.quota?.tiers || [];
  const hasQuotaData = quotaPercent != null || tiers.length > 0 || account.quotaRefreshedAt != null || account.quotaNextRefreshAt != null;
  if (!capability?.supportsQuota && !hasQuotaData) return null;
  return (
    <div className="account-quota-footer">
      <div className="account-quota-line">
        <span>{capability?.supportsQuota ? tx("quota ready") : tx("quota gated")}</span>
        <span>{quotaPercent == null ? tx("quota -") : `${quotaPercent.toFixed(1)}%`}</span>
        <span>{account.subscriptionLevel || tx("account")}</span>
        <span>{quotaRefreshedLabel(account.quotaRefreshedAt, tx)}</span>
      </div>
      {quotaPercent != null && (
        <div className="account-quota-meter" aria-label={tx("quota")}>
          <span style={{ width: `${clampPercent(quotaPercent)}%` }} />
        </div>
      )}
      {tiers.length > 0 && (
        <div className="account-quota-tier-list">
          {tiers.slice(0, 3).map((tier) => (
            <div className="account-quota-tier" key={tier.name}>
              <div>
                <strong>{tier.name}</strong>
                <span>{accountTierLine(tier, tx)}</span>
              </div>
              <div className="account-quota-tier-meter">
                <span style={{ width: `${clampPercent(tier.utilization ?? 0)}%` }} />
              </div>
            </div>
          ))}
        </div>
      )}
      {account.quotaNextRefreshAt != null && (
        <div className="account-quota-note">
          <span>{tx("next refresh")}</span>
          <strong title={formatTime(account.quotaNextRefreshAt)}>
            {quotaCountdownLabel(account.quotaNextRefreshAt, tx)}
          </strong>
        </div>
      )}
      {account.lastRefreshError && <strong className="account-quota-error">{account.lastRefreshError}</strong>}
    </div>
  );
}

function AccountRegressionStrip({
  account,
  capability,
}: {
  account: AccountRecord;
  capability?: AccountManagerCapability;
}) {
  return (
    <div className="account-regression-strip">
      {accountRegressionBadges(account, capability).map((badge) => (
        <span key={badge.label} className={`account-regression-badge ${badge.tone}`}>
          <strong>{badge.label}</strong>
          <small>{badge.value}</small>
        </span>
      ))}
    </div>
  );
}
