import {
  CheckCircle2,
  ExternalLink,
  FileJson,
  KeyRound,
  Loader2,
  LogIn,
  Play,
  RefreshCw,
  RotateCcw,
  ShieldCheck,
  Trash2,
  Upload,
  UserRound,
  X,
} from "lucide-react";
import { FormEvent, ReactNode, useCallback, useEffect, useMemo, useState } from "react";

import { ProviderIcon } from "@/components/ProviderIcon";
import { inferIconForText } from "@/config/iconInference";
import {
  AccountDeviceCodeResponse,
  AccountDevicePollResponse,
  AccountImportTemplate,
  AccountManagerCapability,
  AccountQuotaResponse,
  AccountRecord,
  AccountRefreshPlanResponse,
  deleteAccount,
  finishAccountLogin,
  loadAccountQuota,
  loadAccountRefreshPlan,
  loadAccountsDashboardData,
  OAuthLoginFinish,
  OAuthLoginStart,
  pollCopilotDeviceLogin,
  pollKiroDeviceLogin,
  refreshAccount,
  startAccountLogin,
  startCopilotDeviceLogin,
  startKiroDeviceLogin,
  upsertAccount,
  UpsertAccountInput,
} from "@/lib/api";
import { useI18n } from "@/lib/i18n";

interface AccountsDashboardState {
  accounts: AccountRecord[];
  capabilities: AccountManagerCapability[];
  templates: AccountImportTemplate[];
}

interface AccountImportDraft {
  providerType: string;
  id: string;
  email: string;
  accessToken: string;
  refreshToken: string;
  idToken: string;
  tokenType: string;
  apiKey: string;
  scopes: string;
  subscriptionLevel: string;
  quotaPercent: string;
  expiresAt: string;
  profileJson: string;
  rawJson: string;
  quotaJson: string;
}

type AccountDetail =
  | { kind: "quota"; value: AccountQuotaResponse }
  | { kind: "plan"; value: AccountRefreshPlanResponse };

interface BankedResetCredit {
  id?: string | null;
  status?: string | null;
  grantedAt?: string | null;
  expiresAt?: string | null;
  title?: string | null;
  description?: string | null;
  [key: string]: unknown;
}

interface BankedResetSummary {
  account: AccountRecord;
  availableCount: number | null;
  nextExpiresAt?: string | null;
  readOnly: boolean;
  source?: string | null;
  queriedAt?: number | null;
  credits: BankedResetCredit[];
  raw: unknown;
}

interface AccountRegressionBadge {
  label: string;
  value: string;
  tone: "success" | "warning" | "danger";
}

const oauthPreviewProviderTypes = [
  "codex_oauth",
  "claude_oauth",
  "gemini_cli",
  "cursor_oauth",
  "antigravity_oauth",
  "agy_oauth",
];

