import {
  AlertTriangle,
  Cable,
  CheckCircle2,
  Cloud,
  Loader2,
  Network,
  RefreshCw,
  RotateCcw,
  Save,
} from "lucide-react";
import type { FormEvent, ReactNode } from "react";

import { TextField } from "@/components/TextField";
import { SectionHeader } from "@/components/settings/SettingsSectionHeader";
import { RouterFacts, TunnelStatus } from "@/components/settings/SettingsStatusPanels";
import {
  routerState,
  type ProxyDraft,
  type RouterDraft,
  type TunnelDraft,
} from "@/components/settings/settingsDrafts";
import type { RouterConfigView, RouterStatusResponse, SettingsPageData } from "@/lib/api";
import { useI18n } from "@/lib/i18n";

export function ProxySettingsPanel({
  maskedUrl,
  draft,
  busy,
  onDraftChange,
  onSave,
}: {
  maskedUrl?: string | null;
  draft: ProxyDraft;
  busy: string | null;
  onDraftChange: (draft: ProxyDraft) => void;
  onSave: (event: FormEvent) => void;
}) {
  const { t } = useI18n();
  return (
    <section className="settings-card">
      <SectionHeader
        icon={<Cloud size={17} />}
        title={t("server.settings.upstreamProxy")}
        subtitle={maskedUrl || t("server.settings.notConfigured")}
      />
      <form className="settings-form" onSubmit={onSave}>
        <label className="toggle-row">
          <input
            type="checkbox"
            checked={draft.followSystemProxy}
            onChange={(event) => onDraftChange({ ...draft, followSystemProxy: event.target.checked })}
          />
          <span>{t("server.settings.followSystemProxy")}</span>
        </label>
        <label>
          <span>{t("server.settings.newProxyUrl")}</span>
          <input
            value={draft.url}
            placeholder="http://127.0.0.1:7890"
            onChange={(event) => onDraftChange({ ...draft, url: event.target.value })}
          />
        </label>
        <label className="toggle-row">
          <input
            type="checkbox"
            checked={draft.clear}
            onChange={(event) => onDraftChange({ ...draft, clear: event.target.checked })}
          />
          <span>{t("server.settings.clearConfiguredUrl")}</span>
        </label>
        <SettingsFormFooter busy={busy === "proxy-save"} label={t("server.settings.saveProxy")} />
      </form>
    </section>
  );
}

export function RouterSettingsPanel({
  router,
  status,
  draft,
  busy,
  onDraftChange,
  onSave,
  onRegister,
  onHeartbeat,
  onBatchSync,
}: {
  router?: RouterConfigView;
  status?: RouterStatusResponse;
  draft: RouterDraft;
  busy: string | null;
  onDraftChange: (draft: RouterDraft) => void;
  onSave: (event: FormEvent) => void;
  onRegister: () => void;
  onHeartbeat: () => void;
  onBatchSync: () => void;
}) {
  const { t } = useI18n();
  return (
    <section className="settings-card wide">
      <SectionHeader icon={<Network size={17} />} title={t("server.settings.router")} subtitle={routerState(status)} />
      <form className="settings-form settings-form-grid" onSubmit={onSave}>
        <TextField label={t("server.auth.routerUrl")} value={draft.url} onChange={(value) => onDraftChange({ ...draft, url: value })} />
        <TextField label={t("server.settings.apiBase")} value={draft.apiBase} onChange={(value) => onDraftChange({ ...draft, apiBase: value })} />
        <TextField label={t("server.settings.domain")} value={draft.domain} onChange={(value) => onDraftChange({ ...draft, domain: value })} />
        <TextField label={t("server.settings.region")} value={draft.region} onChange={(value) => onDraftChange({ ...draft, region: value })} />
        <TextField label={t("server.settings.sshHost")} value={draft.sshHost} onChange={(value) => onDraftChange({ ...draft, sshHost: value })} />
        <TextField label={t("server.settings.sshUser")} value={draft.sshUser} onChange={(value) => onDraftChange({ ...draft, sshUser: value })} />
        <label className="toggle-row">
          <input
            type="checkbox"
            checked={draft.custom}
            onChange={(event) => onDraftChange({ ...draft, custom: event.target.checked })}
          />
          <span>{t("server.settings.customRouter")}</span>
        </label>
        <SettingsFormFooter busy={busy === "router-save"} label={t("server.settings.saveRouter")} />
      </form>
      <div className="settings-actions">
        <SettingsActionButton label={t("server.settings.register")} icon={<CheckCircle2 size={15} />} busy={busy === "router-register"} onClick={onRegister} />
        <SettingsActionButton label={t("server.settings.heartbeat")} icon={<RefreshCw size={15} />} busy={busy === "router-heartbeat"} onClick={onHeartbeat} />
        <SettingsActionButton label={t("server.settings.batchSync")} icon={<RotateCcw size={15} />} busy={busy === "router-sync"} onClick={onBatchSync} />
      </div>
      <RouterFacts router={router} status={status} />
    </section>
  );
}

