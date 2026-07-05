import { Archive, CheckCircle2, Copy, KeyRound, Loader2, Mail, Network, Save } from "lucide-react";
import type { FormEvent, ReactNode } from "react";

import type { BackupManifest, RouterDiagnosticsResponse } from "@/lib/api";
import { useI18n } from "@/lib/i18n";
import { AuthCenterPanel } from "@/components/settings/AuthCenterPanel";
import { SectionHeader } from "@/components/settings/SettingsSectionHeader";
import {
  BackupPolicySummary,
  BackupSnapshotGrid,
  Diagnostics,
  DiagnosticsSummary,
} from "@/components/settings/SettingsStatusPanels";
import type { EmailDraft } from "@/components/settings/settingsDrafts";
import { TextField } from "@/components/TextField";

export function AuthSettingsPanel({
  emailDraft,
  apiToken,
  apiTokenCopyStatus,
  busy,
  onEmailDraftChange,
  onRotateToken,
  onCopyToken,
  onRequestCode,
  onVerifyCode,
}: {
  emailDraft: EmailDraft;
  apiToken: string | null;
  apiTokenCopyStatus: { tone: "success" | "warning"; message: string } | null;
  busy: string | null;
  onEmailDraftChange: (draft: EmailDraft) => void;
  onRotateToken: () => void;
  onCopyToken: () => void;
  onRequestCode: () => void;
  onVerifyCode: () => void;
}) {
  const { t, tx } = useI18n();
  return (
    <>
      <section className="settings-card">
        <SectionHeader icon={<KeyRound size={17} />} title={t("server.settings.auth")} subtitle={t("server.settings.authSubtitle")} />
        <div className="settings-actions">
          <SettingsActionButton
            label={t("server.settings.rotateApiToken")}
            icon={<KeyRound size={15} />}
            busy={busy === "api-token"}
            onClick={onRotateToken}
          />
          {apiToken && (
            <button className="secondary-button" type="button" onClick={onCopyToken}>
              <Copy size={15} />
              <span>{t("server.settings.copyToken")}</span>
            </button>
          )}
        </div>
        {apiTokenCopyStatus && <div className={`connect-copy-status ${apiTokenCopyStatus.tone}`}>{apiTokenCopyStatus.message}</div>}
        {apiToken && <pre className="settings-secret-preview">{apiToken}</pre>}
        <div className="settings-form">
          <TextField
            label={t("server.auth.ownerEmail")}
            value={emailDraft.email}
            onChange={(value) => onEmailDraftChange({ ...emailDraft, email: value })}
          />
          <TextField
            label={t("server.settings.verificationCode")}
            value={emailDraft.code}
            onChange={(value) => onEmailDraftChange({ ...emailDraft, code: value })}
          />
          <div className="settings-actions">
            <SettingsActionButton
              label={t("server.settings.requestCode")}
              icon={<Mail size={15} />}
              busy={busy === "email-request"}
              onClick={onRequestCode}
            />
            <SettingsActionButton
              label={t("server.settings.verify")}
              icon={<CheckCircle2 size={15} />}
              busy={busy === "email-verify"}
              onClick={onVerifyCode}
            />
          </div>
        </div>
      </section>
      <section className="settings-card wide settings-accounts-card">
        <SectionHeader
          icon={<KeyRound size={17} />}
          title={t("server.nav.accounts")}
          subtitle={tx("OAuth accounts and quota tools")}
        />
        <AuthCenterPanel embedded />
      </section>
    </>
  );
}

export function BackupSettingsPanel({
  backups,
  backupReason,
  busy,
  onBackupReasonChange,
  onCreateBackup,
  onRestore,
}: {
  backups: BackupManifest[];
  backupReason: string;
  busy: string | null;
  onBackupReasonChange: (value: string) => void;
  onCreateBackup: (event: FormEvent) => void;
  onRestore: (backup: BackupManifest) => void;
}) {
  const { t } = useI18n();
  return (
    <section className="settings-card wide">
      <SectionHeader icon={<Archive size={17} />} title={t("server.settings.backup")} subtitle={t("server.settings.backupSubtitle")} />
      <BackupPolicySummary backups={backups} />
      <form className="settings-form backup-create-row" onSubmit={onCreateBackup}>
        <label>
          <span>{t("server.settings.reason")}</span>
          <input value={backupReason} onChange={(event) => onBackupReasonChange(event.target.value)} />
        </label>
        <SettingsFormFooter busy={busy === "backup-create"} label={t("server.settings.createBackup")} />
      </form>
      <BackupSnapshotGrid backups={backups} busy={busy} onRestore={onRestore} />
    </section>
  );
}

export function DiagnosticsSettingsPanel({ diagnostics }: { diagnostics?: RouterDiagnosticsResponse }) {
  const { t } = useI18n();
  return (
    <section className="settings-card wide">
      <SectionHeader
        icon={<Network size={17} />}
        title={t("server.settings.diagnostics")}
        subtitle={t("server.settings.diagnosticsSubtitle", {
          tunnels: diagnostics?.tunnels.length || 0,
          shares: diagnostics?.shareSync.length || 0,
        })}
      />
      <DiagnosticsSummary diagnostics={diagnostics} />
      <Diagnostics diagnostics={diagnostics} />
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
