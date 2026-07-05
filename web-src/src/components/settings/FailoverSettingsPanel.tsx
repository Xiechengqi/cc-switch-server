import { Loader2, Save, Shuffle } from "lucide-react";
import type { FormEvent } from "react";

import { KeyValue } from "@/components/KeyValue";
import { StatusPill } from "@/components/StatusPill";
import { SectionHeader } from "@/components/settings/SettingsSectionHeader";
import { APP_KINDS, appLabel, type FailoverDraft } from "@/components/settings/settingsDrafts";
import type { AppKind, FailoverSnapshot, StoredProvider } from "@/lib/api";
import { useI18n } from "@/lib/i18n";

export function FailoverSettingsPanel({
  snapshot,
  providers,
  drafts,
  busy,
  onDraftChange,
  onSave,
}: {
  snapshot: FailoverSnapshot;
  providers: StoredProvider[];
  drafts: Record<AppKind, FailoverDraft>;
  busy: string | null;
  onDraftChange: (app: AppKind, draft: FailoverDraft) => void;
  onSave: (app: AppKind, event: FormEvent) => void;
}) {
  const { tx } = useI18n();
  const totalQueued = APP_KINDS.reduce((sum, app) => sum + (snapshot.apps[app]?.providerQueue.length || 0), 0);
  const openBreakers = snapshot.breakers.filter((breaker) => breaker.state !== "closed").length;
  return (
    <section className="settings-card wide settings-failover-card">
      <SectionHeader
        icon={<Shuffle size={17} />}
        title={tx("Failover")}
        subtitle={tx("Automatic provider queue and circuit breaker strategy")}
      />
      <div className="settings-policy-grid">
        <KeyValue label="enabled apps" value={APP_KINDS.filter((app) => snapshot.apps[app]?.enabled).length} />
        <KeyValue label="queued providers" value={totalQueued} />
        <KeyValue label="open breakers" value={openBreakers} />
      </div>
      <div className="failover-settings-grid">
        {APP_KINDS.map((app) => {
          const draft = drafts[app];
          const appProviders = providers.filter((item) => item.app === app);
          const providerNames = new Map(appProviders.map((item) => [item.provider.id, item.provider.name || item.provider.id]));
          const breakers = snapshot.breakers.filter((breaker) => breaker.app === app && breaker.state !== "closed");
          return (
            <form className="failover-settings-app" key={app} onSubmit={(event) => onSave(app, event)}>
              <header>
                <div>
                  <strong>{appLabel(app)}</strong>
                  <span>{tx("{{count}} providers available", { count: appProviders.length })}</span>
                </div>
                <StatusPill tone={draft.enabled ? "success" : "warning"}>
                  {draft.enabled ? tx("enabled") : tx("disabled")}
                </StatusPill>
              </header>
              <label className="toggle-row">
                <input
                  type="checkbox"
                  checked={draft.enabled}
                  onChange={(event) => onDraftChange(app, { ...draft, enabled: event.target.checked })}
                />
                <span>{tx("Enable automatic failover")}</span>
              </label>
              <div className="failover-number-grid">
                <label>
                  <span>{tx("Failure threshold")}</span>
                  <input
                    type="number"
                    min={1}
                    step={1}
                    value={draft.failureThreshold}
                    onChange={(event) => onDraftChange(app, { ...draft, failureThreshold: event.target.value })}
                  />
                </label>
                <label>
                  <span>{tx("Open duration seconds")}</span>
                  <input
                    type="number"
                    min={1}
                    step={1}
                    value={draft.openDurationSeconds}
                    onChange={(event) => onDraftChange(app, { ...draft, openDurationSeconds: event.target.value })}
                  />
                </label>
                <label>
                  <span>{tx("Half-open probes")}</span>
                  <input
                    type="number"
                    min={1}
                    step={1}
                    value={draft.halfOpenMaxProbes}
                    onChange={(event) => onDraftChange(app, { ...draft, halfOpenMaxProbes: event.target.value })}
                  />
                </label>
              </div>
              <div className="failover-queue-summary">
                <div className="section-title-row compact-title">
                  <Shuffle size={15} />
                  <h3>{tx("Provider queue")}</h3>
                </div>
                {draft.providerQueue.length ? (
                  <ol>
                    {draft.providerQueue.map((providerId, index) => (
                      <li key={providerId}>
                        <span>{tx("P{{priority}}", { priority: index + 1 })}</span>
                        <strong>{providerNames.get(providerId) || providerId}</strong>
                        <code>{providerId}</code>
                      </li>
                    ))}
                  </ol>
                ) : (
                  <p>{tx("Queue is empty. Add providers from provider cards.")}</p>
                )}
              </div>
              <div className="failover-breaker-summary">
                <span>{tx("Breakers")}</span>
                {breakers.length ? (
                  breakers.slice(0, 4).map((breaker) => (
                    <StatusPill key={breaker.providerId} tone={breaker.state === "open" ? "danger" : "warning"}>
                      {providerNames.get(breaker.providerId) || breaker.providerId}: {breaker.state}
                    </StatusPill>
                  ))
                ) : (
                  <StatusPill tone="success">{tx("closed")}</StatusPill>
                )}
              </div>
              <FailoverFormFooter busy={busy === `failover-save:${app}`} label={tx("Save failover")} />
            </form>
          );
        })}
      </div>
    </section>
  );
}

function FailoverFormFooter({ busy, label }: { busy: boolean; label: string }) {
  const { tx } = useI18n();
  return (
    <button className="primary-button" type="submit" disabled={busy}>
      {busy ? <Loader2 size={15} /> : <Save size={15} />}
      <span>{tx(label)}</span>
    </button>
  );
}
