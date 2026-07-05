import type { AppKind } from "@/lib/api";
import { useI18n } from "@/lib/i18n";

export type RangePreset = "today" | "1d" | "7d" | "14d" | "30d" | "all" | "custom";

export interface UsageFilterDraft {
  range: RangePreset;
  customFrom: string;
  customTo: string;
  app: "all" | AppKind;
  providerId: string;
  shareId: string;
  userEmail: string;
  sessionId: string;
  dataSource: string;
  health: "all" | "true" | "false";
  streamStatus: string;
  limit: string;
}

const apps: Array<{ id: "all" | AppKind; label: string }> = [
  { id: "all", label: "All" },
  { id: "claude", label: "Claude" },
  { id: "codex", label: "Codex" },
  { id: "gemini", label: "Gemini" },
];

const rangeOptions: Array<{ id: RangePreset; label: string }> = [
  { id: "today", label: "Today" },
  { id: "1d", label: "1d" },
  { id: "7d", label: "7d" },
  { id: "14d", label: "14d" },
  { id: "30d", label: "30d" },
  { id: "all", label: "All" },
  { id: "custom", label: "Custom" },
];

export function UsageFilterBar({
  draft,
  onChange,
}: {
  draft: UsageFilterDraft;
  onChange: (draft: UsageFilterDraft) => void;
}) {
  const { tx } = useI18n();
  const advancedCount = usageAdvancedFilterCount(draft);
  function patch(next: Partial<UsageFilterDraft>) {
    onChange({ ...draft, ...next });
  }
  function clearAdvanced() {
    patch({
      providerId: "",
      shareId: "",
      userEmail: "",
      sessionId: "",
      dataSource: "",
      health: "all",
      streamStatus: "",
      limit: "100",
    });
  }
  return (
    <section className="usage-filter-panel">
      <div className="usage-filter-primary">
        <div className="segmented usage-app-segment">
          {apps.map((app) => (
            <button
              key={app.id}
              className={draft.app === app.id ? "active" : ""}
              type="button"
              onClick={() => patch({ app: app.id })}
            >
              {tx(app.label)}
            </button>
          ))}
        </div>
        <UsageRangePicker draft={draft} onPatch={patch} />
      </div>
      <details className="usage-advanced-filters" open={advancedCount > 0 || undefined}>
        <summary>
          <span>{tx("Advanced filters")}</span>
          <small>{tx("{{count}} active", { count: advancedCount })}</small>
        </summary>
        <div className="usage-advanced-grid">
          <label>
            <span>{tx("Provider ID")}</span>
            <input value={draft.providerId} onChange={(event) => patch({ providerId: event.target.value })} />
          </label>
          <label>
            <span>{tx("Share ID")}</span>
            <input value={draft.shareId} onChange={(event) => patch({ shareId: event.target.value })} />
          </label>
          <label>
            <span>{tx("User email")}</span>
            <input value={draft.userEmail} onChange={(event) => patch({ userEmail: event.target.value })} />
          </label>
          <label>
            <span>{tx("Session ID")}</span>
            <input value={draft.sessionId} onChange={(event) => patch({ sessionId: event.target.value })} />
          </label>
          <label>
            <span>{tx("Data source")}</span>
            <input value={draft.dataSource} onChange={(event) => patch({ dataSource: event.target.value })} />
          </label>
          <label>
            <span>{tx("Health check")}</span>
            <select value={draft.health} onChange={(event) => patch({ health: event.target.value as UsageFilterDraft["health"] })}>
              <option value="all">{tx("all")}</option>
              <option value="true">{tx("yes")}</option>
              <option value="false">{tx("no")}</option>
            </select>
          </label>
          <label>
            <span>{tx("Stream status")}</span>
            <select value={draft.streamStatus} onChange={(event) => patch({ streamStatus: event.target.value })}>
              <option value="">{tx("all")}</option>
              <option value="completed">{tx("completed")}</option>
              <option value="interrupted">{tx("interrupted")}</option>
              <option value="failed">{tx("failed")}</option>
            </select>
          </label>
          <label>
            <span>{tx("Limit")}</span>
            <input value={draft.limit} onChange={(event) => patch({ limit: event.target.value })} />
          </label>
        </div>
        <button className="secondary-button compact" type="button" onClick={clearAdvanced} disabled={advancedCount === 0}>
          {tx("Clear advanced filters")}
        </button>
      </details>
    </section>
  );
}

function UsageRangePicker({
  draft,
  onPatch,
}: {
  draft: UsageFilterDraft;
  onPatch: (next: Partial<UsageFilterDraft>) => void;
}) {
  const { tx } = useI18n();
  const customLiveEnd = draft.range === "custom" && !draft.customTo;
  return (
    <div className="usage-range-picker">
      <div className="usage-range-presets" role="group" aria-label={tx("Date range")}>
        {rangeOptions.map((range) => (
          <button
            key={range.id}
            className={draft.range === range.id ? "active" : ""}
            type="button"
            onClick={() => onPatch({ range: range.id })}
          >
            {tx(range.label)}
          </button>
        ))}
      </div>
      {draft.range === "custom" && (
        <div className="usage-custom-range">
          <label>
            <span>{tx("From")}</span>
            <input
              type="datetime-local"
              value={draft.customFrom}
              onChange={(event) => onPatch({ customFrom: event.target.value })}
            />
          </label>
          <label>
            <span>{tx("To")}</span>
            <input
              type="datetime-local"
              value={draft.customTo}
              disabled={customLiveEnd}
              onChange={(event) => onPatch({ customTo: event.target.value })}
            />
          </label>
          <label className="usage-live-toggle">
            <input
              type="checkbox"
              checked={customLiveEnd}
              onChange={(event) => onPatch({ customTo: event.target.checked ? "" : dateTimeInput(Date.now()) })}
            />
            <span>{tx("Live end time")}</span>
          </label>
        </div>
      )}
    </div>
  );
}

export function usageAdvancedFilterCount(draft: UsageFilterDraft): number {
  return [
    draft.providerId.trim(),
    draft.shareId.trim(),
    draft.userEmail.trim(),
    draft.sessionId.trim(),
    draft.dataSource.trim(),
    draft.health !== "all" ? draft.health : "",
    draft.streamStatus.trim(),
    draft.limit.trim() && draft.limit.trim() !== "100" ? draft.limit.trim() : "",
  ].filter(Boolean).length;
}

export function usageRangeLabel(draft: UsageFilterDraft): string {
  if (draft.range !== "custom") {
    return rangeOptions.find((option) => option.id === draft.range)?.label || draft.range;
  }
  return `${draft.customFrom || "start"} -> ${draft.customTo || "now"}`;
}

export function dateTimeInput(value: number): string {
  const date = new Date(value);
  if (!Number.isFinite(date.getTime())) return "";
  const offsetMs = date.getTimezoneOffset() * 60 * 1000;
  return new Date(date.getTime() - offsetMs).toISOString().slice(0, 16);
}
