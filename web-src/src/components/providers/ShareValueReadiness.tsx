// Hallmark · pre-emit critique: P4 H4 E4 S5 R5 V4 · component-scope; Server UI tokens.
import { useId, type ReactNode } from "react";
import { ExternalLink, Info } from "lucide-react";
import { useTranslation } from "react-i18next";
import { Input } from "@/components/ui/input";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { cn } from "@/lib/utils";
import { SERVER_STORAGE_SETTINGS_SECTION, serverSettingsSectionUrl } from "@/lib/serverView";
import {
  formatValueTime, valueCheckBlocker, valueErrorMessage, valueFailedChecks, valueReadinessRoots, valueReadinessSummary, VALUE_CHECK_ORDER, valueInvalidRowMessage, valueLimitLabel, valuePreviewBlocker, valueQuotaAvailability, valueRowKind,
  valueStateKey, valueStateName, valueStatusReason, valueUsageLabel, VALUE_STATUS_STALE_MS, type ValueMessage, type ValuePreviewRow,
  type ValueStatus,
} from "@/lib/share-value-quota";
import type { useShareValueReadiness, ValuePreviewState } from "./shareValueHooks";

export type ShareQuotaUnit = "tokens" | "usdEquivalent";
const isKnownCheck = (code: string) => (VALUE_CHECK_ORDER as readonly string[]).includes(code);

export function useValueMessage() {
  const { t } = useTranslation();
  return (message: ValueMessage) => "raw" in message ? message.raw : t(message.key, message.values);
}

/** ⓘ holding the detail behind a one-line label. Click/tap or keyboard opens it. */
export function ShareValueInfo({ label, children, className }: { label: string; children: ReactNode; className?: string }) {
  return <Popover>
    <PopoverTrigger asChild>
      <button type="button" aria-label={label}
        className={cn("inline-flex h-6 w-6 shrink-0 items-center justify-center rounded-full text-muted-foreground transition-colors hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-blue-500/40", className)}>
        <Info className="h-3.5 w-3.5" aria-hidden="true" />
      </button>
    </PopoverTrigger>
    <PopoverContent className="z-[140] w-72 max-w-[calc(100vw-1rem)] space-y-1.5 p-3 text-xs leading-5">{children}</PopoverContent>
  </Popover>;
}

/**
 * `Tokens | $` segmented choice. `$` is disabled only after a check said no;
 * screen readers then hear why, from real text rather than `title`, together
 * with the visible "USD quota unavailable" line (`readinessId`).
 */
export function ShareValueUnitToggle({ value, onChange, usdDisabled, disabled, labelledBy, readinessId }: {
  value: ShareQuotaUnit; onChange: (unit: ShareQuotaUnit) => void; usdDisabled?: boolean; disabled?: boolean; labelledBy?: string;
  readinessId?: string;
}) {
  const { t } = useTranslation();
  const reasonId = useId();
  const usdOff = false;
  const needsSetup = !!usdDisabled;
  const describedBy = needsSetup ? [reasonId, readinessId].filter(Boolean).join(" ") : undefined;
  const options: Array<{ id: ShareQuotaUnit; label: string; title?: string; off?: boolean }> = [
    { id: "tokens", label: t("share.userLimit.valueQuota.tokens") },
    { id: "usdEquivalent", label: t("share.userLimit.valueQuota.unitUsdShort"), title: t("share.userLimit.valueQuota.usd"), off: usdOff },
  ];
  return <>
  <div role="radiogroup" aria-labelledby={labelledBy} aria-label={labelledBy ? undefined : t("share.userLimit.valueQuota.unit")}
    aria-describedby={describedBy}
    className="grid h-9 grid-cols-2 gap-0.5 rounded-md border border-border-default bg-muted/40 p-0.5"
    onKeyDown={(event) => {
      if (disabled || !["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown"].includes(event.key)) return;
      event.preventDefault();
      const next = options.find((option) => option.id !== value && !option.off);
      if (next) { onChange(next.id); (event.currentTarget.querySelector(`[data-unit="${next.id}"]`) as HTMLButtonElement | null)?.focus(); }
    }}>
    {options.map((option) => {
      const selected = option.id === value;
      return <button key={option.id} type="button" role="radio" data-unit={option.id} aria-checked={selected}
        aria-label={option.title} title={option.title} tabIndex={selected ? 0 : -1} disabled={disabled || option.off}
        aria-describedby={option.off ? describedBy : undefined}
        onClick={() => onChange(option.id)}
        className={cn("rounded px-3 text-sm font-medium transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-blue-500/40 disabled:cursor-not-allowed disabled:opacity-50",
          selected ? "bg-background text-foreground shadow-sm" : "text-muted-foreground hover:text-foreground")}>
        {option.label}
      </button>;
    })}
  </div>
  {needsSetup ? <span id={reasonId} className="sr-only">{t("share.userLimit.valueQuota.usdDisabledReason")}</span> : null}
  </>;
}

