import { FolderOpen, Monitor, Moon, Palette, ShieldCheck, Sun } from "lucide-react";
import type { ReactNode } from "react";

import type { SettingsPageData } from "@/lib/server-legacy-api";
import type { WebRuntimeContext } from "@/lib/runtime";
import { useI18n } from "@/lib/i18n";
import { KeyValue } from "@/components/KeyValue";
import { StatusPill } from "@/components/StatusPill";
import { useTheme } from "@/components/theme-provider";
import { SectionHeader } from "@/components/settings/SettingsSectionHeader";
import { formatTime } from "@/components/settings/settingsDrafts";

export function ThemeSettingsPanel() {
  const { theme, setTheme } = useTheme();
  const { tx } = useI18n();
  const options: Array<{ value: "light" | "dark" | "system"; label: string; icon: ReactNode }> = [
    { value: "light", label: "Light", icon: <Sun size={16} /> },
    { value: "dark", label: "Dark", icon: <Moon size={16} /> },
    { value: "system", label: "System", icon: <Monitor size={16} /> },
  ];
  return (
    <section className="settings-card">
      <SectionHeader
        icon={<Palette size={17} />}
        title={tx("Theme")}
        subtitle={tx("Choose the desktop color mode for this browser")}
      />
      <div className="theme-option-grid" role="radiogroup" aria-label={tx("Theme")}>
        {options.map((option) => (
          <button
            key={option.value}
            className={theme === option.value ? "theme-option active" : "theme-option"}
            type="button"
            role="radio"
            aria-checked={theme === option.value}
            onClick={() => setTheme(option.value)}
          >
            {option.icon}
            <span>{tx(option.label)}</span>
          </button>
        ))}
      </div>
    </section>
  );
}

export function DirectoryPanel({ runtimeContext }: { runtimeContext: WebRuntimeContext | null }) {
  const { tx } = useI18n();
  const configDir = runtimeContext?.runtime?.configDir || "~/.cc-switch-server";
  const webDistDir = runtimeContext?.runtime?.webDistDir || tx("embedded web assets");
  const embeddedAssets = runtimeContext?.runtime?.embeddedWebAssets ?? "-";
  const files = [
    "server.json",
    "providers.json",
    "universal-providers.json",
    "accounts.json",
    "shares.json",
    "usage-logs.json",
    "usage-logs.jsonl",
    "usage-rollups.json",
    "model-pricing.json",
    "failover.json",
    "tunnels.json",
    "email-auth.json",
  ];
  return (
    <section className="settings-card wide settings-directory-card">
      <SectionHeader
        icon={<FolderOpen size={17} />}
        title={tx("Directory")}
        subtitle={tx("Server data and embedded web asset locations")}
      />
      <div className="settings-policy-grid">
        <KeyValue label="config dir" value={configDir} />
        <KeyValue label="web dist" value={webDistDir} />
        <KeyValue label="embedded assets" value={embeddedAssets} />
        <KeyValue label="mode" value={runtimeContext?.mode || "-"} />
      </div>
      <div className="settings-directory-list">
        {files.map((file) => (
          <div className="settings-directory-row" key={file}>
            <span>{file}</span>
            <code>{joinPath(configDir, file)}</code>
          </div>
        ))}
      </div>
    </section>
  );
}

export function SettingsReadinessPanel({ data }: { data: SettingsPageData }) {
  const { t } = useI18n();
  const items = settingsReadinessItems(data);
  return (
    <section className="settings-card wide settings-readiness-panel">
      <SectionHeader
        icon={<ShieldCheck size={17} />}
        title={t("server.settings.runtimeReadiness")}
        subtitle={t("server.settings.readinessSubtitle", {
          ready: items.filter((item) => item.tone === "success").length,
          total: items.length,
        })}
      />
      <div className="settings-readiness-grid">
        {items.map((item) => (
          <div className="settings-readiness-item" key={item.label}>
            <div>
              <strong>{item.label}</strong>
              <span>{item.detail}</span>
            </div>
            <StatusPill tone={item.tone}>{item.value}</StatusPill>
          </div>
        ))}
      </div>
    </section>
  );
}