export function AccountsDashboard() {
  const { t, tx } = useI18n();
  const [data, setData] = useState<AccountsDashboardState>({
    accounts: [],
    capabilities: [],
    templates: [],
  });
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [busyId, setBusyId] = useState<string | null>(null);
  const [detailById, setDetailById] = useState<Record<string, AccountDetail>>({});
  const [resultById, setResultById] = useState<Record<string, string>>({});
  const [importDraft, setImportDraft] = useState<AccountImportDraft | null>(null);

  const refresh = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      setData(await loadAccountsDashboardData());
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const capabilitiesByType = useMemo(
    () => new Map(data.capabilities.map((item) => [item.providerType, item])),
    [data.capabilities],
  );
  const templatesByType = useMemo(
    () => new Map(data.templates.map((item) => [item.providerType, item])),
    [data.templates],
  );
  const providerTypes = useMemo(() => {
    const all = new Set<string>();
    data.capabilities.forEach((item) => all.add(item.providerType));
    data.templates.forEach((item) => all.add(item.providerType));
    data.accounts.forEach((item) => all.add(item.providerType));
    return Array.from(all).sort((left, right) => providerLabel(left).localeCompare(providerLabel(right)));
  }, [data.accounts, data.capabilities, data.templates]);

  async function runAccountAction(
    account: AccountRecord,
    action: "refresh" | "quota" | "forceQuota" | "plan" | "delete",
  ) {
    const key = `${account.id}:${action}`;
    setBusyId(key);
    setError(null);
    try {
      if (action === "delete") {
        if (!window.confirm(tx("Delete account {{account}}?", { account: account.email || account.id }))) return;
        const deleted = await deleteAccount(account.id);
        setResultById((current) => ({
          ...current,
          [account.id]: deleted ? tx("account deleted") : tx("account not found"),
        }));
        await refresh();
        return;
      }
      if (action === "refresh") {
        const updated = await refreshAccount(account.id);
        setResultById((current) => ({
          ...current,
          [account.id]: tx("refreshed {{account}}", { account: updated.email || updated.id }),
        }));
        await refresh();
        return;
      }
      if (action === "plan") {
        const value = await loadAccountRefreshPlan(account.id);
        setDetailById((current) => ({ ...current, [account.id]: { kind: "plan", value } }));
        setResultById((current) => ({ ...current, [account.id]: value.message }));
        return;
      }
      const value = await loadAccountQuota(account.id, {
        refresh: action === "forceQuota",
        force: action === "forceQuota",
      });
      setDetailById((current) => ({ ...current, [account.id]: { kind: "quota", value } }));
      setResultById((current) => ({
        ...current,
        [account.id]: value.message || (value.refreshed ? tx("quota refreshed") : tx("quota snapshot")),
      }));
      if (action === "forceQuota") await refresh();
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setBusyId(null);
    }
  }

  return (
    <div className="accounts-dashboard">
      <div className="provider-toolbar">
        <div className="section-title-row">
          <KeyRound size={18} />
          <div>
            <h2>{t("server.accounts.title")}</h2>
            <span>{t("server.accounts.importedCredentials", { count: data.accounts.length })}</span>
          </div>
        </div>
        <div className="provider-toolbar-actions">
          {error && <span className="error-text">{error}</span>}
          <button className="secondary-button" type="button" onClick={() => void refresh()}>
            <RefreshCw size={15} />
            <span>{t("common.refresh")}</span>
          </button>
          <button
            className="primary-button"
            type="button"
            onClick={() => setImportDraft(createAccountImportDraft(providerTypes[0] || "codex_oauth"))}
          >
            <Upload size={15} />
            <span>{t("server.accounts.importAccount")}</span>
          </button>
        </div>
      </div>

      <div className="accounts-stats-bar">
        <AccountStat label={t("server.accounts.accounts")} value={data.accounts.length} />
        <AccountStat
          label={t("server.accounts.nativeRefresh")}
          value={data.capabilities.filter((item) => item.supportsRefresh).length}
        />
        <AccountStat
          label={t("server.accounts.quotaReady")}
          value={data.capabilities.filter((item) => item.supportsQuota).length}
        />
        <AccountStat label={t("server.accounts.importTypes")} value={data.templates.length} />
      </div>

      <div className="accounts-layout">
        <section className="accounts-main">
          <div className="section-heading">
            <h2>{t("server.accounts.importedAccounts")}</h2>
            <span>{loading ? t("common.loading") : t("server.accounts.providerTypes", { count: providerTypes.length })}</span>
          </div>
          {loading ? (
            <div className="provider-empty">
              <Loader2 size={22} />
              <span>{t("server.accounts.loading")}</span>
            </div>
          ) : data.accounts.length ? (
            <div className="account-group-list">
              {providerTypes.map((providerType) => {
                const accounts = data.accounts.filter((item) => item.providerType === providerType);
                if (!accounts.length) return null;
                return (
                  <AccountGroup
                    key={providerType}
                    providerType={providerType}
                    accounts={accounts}
                    capability={capabilitiesByType.get(providerType)}
                    busyId={busyId}
                    resultById={resultById}
                    detailById={detailById}
                    onAction={runAccountAction}
                  />
                );
              })}
            </div>
          ) : (
            <div className="provider-empty">
              <UserRound size={24} />
              <strong>{t("server.accounts.noAccounts")}</strong>
              <span>{t("server.accounts.noAccountsHint")}</span>
            </div>
          )}
        </section>

        <aside className="accounts-side">
          <CapabilityPanel
            providerTypes={providerTypes}
            capabilitiesByType={capabilitiesByType}
            templatesByType={templatesByType}
            onImport={(providerType) => setImportDraft(createAccountImportDraft(providerType))}
          />
          <CodexBankedResetPanel accounts={data.accounts} />
          <OAuthPreviewPanel
            capabilities={data.capabilities}
            onImported={() => void refresh()}
          />
          <DeviceFlowPanel onImported={() => void refresh()} />
        </aside>
      </div>

      {importDraft && (
        <AccountImportModal
          draft={importDraft}
          templates={data.templates}
          capabilities={data.capabilities}
          saving={busyId === "account-import"}
          onChange={setImportDraft}
          onClose={() => setImportDraft(null)}
          onSubmit={async (event) => {
            event.preventDefault();
            setBusyId("account-import");
            setError(null);
            try {
              await upsertAccount(accountInputFromDraft(importDraft));
              setImportDraft(null);
              await refresh();
            } catch (reason) {
              setError(tx(errorMessage(reason)));
            } finally {
              setBusyId(null);
            }
          }}
        />
      )}
    </div>
  );
}