const linkClass = "rounded px-1 font-medium text-blue-600 underline-offset-2 hover:underline focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-blue-500/40 disabled:opacity-50 dark:text-blue-400";

/**
 * Silent when ready or unknown. When not, one line naming the first root cause
 * ("Server storage migration required +1") with the full checklist behind ⓘ;
 * checks that only wait on another failing check are marked as such there.
 * A request failure keeps the generic line, since the checklist may be stale.
 */
export function ShareValueReadiness({ state, id }: { state: ReturnType<typeof useShareValueReadiness>; id?: string }) {
  const { t } = useTranslation(); const prefix = "share.userLimit.valueQuota.readiness.";
  if (!state.unavailable) return null;
  const failed = valueFailedChecks(state.preview?.checks);
  const summary = state.error ? undefined : valueReadinessSummary(failed);
  const rootCode = summary && isKnownCheck(summary.root.code) ? summary.root.code : undefined;
  const shortName = (code: string) => isKnownCheck(code) ? t(prefix + "root." + code) : code;
  // Any root storage cause gets the fix link. It opens beside the Share editor so its unsaved draft survives.
  const storageHref = summary && valueReadinessRoots(failed).some((check) => check.code === "sqlite_authority") ? serverSettingsSectionUrl(SERVER_STORAGE_SETTINGS_SECTION, window.location.href) : undefined;
  return <div className="flex flex-wrap items-center gap-x-1 text-xs text-muted-foreground" role="status" aria-busy={state.loading}>
    <span id={id} className="inline-flex items-center gap-1.5"><span aria-hidden="true" className="h-1.5 w-1.5 rounded-full bg-amber-500" />
      {rootCode ? t(prefix + "root." + rootCode) : t("share.userLimit.valueQuota.unavailable")}
      {rootCode && summary!.more > 0 ? <span className="font-medium">{t(prefix + "more", { count: summary!.more })}</span> : null}
    </span>
    <ShareValueInfo label={t(prefix + "blocked")}>
      {state.error ? <p className="text-destructive">{t(prefix + state.error)}</p> : null}
      {failed.length ? <>
        <p>{t("share.userLimit.valueQuota.unavailableHint")}</p>
        <ul className="list-disc space-y-1 pl-4">{failed.map((check, index) => {
          const blocker = valueCheckBlocker(check, failed);
          return <li key={`${check.code}:${index}`} className={cn(blocker && "text-muted-foreground")}>
            {t(prefix + (check.reasonCode === "model_unresolved" ? "model_unresolved" : isKnownCheck(check.code) ? check.code : "failed"))}
            {check.code === "sqlite_authority" && check.state ? ` (${check.state})` : null}
            {check.code === "text_surface" ? <span className="break-all font-mono"> {check.app} · {check.providerId ?? "—"} · {check.profileId ?? "—"} / {check.driverId ?? "—"}</span> : null}
            {check.code === "model_price_coverage" ? <span className="break-all font-mono"> {check.app} · {check.providerId ?? "—"} · {check.model ?? "—"} · {check.profileId ?? "—"} / {check.driverId ?? "—"}</span> : null}
            {blocker ? <span className="block italic">{t(prefix + "waitingOn", { blocker: shortName(blocker) })}</span> : null}
          </li>;
        })}</ul>
      </> : null}
    </ShareValueInfo>
    {storageHref ? <>
      <span aria-hidden="true">·</span>
      <a href={storageHref} target="_blank" rel="noopener noreferrer" className={cn(linkClass, "inline-flex items-center gap-1")}>
        {t(prefix + "openStorage")}<ExternalLink className="h-3 w-3" aria-hidden="true" /><span className="sr-only">{t(prefix + "opensNewTab")}</span>
      </a>
    </> : null}
    <span aria-hidden="true">·</span>
    <button type="button" disabled={state.loading} onClick={state.retry} className={linkClass}>
      {t(prefix + "retry")}
    </button>
  </div>;
}

