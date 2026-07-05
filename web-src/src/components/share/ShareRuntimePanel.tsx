import type { ReactNode } from "react";

import { JsonPreview } from "@/components/JsonPreview";
import { StatusPill } from "@/components/StatusPill";
import type { AppKind, ShareRecord } from "@/lib/api";
import { useI18n } from "@/lib/i18n";
import { appLabel, formatTime } from "@/components/share/shareDisplay";

const apps: AppKind[] = ["claude", "codex", "gemini"];

interface ModelHealthView {
  app: AppKind;
  requestedModel: string;
  actualModel: string;
  status: string;
  statusCode?: number;
  latencyMs?: number;
  checkedAt?: number;
  providerId?: string;
  providerName?: string;
}

export function ShareRuntimePanel({ share }: { share: ShareRecord }) {
  const { tx } = useI18n();
  const snapshot = asRecord(share.runtimeSnapshot);
  if (!snapshot) {
    return (
      <section className="share-runtime-panel">
        <div className="compact-empty">
          <span>{tx("No runtime snapshot")}</span>
        </div>
      </section>
    );
  }
  const health = asRecord(snapshot.health);
  const lastRequest = asRecord(snapshot.lastRequest);
  const upstreamProvider = asRecord(snapshot.upstreamProvider);
  const availability = asRecord(snapshot.appAvailability);
  const appProviders = asRecord(snapshot.appProviders);
  const appRuntimes = asRecord(snapshot.appRuntimes);
  const healthStatus = providerHealthStatus(health);
  const modelHealth = modelHealthResults(snapshot.modelHealth);
  return (
    <section className="share-runtime-panel">
      <div className="share-runtime-grid">
        <RuntimeValue label="updated" value={formatTime(numberValue(snapshot.updatedAtMs))} />
        <RuntimeValue label="provider" value={stringValue(snapshot.providerName) || share.providerType || "-"} />
        <RuntimeValue label="account" value={stringValue(snapshot.accountEmail) || stringValue(upstreamProvider?.accountEmail) || share.accountEmail || "-"} />
        <RuntimeValue label="plan" value={stringValue(snapshot.subscriptionLevel) || stringValue(upstreamProvider?.subscriptionLevel) || share.subscriptionLevel || "-"} />
        <RuntimeValue label="quota" value={formatPercent(numberValue(snapshot.quotaPercent) ?? share.quotaPercent)} />
        <RuntimeValue label="health" value={healthStatus.label} tone={healthStatus.tone} />
      </div>

      {lastRequest && (
        <div className="runtime-mini-row">
          <span>{tx("last request")}</span>
          <strong>{stringValue(lastRequest.requestId) || "-"}</strong>
          <span>{numberValue(lastRequest.statusCode) ?? "-"}</span>
          <span>{modelPair(lastRequest)}</span>
          <span>{formatTime(numberValue(lastRequest.createdAtMs))}</span>
        </div>
      )}

      <div className="share-app-runtime-list">
        {apps.map((app) => (
          <ShareAppRuntimeRow
            key={app}
            app={app}
            availability={asRecord(availability?.[app])}
            providers={arrayRecords(appProviders?.[app])}
            runtime={asRecord(appRuntimes?.[app])}
          />
        ))}
      </div>

      {modelHealth.length > 0 && (
        <div className="model-health-list">
          {modelHealth.map((item, index) => (
            <div className="model-health-row" key={`${item.app}:${item.providerId || item.requestedModel}:${index}`}>
              <StatusPill tone={modelHealthTone(item.status)}>{item.status || "unknown"}</StatusPill>
              <div>
                <strong>{appLabel(item.app)} · {item.providerName || item.providerId || "provider"}</strong>
                <span>{`${item.requestedModel || "-"} -> ${item.actualModel || "-"}`}</span>
              </div>
              <span>{item.statusCode ?? "-"}</span>
              <span>{item.latencyMs == null ? "-" : `${item.latencyMs}ms`}</span>
              <span>{formatHealthCheckedAt(item.checkedAt)}</span>
            </div>
          ))}
        </div>
      )}

      <details className="json-details">
        <summary>{tx("Runtime JSON")}</summary>
        <JsonPreview value={snapshot} />
      </details>
    </section>
  );
}