export function TunnelSettingsPanel({
  tunnel,
  draft,
  busy,
  clientTunnelRunning,
  onDraftChange,
  onSave,
  onClaim,
  onStart,
  onStop,
}: {
  tunnel?: SettingsPageData["tunnel"];
  draft: TunnelDraft;
  busy: string | null;
  clientTunnelRunning: boolean;
  onDraftChange: (draft: TunnelDraft) => void;
  onSave: (event: FormEvent) => void;
  onClaim: () => void;
  onStart: () => void;
  onStop: () => void;
}) {
  const { t } = useI18n();
  return (
    <section className="settings-card">
      <SectionHeader
        icon={<Cable size={17} />}
        title={t("server.settings.clientTunnel")}
        subtitle={tunnel?.runtimeStatus?.tunnelUrl || tunnel?.tunnelSubdomain || "-"}
      />
      <form className="settings-form" onSubmit={onSave}>
        <TextField label={t("server.settings.subdomain")} value={draft.tunnelSubdomain} onChange={(value) => onDraftChange({ ...draft, tunnelSubdomain: value })} />
        <TextField label={t("server.settings.status")} value={draft.tunnelStatus} onChange={(value) => onDraftChange({ ...draft, tunnelStatus: value })} />
        <SettingsFormFooter busy={busy === "tunnel-save"} label={t("server.settings.saveTunnel")} />
      </form>
      <div className="settings-actions">
        <SettingsActionButton label={t("server.settings.claim")} icon={<CheckCircle2 size={15} />} busy={busy === "tunnel-claim"} onClick={onClaim} />
        <SettingsActionButton
          label={t("server.settings.start")}
          icon={<Cable size={15} />}
          busy={busy === "tunnel-start"}
          disabled={clientTunnelRunning}
          onClick={onStart}
        />
        <SettingsActionButton
          label={t("server.settings.stop")}
          icon={<AlertTriangle size={15} />}
          busy={busy === "tunnel-stop"}
          disabled={!clientTunnelRunning}
          onClick={onStop}
        />
      </div>
      <TunnelStatus status={tunnel?.runtimeStatus} />
    </section>
  );
}

function SettingsFormFooter({ busy, label }: { busy: boolean; label: string }) {
  const { tx } = useI18n();
  return (
    <button className="primary-button" type="submit" disabled={busy}>
      {busy ? <Loader2 size={15} /> : <Save size={15} />}
      <span>{tx(label)}</span>
    </button>
  );
}

function SettingsActionButton({
  label,
  icon,
  busy,
  disabled,
  onClick,
}: {
  label: string;
  icon: ReactNode;
  busy: boolean;
  disabled?: boolean;
  onClick: () => void;
}) {
  const { tx } = useI18n();
  return (
    <button className="secondary-button" type="button" onClick={onClick} disabled={busy || disabled}>
      {busy ? <Loader2 size={15} /> : icon}
      <span>{tx(label)}</span>
    </button>
  );
}