/** Amount for either unit. USD gets a `$` prefix, a `10.00` placeholder and a specific inline error. */
export function ShareValueLimitField({ id, unit, value, onChange, error, disabled, label, labelHidden }: {
  id: string; unit: ShareQuotaUnit; value: string; onChange: (value: string) => void; error?: string; disabled?: boolean;
  label: string; labelHidden?: boolean;
}) {
  const { t } = useTranslation();
  const usd = unit === "usdEquivalent";
  return <div className="space-y-1.5">
    <div className={cn("flex items-center gap-1", labelHidden && !usd && "sr-only")}>
      <label htmlFor={id} className={cn("text-sm font-medium leading-none", labelHidden && "sr-only")}>{label}</label>
      {usd ? <ShareValueInfo label={t("share.userLimit.valueQuota.details")}>
        <p>{t("share.userLimit.valueQuota.hint")}</p><p>{t("share.userLimit.valueQuota.start")}</p>
      </ShareValueInfo> : null}
    </div>
    <div className="relative">
      {usd ? <span aria-hidden="true" className="pointer-events-none absolute left-3 top-1/2 -translate-y-1/2 text-sm text-muted-foreground">$</span> : null}
      <Input id={id} type="text" inputMode="decimal" disabled={disabled} className={cn(usd && "pl-6")}
        placeholder={usd ? t("share.userLimit.valueQuota.amountPlaceholder") : t("share.unlimited", { defaultValue: "无限" })}
        aria-invalid={error ? true : undefined} aria-describedby={error ? `${id}-error` : undefined}
        value={value} onChange={(event) => onChange(event.target.value)} />
    </div>
    {error ? <p id={`${id}-error`} className="text-xs text-destructive">{error}</p> : null}
  </div>;
}