function RuntimeValue({
  label,
  value,
  tone,
}: {
  label: string;
  value: ReactNode;
  tone?: "success" | "warning" | "danger";
}) {
  const { tx } = useI18n();
  return (
    <div className="share-runtime-value">
      <span>{tx(label)}</span>
      {tone ? <StatusPill tone={tone}>{value}</StatusPill> : <strong>{value}</strong>}
    </div>
  );
}

function ShareAppRuntimeRow({
  app,
  availability,
  providers,
  runtime,
}: {
  app: AppKind;
  availability?: Record<string, unknown>;
  providers: Array<Record<string, unknown>>;
  runtime?: Record<string, unknown>;
}) {
  const { tx } = useI18n();
  const available = booleanValue(availability?.available);
  const providerName =
    stringValue(runtime?.name) ||
    stringValue(runtime?.providerName) ||
    providers.map((provider) => stringValue(provider.name)).find(Boolean) ||
    "-";
  const reason = stringValue(availability?.reason);
  const quotaBlocked = booleanValue(availability?.quotaBlocked);
  return (
    <div className="share-app-runtime-row">
      <strong>{appLabel(app)}</strong>
      <span>{providerName}</span>
      <StatusPill tone={available === false || quotaBlocked ? "danger" : available === true ? "success" : "warning"}>
        {tx(quotaBlocked ? "quota" : available === false ? "blocked" : available === true ? "available" : "unknown")}
      </StatusPill>
      <span>{reason || `${providers.length} provider${providers.length === 1 ? "" : "s"}`}</span>
    </div>
  );
}

function asRecord(value: unknown): Record<string, unknown> | undefined {
  return value && typeof value === "object" && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : undefined;
}

function arrayRecords(value: unknown): Array<Record<string, unknown>> {
  if (!Array.isArray(value)) return [];
  const records: Array<Record<string, unknown>> = [];
  for (const item of value) {
    const record = asRecord(item);
    if (record) records.push(record);
  }
  return records;
}

function stringValue(value: unknown): string | undefined {
  return typeof value === "string" && value.trim() ? value.trim() : undefined;
}

function numberValue(value: unknown): number | undefined {
  if (typeof value === "number" && Number.isFinite(value)) return value;
  if (typeof value === "string" && value.trim()) {
    const parsed = Number(value);
    if (Number.isFinite(parsed)) return parsed;
  }
  return undefined;
}

function booleanValue(value: unknown): boolean | undefined {
  if (typeof value === "boolean") return value;
  if (value === "true") return true;
  if (value === "false") return false;
  return undefined;
}

function providerHealthStatus(
  health?: Record<string, unknown>,
): { label: string; tone: "success" | "warning" | "danger" } {
  if (!health) return { label: "unknown", tone: "warning" };
  const healthy = booleanValue(health.healthy);
  const reason = stringValue(health.reason);
  if (healthy === true) return { label: "healthy", tone: "success" };
  if (healthy === false) return { label: reason || "unhealthy", tone: "danger" };
  const successRate = numberValue(health.successRate);
  if (successRate != null) return { label: `${successRate.toFixed(1)}%`, tone: "warning" };
  return { label: reason || "unknown", tone: "warning" };
}

function modelHealthResults(value: unknown): ModelHealthView[] {
  const summary = asRecord(value);
  if (!summary) return [];
  return apps.flatMap((app) =>
    arrayRecords(summary[app]).map((record) => ({
      app,
      requestedModel: stringValue(record.requestedModel) || "-",
      actualModel: stringValue(record.actualModel) || "-",
      status: stringValue(record.status) || "unknown",
      statusCode: numberValue(record.statusCode),
      latencyMs: numberValue(record.latencyMs),
      checkedAt: numberValue(record.checkedAt),
      providerId: stringValue(record.providerId),
      providerName: stringValue(record.providerName),
    })),
  );
}

function modelHealthTone(status: string): "success" | "warning" | "danger" {
  if (status === "success" || status === "healthy") return "success";
  if (status === "quota_blocked" || status === "unknown") return "warning";
  return "danger";
}

function modelPair(record: Record<string, unknown>): string {
  const requested =
    stringValue(record.requestedModel) ||
    stringValue(record.model) ||
    stringValue(record.requestModel) ||
    "-";
  const actual = stringValue(record.actualModel) || requested;
  return requested === actual ? requested : `${requested} -> ${actual}`;
}

function formatPercent(value?: number | null): string {
  return value == null ? "-" : `${value.toFixed(1)}%`;
}

function formatHealthCheckedAt(value?: number): string {
  return value == null ? "-" : formatTime(value);
}
