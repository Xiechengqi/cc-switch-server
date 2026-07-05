import { RefreshCw, Upload, UserRound } from "lucide-react";
import { ReactNode, useCallback, useEffect, useMemo, useState } from "react";

import { LoadingBlock } from "@/components/LoadingBlock";
import {
  AccountImportModal,
  accountInputFromDraft,
  createAccountImportDraft,
  type AccountImportDraft,
} from "@/components/settings/AccountImportModal";
import {
  AccountGroup,
  AuthCenterOverview,
  type AccountAction,
  type AccountDetail,
} from "@/components/settings/AuthCenterAccounts";
import { DeviceFlowPanel, OAuthPreviewPanel } from "@/components/settings/AuthCenterFlows";
import { CapabilityPanel, CodexBankedResetPanel } from "@/components/settings/AuthCenterSidePanels";
import { providerLabel } from "@/components/settings/accountDisplay";
import {
  AccountImportTemplate,
  AccountManagerCapability,
  AccountRecord,
  deleteAccount,
  loadAccountQuota,
  loadAccountRefreshPlan,
  loadAuthCenterPanelData,
  refreshAccount,
  upsertAccount,
} from "@/lib/api";
import { useI18n } from "@/lib/i18n";

interface AuthCenterPanelState {
  accounts: AccountRecord[];
  capabilities: AccountManagerCapability[];
  templates: AccountImportTemplate[];
}

export function AuthCenterPanel({ embedded = false }: { embedded?: boolean } = {}) {
  const { t, tx } = useI18n();
  const [data, setData] = useState<AuthCenterPanelState>({
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
      setData(await loadAuthCenterPanelData());
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
    action: AccountAction,
  ) {
    const key = `${account.id}:${action}`;
    setBusyId(key);
    setError(null);
    try {
      if (action === "delete") {
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
    <div className={embedded ? "auth-center-panel embedded" : "auth-center-panel"}>
      {!embedded && (
        <div className="provider-toolbar">
          <div className="provider-toolbar-status">
            <span>{t("server.accounts.importedCredentials", { count: data.accounts.length })}</span>
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
      )}

      {embedded && (
        <div className="auth-center-inline-actions">
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
      )}

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

      <AuthCenterOverview
        providerTypes={providerTypes}
        accounts={data.accounts}
        capabilitiesByType={capabilitiesByType}
        templatesByType={templatesByType}
        loading={loading}
        onImport={(providerType) => setImportDraft(createAccountImportDraft(providerType))}
      />

      <div className="accounts-layout">
        <section className="accounts-main">
          <div className="section-heading">
            <h2>{t("server.accounts.importedAccounts")}</h2>
            <span>{loading ? t("common.loading") : t("server.accounts.providerTypes", { count: providerTypes.length })}</span>
          </div>
          {loading ? (
            <LoadingBlock label="server.accounts.loading" />
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

function AccountStat({ label, value }: { label: string; value: ReactNode }) {
  const { tx } = useI18n();
  return (
    <div className="account-stat">
      <span>{tx(label)}</span>
      <strong>{value}</strong>
    </div>
  );
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