function RowLine({ row, withEmail }: { row: ValuePreviewRow; withEmail?: boolean }) {
  const { t, i18n } = useTranslation(); const p = "share.userLimit.valueQuota.preview.";
  const kind = valueRowKind(row);
  const used = valueUsageLabel(row.usedPicos ?? "0");
  const text = kind === "scheduled" ? t(p + "starts", { date: formatValueTime(row.effectiveFromMs, i18n.language, "short"), used })
    : kind === "fromNow" ? t(p + "fromNow") : t(p + "now", { used });
  const hint = kind === "scheduled" ? "hintScheduled" : kind === "fromNow" ? "hintFromNow" : "hintNow";
  return <span className="inline-flex min-w-0 items-center gap-1">
    <span className="min-w-0 break-words">{withEmail ? <span className="font-mono">{row.email} · </span> : null}{text}</span>
    {withEmail ? null : <ShareValueInfo label={t(p + "title")}>
      <p>{t(p + hint)}</p>
      {row.meteredFromMs != null ? <p className="text-muted-foreground">{t("share.userLimit.valueQuota.meteredSince", { time: formatValueTime(row.meteredFromMs, i18n.language) })}</p> : null}
    </ShareValueInfo>}
  </span>;
}
/** One line under the amount; batches add a short per-user list. */
export function ShareValuePreviewLine({ state }: { state: ValuePreviewState }) {
  const { t } = useTranslation(); const render = useValueMessage(); const p = "share.userLimit.valueQuota.preview.";
  if (!state.intent) return null;
  if (state.loading) return <p className="text-xs text-muted-foreground" role="status">{t(p + "loading")}</p>;
  if (state.error) return <p className="text-xs text-destructive" role="alert">{render(valueErrorMessage(state.error))}</p>;
  const preview = state.preview;
  if (!preview) return null;
  const blocker = valuePreviewBlocker(preview);
  const invalid = preview.invalidRows ?? [];
  if (invalid.length > 1 || (invalid.length && preview.rows.length)) {
    // List every refused user so a batch can be fixed in one pass.
    return <div className="space-y-1 text-xs text-destructive" role="alert">
      <p>{t("share.userLimit.valueQuota.invalidRows", { count: invalid.length })}</p>
      <ul className="max-h-28 space-y-0.5 overflow-auto rounded-md border border-destructive/30 px-2 py-1.5">
        {invalid.map((row, index) => <li key={`${row.email}:${index}`} className="break-words"><span className="font-mono">{row.email}</span> · {render(valueInvalidRowMessage(row))}</li>)}
      </ul>
    </div>;
  }
  if (blocker) {
    const reason = render(blocker.message);
    return <p className="text-xs text-destructive" role="alert">{blocker.email && preview.rows.length + invalid.length > 1 ? t("share.userLimit.valueQuota.rowReason", { email: blocker.email, reason }) : reason}</p>;
  }
  if (preview.rows.length <= 1) return preview.rows[0] ? <p className="text-xs text-muted-foreground" role="status"><RowLine row={preview.rows[0]} /></p> : null;
  const scheduled = preview.rows.filter((row) => valueRowKind(row) === "scheduled").length;
  const fromNow = preview.rows.filter((row) => valueRowKind(row) === "fromNow").length;
  return <div className="space-y-1 text-xs text-muted-foreground" role="status">
    <span className="inline-flex items-center gap-1">{t(p + "summary", { count: preview.rows.length, scheduled, fromNow })}
      <ShareValueInfo label={t(p + "title")}><p>{t(p + "hintScheduled")}</p><p>{t(p + "hintFromNow")}</p></ShareValueInfo></span>
    <ul className="max-h-28 space-y-0.5 overflow-auto rounded-md border border-border-default px-2 py-1.5">
      {preview.rows.map((row) => <li key={row.email}><RowLine row={row} withEmail /></li>)}
    </ul>
  </div>;
}

function resetIn(ms: number, now: number) {
  const minutes = Math.max(0, Math.floor((ms - now) / 60000));
  const days = Math.floor(minutes / 1440), hours = Math.floor((minutes % 1440) / 60);
  return days > 0 ? `${days}d ${hours}h` : hours > 0 ? `${hours}h ${minutes % 60}m` : `${minutes % 60}m`;
}
/**
 * The USD row: "US$1.50 / US$10 · resets in 3d", a thin bar, and only the
 * state that needs action. Metering state and sync time live behind ⓘ.
 */