function AccountGroup({
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
  onAction: (
    account: AccountRecord,
    action: "refresh" | "quota" | "forceQuota" | "plan" | "delete",
  ) => void;
}) {
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
            <span>{capability?.status || "manual_import_only"}</span>
          </div>
        </div>
        <StatusPill tone={capability?.supportsRefresh ? "success" : "warning"}>
          {capability?.supportsRefresh ? "refresh-ready" : "manual"}
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
  onAction: (action: "refresh" | "quota" | "forceQuota" | "plan" | "delete") => void;
}) {
  const busyPrefix = `${account.id}:`;
  const credentials = credentialFlags(account);
  const icon = accountProviderIcon(account.providerType);
  return (
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
          {account.lastRefreshError ? "error" : "ready"}
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
        {credentials.length ? credentials.map((item) => <span key={item}>{item}</span>) : <span>no credential flag</span>}
      </div>
      <div className="provider-card-result">
        {result || account.lastRefreshError || quotaTierSummary(account) || capability?.blockingReason || "account imported"}
      </div>
      {detail && (
        <details className="json-details">
          <summary>{detail.kind === "plan" ? "Refresh plan" : "Quota result"}</summary>
          <JsonPreview value={detail.value} />
        </details>
      )}
      <div className="provider-actions">
        <IconAction
          title="Refresh account"
          onClick={() => onAction("refresh")}
          busy={busyId === `${busyPrefix}refresh`}
          disabled={!capability?.supportsRefresh}
        >
          <RefreshCw size={15} />
        </IconAction>
        <IconAction
          title="Quota snapshot"
          onClick={() => onAction("quota")}
          busy={busyId === `${busyPrefix}quota`}
          disabled={!capability?.supportsQuota}
        >
          <FileJson size={15} />
        </IconAction>
        <IconAction
          title="Refresh quota"
          onClick={() => onAction("forceQuota")}
          busy={busyId === `${busyPrefix}forceQuota`}
          disabled={!capability?.supportsQuota}
        >
          <CheckCircle2 size={15} />
        </IconAction>
        <IconAction
          title="Refresh plan"
          onClick={() => onAction("plan")}
          busy={busyId === `${busyPrefix}plan`}
          disabled={!capability?.supportsRefreshPlan}
        >
          <ShieldCheck size={15} />
        </IconAction>
        <IconAction
          title="Delete"
          onClick={() => onAction("delete")}
          busy={busyId === `${busyPrefix}delete`}
          danger
        >
          <Trash2 size={15} />
        </IconAction>
      </div>
    </article>
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

function CapabilityPanel({
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
    <section className="account-tool-panel">
      <div className="section-heading">
        <h2>{tx("Capability Matrix")}</h2>
        <span>{tx("{{count}} types", { count: providerTypes.length })}</span>
      </div>
      <div className="capability-list">
        {providerTypes.map((providerType) => {
          const capability = capabilitiesByType.get(providerType);
          const template = templatesByType.get(providerType);
          const icon = accountProviderIcon(providerType);
          return (
            <article className="capability-card" key={providerType}>
              <header>
                <div className="account-provider-title-row">
                  <span className="account-icon-frame compact">
                    <ProviderIcon icon={icon.icon} color={icon.color} name={providerLabel(providerType)} size={18} />
                  </span>
                  <div>
                    <h3>{providerLabel(providerType)}</h3>
                    <span>{template?.credentialKind || "manual"}</span>
                  </div>
                </div>
                <StatusPill tone={capability?.supportsRefresh ? "success" : "warning"}>
                  {capability?.status || "manual_import_only"}
                </StatusPill>
              </header>
              <div className="capability-flags">
                <span>{capability?.serverNativeStage || "manual"}</span>
                <span>{capability?.quotaStrategy || "quota-none"}</span>
                <span>{capability?.supportsQuota ? "quota" : "no-quota"}</span>
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

function CodexBankedResetPanel({ accounts }: { accounts: AccountRecord[] }) {
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
                <JsonPreview value={summary.raw} />
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

function OAuthPreviewPanel({
  capabilities,
  onImported,
}: {
  capabilities: AccountManagerCapability[];
  onImported: () => void;
}) {
  const { tx } = useI18n();
  const available = oauthPreviewProviderTypes.filter((providerType) =>
    capabilities.some((item) => item.providerType === providerType),
  );
  const [providerType, setProviderType] = useState(available[0] || "codex_oauth");
  const [redirectUri, setRedirectUri] = useState("");
  const [login, setLogin] = useState<OAuthLoginStart | null>(null);
  const [finishResult, setFinishResult] = useState<OAuthLoginFinish | null>(null);
  const [accountResult, setAccountResult] = useState<ReactNode>(null);
  const [sessionId, setSessionId] = useState("");
  const [state, setState] = useState("");
  const [code, setCode] = useState("");
  const [executeTokenExchange, setExecuteTokenExchange] = useState(false);
  const [busy, setBusy] = useState<"start" | "finish" | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (available.length && !available.includes(providerType)) {
      setProviderType(available[0]);
    }
  }, [available, providerType]);

  async function start(event: FormEvent) {
    event.preventDefault();
    setBusy("start");
    setError(null);
    try {
      const next = await startAccountLogin({
        providerType,
        redirectUri: redirectUri.trim() || undefined,
      });
      setLogin(next);
      setSessionId(next.sessionId);
      setState(next.state);
      setFinishResult(null);
      setAccountResult(null);
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setBusy(null);
    }
  }

  async function finish(event: FormEvent) {
    event.preventDefault();
    setBusy("finish");
    setError(null);
    try {
      const next = await finishAccountLogin({
        sessionId: sessionId.trim() || undefined,
        state: state.trim() || undefined,
        code: code.trim() || undefined,
        executeTokenExchange,
      });
      setFinishResult(next.login);
      setAccountResult(
        next.account
          ? `${next.account.email || next.account.id} imported`
          : "token request preview ready",
      );
      if (next.account) onImported();
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setBusy(null);
    }
  }

  return (
    <section className="account-tool-panel">
      <div className="section-heading">
        <h2>{tx("OAuth Preview")}</h2>
        <span>{tx("manual gate")}</span>
      </div>
      <form className="account-tool-form" onSubmit={start}>
        <label>
          <span>{tx("Provider")}</span>
          <select value={providerType} onChange={(event) => setProviderType(event.target.value)}>
            {available.map((item) => (
              <option key={item} value={item}>
                {providerLabel(item)}
              </option>
            ))}
          </select>
        </label>
        <label>
          <span>{tx("Redirect URI")}</span>
          <input value={redirectUri} onChange={(event) => setRedirectUri(event.target.value)} />
        </label>
        <button className="secondary-button" type="submit" disabled={busy === "start"}>
          {busy === "start" ? <Loader2 size={15} /> : <LogIn size={15} />}
          <span>{tx("Start")}</span>
        </button>
      </form>
      {login && (
        <div className="oauth-result">
          <div className="provider-card-meta">
            <KeyValue label="session" value={login.sessionId} />
            <KeyValue label="state" value={login.state} />
            <KeyValue label="stage" value={login.serverNativeStage} />
            <KeyValue label="expires" value={formatTime(login.expiresAtMs)} />
          </div>
          <a href={login.authorizeUrl} target="_blank" rel="noreferrer" className="inline-link">
            <ExternalLink size={14} />
            <span>{tx("Open authorize URL")}</span>
          </a>
        </div>
      )}
      <form className="account-tool-form" onSubmit={finish}>
        <label>
          <span>{tx("Session ID")}</span>
          <input value={sessionId} onChange={(event) => setSessionId(event.target.value)} />
        </label>
        <label>
          <span>{tx("State")}</span>
          <input value={state} onChange={(event) => setState(event.target.value)} />
        </label>
        <label className="wide-field">
          <span>{tx("Authorization code")}</span>
          <input value={code} onChange={(event) => setCode(event.target.value)} />
        </label>
        <label className="toggle-row wide-field">
          <input
            type="checkbox"
            checked={executeTokenExchange}
            onChange={(event) => setExecuteTokenExchange(event.target.checked)}
          />
          <span>{tx("Execute token exchange")}</span>
        </label>
        <button className="secondary-button" type="submit" disabled={busy === "finish"}>
          {busy === "finish" ? <Loader2 size={15} /> : <Play size={15} />}
          <span>{tx(executeTokenExchange ? "Exchange" : "Preview")}</span>
        </button>
      </form>
      {accountResult && <div className="provider-card-result">{accountResult}</div>}
      {finishResult?.tokenRequest && (
        <details className="json-details">
          <summary>{tx("Token request")}</summary>
          <JsonPreview value={finishResult.tokenRequest} />
        </details>
      )}
      {error && <div className="form-error">{error}</div>}
    </section>
  );
}

function DeviceFlowPanel({ onImported }: { onImported: () => void }) {
  const { tx } = useI18n();
  const [copilotDomain, setCopilotDomain] = useState("");
  const [copilotDevice, setCopilotDevice] = useState<AccountDeviceCodeResponse | null>(null);
  const [copilotPoll, setCopilotPoll] = useState<AccountDevicePollResponse | null>(null);
  const [kiroRegion, setKiroRegion] = useState("us-east-1");
  const [kiroStartUrl, setKiroStartUrl] = useState("https://view.awsapps.com/start");
  const [kiroDevice, setKiroDevice] = useState<AccountDeviceCodeResponse | null>(null);
  const [kiroPoll, setKiroPoll] = useState<AccountDevicePollResponse | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  async function startCopilot() {
    setBusy("copilot-start");
    setError(null);
    try {
      setCopilotDevice(await startCopilotDeviceLogin({ githubDomain: copilotDomain.trim() || undefined }));
      setCopilotPoll(null);
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setBusy(null);
    }
  }

  async function pollCopilot() {
    if (!copilotDevice) return;
    setBusy("copilot-poll");
    setError(null);
    try {
      const next = await pollCopilotDeviceLogin({
        deviceCode: copilotDevice.deviceCode,
        githubDomain: copilotDevice.githubDomain || copilotDomain.trim() || undefined,
      });
      setCopilotPoll(next);
      if (next.account) onImported();
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setBusy(null);
    }
  }

  async function startKiro() {
    setBusy("kiro-start");
    setError(null);
    try {
      setKiroDevice(await startKiroDeviceLogin({
        region: kiroRegion.trim() || undefined,
        startUrl: kiroStartUrl.trim() || undefined,
      }));
      setKiroPoll(null);
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setBusy(null);
    }
  }

  async function pollKiro() {
    if (!kiroDevice) return;
    setBusy("kiro-poll");
    setError(null);
    try {
      const next = await pollKiroDeviceLogin({ deviceCode: kiroDevice.deviceCode });
      setKiroPoll(next);
      if (next.account) onImported();
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setBusy(null);
    }
  }

  return (
    <section className="account-tool-panel">
      <div className="section-heading">
        <h2>{tx("Device Flow")}</h2>
        <span>{tx("Copilot / Kiro")}</span>
      </div>
      <div className="device-flow-grid">
        <DeviceFlowCard
          title="GitHub Copilot"
          icon={<AccountProviderIcon providerType="github_copilot" size={16} />}
          fields={
            <label>
              <span>{tx("GitHub domain")}</span>
              <input value={copilotDomain} onChange={(event) => setCopilotDomain(event.target.value)} />
            </label>
          }
          device={copilotDevice}
          poll={copilotPoll}
          startBusy={busy === "copilot-start"}
          pollBusy={busy === "copilot-poll"}
          onStart={() => void startCopilot()}
          onPoll={() => void pollCopilot()}
        />
        <DeviceFlowCard
          title="Kiro OAuth"
          icon={<AccountProviderIcon providerType="kiro_oauth" size={16} />}
          fields={
            <>
              <label>
                <span>{tx("Region")}</span>
                <input value={kiroRegion} onChange={(event) => setKiroRegion(event.target.value)} />
              </label>
              <label>
                <span>{tx("Start URL")}</span>
                <input value={kiroStartUrl} onChange={(event) => setKiroStartUrl(event.target.value)} />
              </label>
            </>
          }
          device={kiroDevice}
          poll={kiroPoll}
          startBusy={busy === "kiro-start"}
          pollBusy={busy === "kiro-poll"}
          onStart={() => void startKiro()}
          onPoll={() => void pollKiro()}
        />
      </div>
      {error && <div className="form-error">{error}</div>}
    </section>
  );
}

function DeviceFlowCard({
  title,
  icon,
  fields,
  device,
  poll,
  startBusy,
  pollBusy,
  onStart,
  onPoll,
}: {
  title: string;
  icon: ReactNode;
  fields: ReactNode;
  device: AccountDeviceCodeResponse | null;
  poll: AccountDevicePollResponse | null;
  startBusy: boolean;
  pollBusy: boolean;
  onStart: () => void;
  onPoll: () => void;
}) {
  const { tx } = useI18n();
  return (
    <article className="device-flow-card">
      <header>
        <div className="section-title-row compact-title">
          {icon}
          <h3>{tx(title)}</h3>
        </div>
      </header>
      <div className="account-tool-form">
        {fields}
        <button className="secondary-button" type="button" onClick={onStart} disabled={startBusy}>
          {startBusy ? <Loader2 size={15} /> : <Play size={15} />}
          <span>{tx("Start")}</span>
        </button>
      </div>
      {device && (
        <div className="device-code-block">
          <KeyValue label="user code" value={device.userCode} />
          <KeyValue label="expires" value={`${device.expiresIn}s`} />
          <a href={device.verificationUriComplete || device.verificationUri} target="_blank" rel="noreferrer" className="inline-link">
            <ExternalLink size={14} />
            <span>{tx("Open verification")}</span>
          </a>
          <button className="secondary-button compact" type="button" onClick={onPoll} disabled={pollBusy}>
            {pollBusy ? <Loader2 size={13} /> : <RefreshCw size={13} />}
            <span>{tx("Poll")}</span>
          </button>
        </div>
      )}
      {poll && (
        <div className="provider-card-result">
          {poll.account ? tx("{{account}} imported", { account: poll.account.email || poll.account.id }) : poll.message}
        </div>
      )}
    </article>
  );
}

function AccountImportModal({
  draft,
  templates,
  capabilities,
  saving,
  onChange,
  onSubmit,
  onClose,
}: {
  draft: AccountImportDraft;
  templates: AccountImportTemplate[];
  capabilities: AccountManagerCapability[];
  saving: boolean;
  onChange: (draft: AccountImportDraft) => void;
  onSubmit: (event: FormEvent) => void;
  onClose: () => void;
}) {
  const { tx } = useI18n();
  const providerTypes = Array.from(
    new Set([...templates.map((item) => item.providerType), ...capabilities.map((item) => item.providerType)]),
  ).sort((left, right) => providerLabel(left).localeCompare(providerLabel(right)));
  const template = templates.find((item) => item.providerType === draft.providerType);
  function patch(next: Partial<AccountImportDraft>) {
    onChange({ ...draft, ...next });
  }
  return (
    <div className="modal-backdrop" role="presentation">
      <form className="provider-form-modal account-import-modal" onSubmit={onSubmit}>
        <header>
          <div>
            <h2>{tx("Import Account")}</h2>
            <p>{template?.credentialKind || tx("manual credential")}</p>
          </div>
          <button className="icon-button" type="button" onClick={onClose} aria-label={tx("Close")}>
            <X size={16} />
          </button>
        </header>
        <div className="provider-form-grid">
          <label>
            <span>{tx("Provider type")}</span>
            <select value={draft.providerType} onChange={(event) => patch({ providerType: event.target.value })}>
              {providerTypes.map((providerType) => (
                <option key={providerType} value={providerType}>
                  {providerLabel(providerType)}
                </option>
              ))}
            </select>
          </label>
          <label>
            <span>{tx("Account ID")}</span>
            <input value={draft.id} onChange={(event) => patch({ id: event.target.value })} />
          </label>
          <label>
            <span>{tx("Email")}</span>
            <input value={draft.email} onChange={(event) => patch({ email: event.target.value })} />
          </label>
          <label>
            <span>{tx("Subscription")}</span>
            <input
              value={draft.subscriptionLevel}
              onChange={(event) => patch({ subscriptionLevel: event.target.value })}
            />
          </label>
          <label>
            <span>{tx("Quota percent")}</span>
            <input value={draft.quotaPercent} onChange={(event) => patch({ quotaPercent: event.target.value })} />
          </label>
          <label>
            <span>{tx("Expires at")}</span>
            <input
              type="datetime-local"
              value={draft.expiresAt}
              onChange={(event) => patch({ expiresAt: event.target.value })}
            />
          </label>
          <label>
            <span>{tx("Access token")}</span>
            <input
              type="password"
              value={draft.accessToken}
              onChange={(event) => patch({ accessToken: event.target.value })}
            />
          </label>
          <label>
            <span>{tx("Refresh token")}</span>
            <input
              type="password"
              value={draft.refreshToken}
              onChange={(event) => patch({ refreshToken: event.target.value })}
            />
          </label>
          <label>
            <span>{tx("ID token")}</span>
            <input type="password" value={draft.idToken} onChange={(event) => patch({ idToken: event.target.value })} />
          </label>
          <label>
            <span>{tx("API key")}</span>
            <input type="password" value={draft.apiKey} onChange={(event) => patch({ apiKey: event.target.value })} />
          </label>
          <label>
            <span>{tx("Token type")}</span>
            <input value={draft.tokenType} onChange={(event) => patch({ tokenType: event.target.value })} />
          </label>
          <label>
            <span>{tx("Scopes")}</span>
            <input value={draft.scopes} onChange={(event) => patch({ scopes: event.target.value })} />
          </label>
          <label className="wide-field">
            <span>{tx("Profile JSON")}</span>
            <textarea value={draft.profileJson} onChange={(event) => patch({ profileJson: event.target.value })} />
          </label>
          <label className="wide-field">
            <span>{tx("Raw JSON")}</span>
            <textarea value={draft.rawJson} onChange={(event) => patch({ rawJson: event.target.value })} />
          </label>
          <label className="wide-field">
            <span>{tx("Quota JSON")}</span>
            <textarea value={draft.quotaJson} onChange={(event) => patch({ quotaJson: event.target.value })} />
          </label>
          <div className="wide-field template-note">
            <KeyValue label="required" value={(template?.requiredFields || []).join(", ") || "-"} />
            <p>{template?.notes || "-"}</p>
          </div>
        </div>
        <footer>
          <button className="secondary-button" type="button" onClick={onClose}>
            {tx("Cancel")}
          </button>
          <button className="primary-button" type="submit" disabled={saving}>
            {saving && <Loader2 size={15} />}
            <span>{tx("Save Account")}</span>
          </button>
        </footer>
      </form>
    </div>
  );
}

function createAccountImportDraft(providerType: string): AccountImportDraft {
  return {
    providerType,
    id: "",
    email: "",
    accessToken: "",
    refreshToken: "",
    idToken: "",
    tokenType: "",
    apiKey: "",
    scopes: "",
    subscriptionLevel: "",
    quotaPercent: "",
    expiresAt: "",
    profileJson: "",
    rawJson: "",
    quotaJson: "",
  };
}

function accountInputFromDraft(draft: AccountImportDraft): UpsertAccountInput {
  const input: UpsertAccountInput = {
    providerType: draft.providerType,
  };
  assignString(input, "id", draft.id);
  assignString(input, "email", draft.email);
  assignString(input, "accessToken", draft.accessToken);
  assignString(input, "refreshToken", draft.refreshToken);
  assignString(input, "idToken", draft.idToken);
  assignString(input, "tokenType", draft.tokenType);
  assignString(input, "apiKey", draft.apiKey);
  assignString(input, "subscriptionLevel", draft.subscriptionLevel);
  const scopes = splitScopes(draft.scopes);
  if (scopes.length) input.scopes = scopes;
  const quotaPercent = parseOptionalNumber(draft.quotaPercent, "quota percent");
  if (quotaPercent != null) input.quotaPercent = quotaPercent;
  const expiresAt = parseOptionalDateTime(draft.expiresAt);
  if (expiresAt != null) input.expiresAt = expiresAt;
  const profile = parseOptionalJson(draft.profileJson, "profile JSON");
  if (profile !== undefined) input.profile = profile;
  const raw = parseOptionalJson(draft.rawJson, "raw JSON");
  if (raw !== undefined) input.raw = raw;
  const quota = parseOptionalJson(draft.quotaJson, "quota JSON");
  if (quota !== undefined) input.quota = quota as UpsertAccountInput["quota"];
  if (!input.accessToken && !input.refreshToken && !input.apiKey && !input.raw) {
    throw new Error("account import requires credential material");
  }
  return input;
}

function assignString(target: UpsertAccountInput, key: keyof UpsertAccountInput, value: string) {
  const trimmed = value.trim();
  if (trimmed) {
    (target as unknown as Record<string, unknown>)[key] = trimmed;
  }
}

function splitScopes(value: string): string[] {
  return value
    .split(/[\s,]+/)
    .map((item) => item.trim())
    .filter(Boolean);
}

function parseOptionalNumber(value: string, label: string): number | undefined {
  if (!value.trim()) return undefined;
  const parsed = Number(value);
  if (!Number.isFinite(parsed)) throw new Error(`${label} must be a number`);
  return parsed;
}

function parseOptionalDateTime(value: string): number | undefined {
  if (!value.trim()) return undefined;
  const parsed = Date.parse(value);
  if (!Number.isFinite(parsed)) throw new Error("expires at is invalid");
  return parsed;
}

function parseOptionalJson(value: string, label: string): unknown {
  if (!value.trim()) return undefined;
  try {
    return JSON.parse(value);
  } catch (error) {
    throw new Error(`${label} is invalid: ${errorMessage(error)}`);
  }
}

function AccountStat({ label, value }: { label: string; value: ReactNode }) {
  const { tx } = useI18n();
  return (
    <div className="account-stat">
      <span>{tx(label)}</span>
      <strong>{value}</strong>
    </div>
  );
}

function AccountProviderIcon({ providerType, size = 20 }: { providerType: string; size?: number }) {
  const icon = accountProviderIcon(providerType);
  return <ProviderIcon icon={icon.icon} color={icon.color} name={providerLabel(providerType)} size={size} />;
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

function IconAction({
  title,
  children,
  busy,
  danger,
  disabled,
  onClick,
}: {
  title: string;
  children: ReactNode;
  busy?: boolean;
  danger?: boolean;
  disabled?: boolean;
  onClick: () => void;
}) {
  const { tx } = useI18n();
  const translatedTitle = tx(title);
  return (
    <button
      className={danger ? "icon-button danger" : "icon-button"}
      type="button"
      title={translatedTitle}
      aria-label={translatedTitle}
      onClick={onClick}
      disabled={busy || disabled}
    >
      {busy ? <Loader2 size={15} /> : children}
    </button>
  );
}

function JsonPreview({ value }: { value: unknown }) {
  return <pre className="json-preview">{JSON.stringify(redactSecrets(value), null, 2)}</pre>;
}

function providerLabel(providerType: string): string {
  const labels: Record<string, string> = {
    claude: "Claude API",
    claude_auth: "Claude bearer relay",
    claude_oauth: "Claude OAuth",
    codex: "OpenAI/Codex",
    codex_oauth: "OpenAI OAuth",
    gemini: "Gemini API",
    gemini_cli: "Gemini OAuth/CLI",
    openrouter: "OpenRouter",
    github_copilot: "GitHub Copilot",
    deepseek_account: "DeepSeek Account",
    kiro_oauth: "Kiro OAuth",
    cursor_oauth: "Cursor OAuth",
    cursor_apikey: "Cursor API Key",
    antigravity_oauth: "Antigravity OAuth",
    agy_oauth: "Antigravity CLI",
    ollama_cloud: "Ollama Cloud",
    aws_bedrock: "AWS Bedrock",
    nvidia: "Nvidia",
    deepseek_api: "DeepSeek API Key",
  };
  return labels[providerType] || providerType.replace(/_/g, " ");
}

function accountProviderIcon(providerType: string): { icon?: string; color?: string } {
  const normalized = providerType
    .replace(/_oauth|_cli|_account|_apikey|_api_key|_auth|_cloud/g, " ")
    .replace(/_/g, " ");
  const inferred = inferIconForText(providerType, normalized, providerLabel(providerType));
  return { icon: inferred.icon, color: inferred.iconColor };
}

function credentialFlags(account: AccountRecord): string[] {
  const flags: string[] = [];
  if (account.accessToken) flags.push("access");
  if (account.refreshToken) flags.push("refresh");
  if (account.apiKey) flags.push("api key");
  if (account.idToken) flags.push("id token");
  return flags;
}

function accountRegressionBadges(
  account: AccountRecord,
  capability?: AccountManagerCapability,
): AccountRegressionBadge[] {
  return [
    loginRegressionBadge(capability),
    refreshRegressionBadge(account, capability),
    tokenRegressionBadge(account),
    quotaRegressionBadge(account, capability),
  ];
}

function loginRegressionBadge(capability?: AccountManagerCapability): AccountRegressionBadge {
  if (capability?.supportsStartLogin) {
    return { label: "login", value: "native", tone: "success" };
  }
  if (capability?.supportsImport) {
    return { label: "login", value: "import", tone: "warning" };
  }
  return { label: "login", value: "gated", tone: "warning" };
}

function refreshRegressionBadge(
  account: AccountRecord,
  capability?: AccountManagerCapability,
): AccountRegressionBadge {
  if (account.lastRefreshError) {
    return { label: "refresh", value: "error", tone: "danger" };
  }
  if (capability?.supportsRefresh && account.refreshToken) {
    return { label: "refresh", value: "ready", tone: "success" };
  }
  if (capability?.supportsRefresh) {
    return { label: "refresh", value: "no-token", tone: "warning" };
  }
  return { label: "refresh", value: "manual", tone: "warning" };
}

function tokenRegressionBadge(account: AccountRecord): AccountRegressionBadge {
  const expiry = normalizeTimestamp(account.expiresAt);
  if (expiry == null) {
    return { label: "token", value: account.accessToken || account.apiKey ? "no-expiry" : "missing", tone: account.accessToken || account.apiKey ? "warning" : "danger" };
  }
  const remaining = expiry - Date.now();
  if (remaining <= 0) return { label: "token", value: "expired", tone: "danger" };
  if (remaining <= 24 * 60 * 60 * 1000) return { label: "token", value: "soon", tone: "warning" };
  return { label: "token", value: "valid", tone: "success" };
}

function quotaRegressionBadge(
  account: AccountRecord,
  capability?: AccountManagerCapability,
): AccountRegressionBadge {
  if (!capability?.supportsQuota) {
    return { label: "quota", value: "gated", tone: "warning" };
  }
  if (!account.quota && account.quotaPercent == null) {
    return { label: "quota", value: "missing", tone: "warning" };
  }
  const refreshedAt = normalizeTimestamp(account.quotaRefreshedAt);
  if (refreshedAt == null) {
    return { label: "quota", value: "snapshot", tone: "warning" };
  }
  const age = Date.now() - refreshedAt;
  if (age <= 24 * 60 * 60 * 1000) {
    return { label: "quota", value: "fresh", tone: "success" };
  }
  if (age <= 7 * 24 * 60 * 60 * 1000) {
    return { label: "quota", value: "aged", tone: "warning" };
  }
  return { label: "quota", value: "stale", tone: "danger" };
}

function normalizeTimestamp(value?: number | null): number | null {
  if (value == null || !Number.isFinite(value)) return null;
  return value < 10_000_000_000 ? value * 1000 : value;
}

function formatQuotaPercent(account: AccountRecord): string {
  if (account.quotaPercent != null) return `${account.quotaPercent.toFixed(1)}%`;
  const utilization = account.quota?.tiers?.find((tier) => tier.utilization != null)?.utilization;
  return utilization == null ? "-" : `${utilization.toFixed(1)}%`;
}

function quotaTierSummary(account: AccountRecord): string | null {
  const tiers = account.quota?.tiers || [];
  if (!tiers.length) return null;
  return tiers
    .slice(0, 2)
    .map((tier) => {
      const usage = tier.used != null && tier.limit != null ? ` ${tier.used}/${tier.limit}` : "";
      const unit = tier.unit ? ` ${tier.unit}` : "";
      return `${tier.name}${usage}${unit}`;
    })
    .join("; ");
}

function bankedResetSummary(account: AccountRecord): BankedResetSummary | null {
  const source =
    valueAt(account.quota?.extraUsage, ["bankedReset", "codexBankedReset"]) ??
    valueAt(account.raw, [
      "bankedReset",
      "banked_reset",
      "codexBankedReset",
      "codex_banked_reset",
      "rateLimitResetCredits",
      "rate_limit_reset_credits",
    ]);
  if (!source) return null;
  const record = asRecord(source);
  const credits = bankedResetCredits(source);
  const availableCount =
    numberValue(record?.availableCount) ??
    numberValue(record?.available_count) ??
    numberValue(record?.available) ??
    credits.filter((credit) => String(credit.status || "available").toLowerCase() === "available").length;
  const nextExpiresAt =
    stringValue(record?.nextExpiresAt) ||
    stringValue(record?.next_expires_at) ||
    nextCreditExpiry(credits);
  return {
    account,
    availableCount,
    nextExpiresAt,
    readOnly: Boolean(record?.readOnly ?? record?.read_only ?? true),
    source: stringValue(record?.source),
    queriedAt: numberValue(record?.queriedAt ?? record?.queried_at),
    credits,
    raw: source,
  };
}

function bankedResetCredits(source: unknown): BankedResetCredit[] {
  const record = asRecord(source);
  const rawCredits =
    arrayValue(record?.credits) ??
    arrayValue(record?.remainingCredits) ??
    arrayValue(record?.remaining_credits) ??
    arrayValue(source) ??
    [];
  return rawCredits
    .map((item) => asRecord(item))
    .filter((item): item is Record<string, unknown> => Boolean(item))
    .map((item) => ({
      ...item,
      id: stringValue(item.id),
      status: stringValue(item.status),
      grantedAt: stringValue(item.grantedAt ?? item.granted_at),
      expiresAt: stringValue(item.expiresAt ?? item.expires_at),
      title: stringValue(item.title),
      description: stringValue(item.description),
    }));
}

function nextCreditExpiry(credits: BankedResetCredit[]): string | null {
  const candidates = credits
    .filter((credit) => String(credit.status || "available").toLowerCase() === "available")
    .map((credit) => credit.expiresAt)
    .filter((value): value is string => Boolean(value))
    .map((value) => ({ value, ms: Date.parse(value) }))
    .filter((item) => Number.isFinite(item.ms))
    .sort((left, right) => left.ms - right.ms);
  return candidates[0]?.value || null;
}

function formatTime(value?: number | null): string {
  if (!value) return "-";
  const millis = value < 10_000_000_000 ? value * 1000 : value;
  const date = new Date(millis);
  if (Number.isNaN(date.getTime())) return "-";
  return new Intl.DateTimeFormat(undefined, {
    dateStyle: "medium",
    timeStyle: "short",
  }).format(date);
}

function formatDateish(value?: string | number | null): string {
  if (!value) return "-";
  if (typeof value === "number") return formatTime(value);
  const parsed = Date.parse(value);
  if (!Number.isFinite(parsed)) return value;
  return formatTime(parsed);
}

function valueAt(source: unknown, keys: string[]): unknown {
  const record = asRecord(source);
  if (!record) return undefined;
  for (const key of keys) {
    if (record[key] !== undefined) return record[key];
  }
  return undefined;
}

function asRecord(value: unknown): Record<string, unknown> | null {
  return value && typeof value === "object" && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : null;
}

function arrayValue(value: unknown): unknown[] | null {
  return Array.isArray(value) ? value : null;
}

function stringValue(value: unknown): string | null {
  return typeof value === "string" && value.trim() ? value : null;
}

function numberValue(value: unknown): number | null {
  if (typeof value === "number" && Number.isFinite(value)) return value;
  if (typeof value === "string" && value.trim()) {
    const parsed = Number(value);
    return Number.isFinite(parsed) ? parsed : null;
  }
  return null;
}

function redactSecrets(value: unknown): unknown {
  if (Array.isArray(value)) {
    if (value.length === 2 && typeof value[0] === "string" && isSecretKey(value[0])) {
      return [value[0], value[1] == null || value[1] === "" ? value[1] : "[REDACTED]"];
    }
    return value.map(redactSecrets);
  }
  if (!value || typeof value !== "object") return value;
  const redacted: Record<string, unknown> = {};
  for (const [key, item] of Object.entries(value as Record<string, unknown>)) {
    if (isSecretKey(key)) {
      redacted[key] = item == null || item === "" ? item : "[REDACTED]";
    } else {
      redacted[key] = redactSecrets(item);
    }
  }
  return redacted;
}

function isSecretKey(key: string): boolean {
  const lower = key.toLowerCase();
  return (
    lower.includes("token") ||
    lower.includes("secret") ||
    lower.includes("apikey") ||
    lower.includes("api_key") ||
    lower === "code" ||
    lower.includes("codeverifier") ||
    lower.includes("code_verifier") ||
    lower === "authorization"
  );
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