export function SettingsOverviewStrip({ data }: { data: SettingsPageData }) {
  const { t, tx } = useI18n();
  const ownerEmail = data.config?.ownerEmail;
  const tunnelStatus = data.tunnel.runtimeStatus?.status || data.tunnel.tunnelStatus || "-";
  const items: Array<{ label: string; value: ReactNode; detail: string; tone: "success" | "warning" | "danger" }> = [
    {
      label: t("server.settings.owner"),
      value: ownerEmail || "-",
      detail: ownerEmail ? tx("owner-bound") : tx("owner pending"),
      tone: ownerEmail ? "success" : "warning",
    },
    {
      label: t("server.settings.router"),
      value: data.routerStatus.registered ? tx("registered") : tx("not registered"),
      detail: data.routerStatus.lastError || formatTime(data.routerStatus.lastHeartbeatMs),
      tone: data.routerStatus.lastError ? "danger" : data.routerStatus.registered ? "success" : "warning",
    },
    {
      label: t("server.settings.tunnel"),
      value: tunnelStatus,
      detail: data.tunnel.runtimeStatus?.tunnelUrl || data.tunnel.tunnelSubdomain || "-",
      tone: diagnosticTone(tunnelStatus, data.tunnel.runtimeStatus?.lastError),
    },
    {
      label: t("server.settings.pendingLogs"),
      value: data.routerStatus.pendingRequestLogSync,
      detail: tx("request log sync backlog"),
      tone: data.routerStatus.pendingRequestLogSync > 0 ? "warning" : "success",
    },
    {
      label: t("server.settings.backups"),
      value: data.backups.length,
      detail: data.backups[0] ? formatTime(Math.max(...data.backups.map((backup) => backup.createdAtMs))) : tx("no snapshots"),
      tone: data.backups.length ? "success" : "warning",
    },
  ];
  return (
    <div className="settings-overview-strip">
      {items.map((item) => (
        <article className="settings-overview-card" key={item.label}>
          <div>
            <span>{item.label}</span>
            <strong>{item.value}</strong>
            <small>{item.detail}</small>
          </div>
          <StatusPill tone={item.tone}>{item.tone}</StatusPill>
        </article>
      ))}
    </div>
  );
}

function joinPath(dir: string, file: string): string {
  if (!dir || dir === "~/.cc-switch-server") return `~/.cc-switch-server/${file}`;
  return `${dir.replace(/\/+$/, "")}/${file}`;
}

function diagnosticTone(status?: string | null, error?: string | null): "success" | "warning" | "danger" {
  if (error) return "danger";
  const normalized = status?.trim().toLowerCase();
  if (!normalized || ["disabled", "stopped", "ended", "unknown"].includes(normalized)) return "warning";
  if (["error", "failed", "expired", "exhausted"].includes(normalized)) return "danger";
  return "success";
}

function settingsReadinessItems(data: SettingsPageData): Array<{
  label: string;
  value: string;
  detail: string;
  tone: "success" | "warning" | "danger";
}> {
  const ownerEmail = data.config?.ownerEmail;
  const latestBackup = [...data.backups].sort((left, right) => right.createdAtMs - left.createdAtMs)[0];
  const latestBackupAge = latestBackup ? Date.now() - latestBackup.createdAtMs : Number.POSITIVE_INFINITY;
  const tunnelErrors = data.diagnostics.tunnels.filter((tunnel) => tunnel.lastError).length;
  const shareErrors = data.diagnostics.shareSync.filter((share) => share.routerLastSyncError).length;
  const diagnosticsIssues = tunnelErrors + shareErrors + (data.routerStatus.lastError ? 1 : 0);
  return [
    {
      label: "setup",
      value: ownerEmail ? "owner" : "pending",
      detail: ownerEmail || "no owner email",
      tone: ownerEmail ? "success" : "warning",
    },
    {
      label: "login",
      value: "email code",
      detail: ownerEmail ? "owner-bound" : "owner pending",
      tone: ownerEmail ? "success" : "warning",
    },
    {
      label: "router",
      value: data.routerStatus.registered ? "registered" : "local",
      detail: data.routerStatus.lastError || formatTime(data.routerStatus.lastHeartbeatMs),
      tone: data.routerStatus.lastError ? "danger" : data.routerStatus.registered ? "success" : "warning",
    },
    {
      label: "backup",
      value: latestBackup ? "ready" : "missing",
      detail: latestBackup ? formatTime(latestBackup.createdAtMs) : "no snapshots",
      tone: latestBackupAge <= 24 * 60 * 60 * 1000 ? "success" : latestBackup ? "warning" : "danger",
    },
    {
      label: "diagnostics",
      value: diagnosticsIssues ? `${diagnosticsIssues} issue` : "clean",
      detail: `${data.diagnostics.tunnels.length} tunnels / ${data.diagnostics.shareSync.length} shares`,
      tone: diagnosticsIssues ? "warning" : "success",
    },
  ];
}