export function ShareValueUsageCell({ status, limitMicros, stale, onReconcile }: {
  status?: ValueStatus; limitMicros: string; stale?: boolean; onReconcile?: () => void;
}) {
  const { t, i18n } = useTranslation(); const q = "share.userLimit.valueQuota."; const render = useValueMessage();
  const now = Date.now();
  const reason = valueStatusReason(status);
  const available = valueQuotaAvailability(status?.usedPicos, limitMicros);
  const blocked = !!status && valueStateName(status) === "blocked";
  const delayed = !!status && (stale || now - status.syncedAtMs > VALUE_STATUS_STALE_MS);
  const risk = status?.pendingRiskPicos && /^[1-9]\d*$/.test(status.pendingRiskPicos) ? status.pendingRiskPicos : undefined;
  return <div className="min-w-0 space-y-1">
    <div className="flex min-w-0 items-center gap-0.5 font-mono text-xs">
      <span className="min-w-0 break-words">
        {status ? valueUsageLabel(status.usedPicos) : "—"} / {valueLimitLabel(limitMicros)}
        {status?.resetsAtMs ? <span className="text-muted-foreground"> · {t(q + "resetsIn", { time: resetIn(status.resetsAtMs, now) })}</span> : null}
      </span>
      <ShareValueInfo label={t(q + "details")}>
        <p className="font-medium">{t(valueStateKey(status, now))}</p>
        {status ? <>
          {available ? <p>{t(q + "remaining", { amount: valueUsageLabel(available.remainingPicos) })}</p> : null}
          {risk ? <p>{t(q + "pendingRisk", { amount: valueUsageLabel(risk) })}</p> : null}
          {status.meteredFromMs != null ? <p>{t(q + "meteredSince", { time: formatValueTime(status.meteredFromMs, i18n.language) })}</p> : null}
          <p>{t(q + "lastSync", { time: formatValueTime(status.syncedAtMs, i18n.language) })}</p>
        </> : null}
        <p className="text-muted-foreground">{t(q + "hint")}</p>
      </ShareValueInfo>
    </div>
    {available ? <div className="h-1 w-full overflow-hidden rounded bg-muted" role="progressbar" aria-label={t(q + "details")}
      aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.round(available.percent)}>
      <div className={cn("h-full rounded", available.exhausted ? "bg-destructive" : available.nearLimit ? "bg-amber-500" : "bg-blue-500")} style={{ width: `${Math.min(100, available.percent)}%` }} />
    </div> : null}
    {blocked ? <div className="flex flex-wrap items-center gap-x-1.5 text-[11px] font-medium text-destructive">
      <span className="inline-flex items-center gap-1"><span aria-hidden="true" className="h-1.5 w-1.5 rounded-full bg-destructive" />{t(q + "needsReconcile")}</span>
      {onReconcile ? <button type="button" onClick={onReconcile} className="rounded underline underline-offset-2 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-blue-500/40">{t(q + "reconcile")}</button> : null}
    </div> : !status ? <div className="text-[11px] text-muted-foreground">{t(q + "pending")}</div>
      : available?.exhausted ? <div className="text-[11px] text-destructive">{t(q + "exhausted")}</div>
        : available?.nearLimit ? <div className="text-[11px] text-amber-600 dark:text-amber-400">{t(q + "nearLimit")}</div> : null}
    {reason ? <div className="break-words text-[11px] text-muted-foreground">{render(reason)}</div> : null}
    {delayed ? <div className="text-[11px] italic text-muted-foreground">{t(q + "syncDelayed")}</div> : null}
  </div>;
}

/** "→ US$10 from Nov 1" under the policy that stays enforced until then. */
export function ShareValueScheduledLine({ limitMicros, effectiveFromMs }: { limitMicros: string; effectiveFromMs: number }) {
  const { t, i18n } = useTranslation(); const q = "share.userLimit.valueQuota.";
  return <div className="flex items-center gap-0.5 text-[11px] text-muted-foreground">
    <span className="font-mono">{t(q + "scheduledLine", { amount: valueLimitLabel(limitMicros), date: formatValueTime(effectiveFromMs, i18n.language, "short") })}</span>
    <ShareValueInfo label={t(q + "details")}><p>{t(q + "scheduledHint")}</p></ShareValueInfo>
  </div>;
}

export function ShareValueResourceToggle({value,onChange,disabled}: {value:boolean;onChange:(value:boolean)=>void;disabled?:boolean}) {
  const {t}=useTranslation();
  return <label className="flex items-start gap-2 text-xs text-muted-foreground">
    <input type="checkbox" className="mt-0.5" checked={value} disabled={disabled} onChange={event=>onChange(event.target.checked)} />
    <span>{t("share.userLimit.valueQuota.resources")}<span className="mt-1 block">{t("share.userLimit.valueQuota.resourcesHint")}</span></span>
  </label>;
}
