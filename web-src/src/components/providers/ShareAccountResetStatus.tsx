import { useTranslation } from "react-i18next";
import type { AccountQuotaCycle, AccountResetPreview } from "@/lib/api/share";
import {
  accountCycleEnded,
  accountResetBasisKey,
  accountResetBucketKey,
  accountResetReasonKey,
  accountResetStatusKey,
  effectiveAccountResetCapability,
  formatAccountResetTime,
  isKnownAccountResetReason,
} from "@/lib/share-account-reset";
import { cn } from "@/lib/utils";
import { ShareValueInfo } from "./ShareValueReadiness";

const DOT: Record<string, string> = {
  ready: "bg-success",
  initializing: "bg-muted-foreground/50",
  stale: "bg-warning",
  needsReconcile: "bg-warning",
  error: "bg-danger",
};

/**
 * One line for the Account reset period: its state in words, the current
 * cycle in local time, and the evidence (basis, last observation, caveat,
 * original code) behind ⓘ. Shown only where that period is in use or chosen.
 */
export function ShareAccountResetStatus({ preview, cycle, saveFirst, loadFailed, onRetry, className }: {
  preview?: AccountResetPreview;
  cycle?: AccountQuotaCycle;
  saveFirst?: boolean;
  loadFailed?: boolean;
  onRetry?: () => void;
  className?: string;
}) {
  const { t, i18n } = useTranslation();
  const p = "share.userLimit.accountReset.";
  const label = t(p + "label");
  if (saveFirst) {
    return <p className={cn("text-xs text-muted-foreground", className)}>{label} · {t(p + "saveFirst")}</p>;
  }
  const shownCycle = preview?.accountQuotaCycle ?? cycle;
  const state = effectiveAccountResetCapability(preview?.accountResetCapability, shownCycle);
  const reasonKey = accountResetReasonKey(state?.reason);
  const rawReason = state?.reason && !isKnownAccountResetReason(state.reason) ? state.reason : undefined;
  const ended = accountCycleEnded(shownCycle);
  const time = (ms: number | null | undefined) => formatAccountResetTime(ms, i18n.language);
  return <div className={cn("space-y-0.5 text-xs text-muted-foreground", className)}>
    <div className="flex flex-wrap items-center gap-x-1.5" role="status">
      <span aria-hidden="true" className={cn("h-1.5 w-1.5 rounded-full", DOT[state?.syncStatus ?? ""] ?? "bg-muted-foreground/50")} />
      <span className="font-medium text-foreground">{label}</span>
      <span>· {state ? t(accountResetStatusKey(state.syncStatus)) : loadFailed ? t(p + "loadFailed") : t(p + "loading")}</span>
      {reasonKey ? <span>· {t(reasonKey)}</span> : null}
      <ShareValueInfo label={t(p + "details")}>
        {shownCycle ? <>
          <p>{t(accountResetBucketKey(shownCycle.bucketId))}</p>
          <p>{t(p + "basisLine", { basis: t(accountResetBasisKey(shownCycle.basis)) })}</p>
          <p>{t(p + "observedLine", { time: time(shownCycle.observedAtMs) })}</p>
        </> : null}
        <p className="text-muted-foreground">{t(p + "caveat")}</p>
        {rawReason ? <p className="break-all font-mono text-muted-foreground">{t(p + "code", { code: rawReason })}</p> : null}
      </ShareValueInfo>
      {loadFailed && onRetry ? <button type="button" onClick={onRetry}
        className="rounded px-1 font-medium text-foreground underline-offset-2 hover:underline focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring">
        {t(p + "retry")}
      </button> : null}
    </div>
    {shownCycle ? <p>
      {t(accountResetBucketKey(shownCycle.bucketId))}{": "}
      {ended
        ? t(p + "cycleEnded", { start: time(shownCycle.startsAtMs), end: time(shownCycle.endsAtMs) })
        : shownCycle.endsAtMs != null
          ? t(p + "cycleRange", { start: time(shownCycle.startsAtMs), end: time(shownCycle.endsAtMs) })
          : t(p + "cycleOpen", { start: time(shownCycle.startsAtMs) })}
    </p> : null}
    {preview?.ledgerHealth?.capacityWarning ? <p className="text-warning" role="status">
      {t(p + "capacityWarning", { count: preview.ledgerHealth.revisionCount, limit: preview.ledgerHealth.admissionLimit })}
    </p> : null}
  </div>;
}
