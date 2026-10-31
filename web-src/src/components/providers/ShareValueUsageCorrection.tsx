import { useId, useRef, useState } from "react";
import { ChevronDown, ChevronRight } from "lucide-react";
import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { shareApi, type ShareUserGrant } from "@/lib/api/share";
import { parseValueUsage, splitValueRows, valueErrorMessage, valueUsageLabel, type ValueStatus, type ValueUsageEdit } from "@/lib/share-value-quota";
import { ShareValueInfo, useValueMessage } from "./ShareValueReadiness";

/**
 * Immediate, audited reconcile; deliberately outside the staged Share save.
 * Collapsed by default; a blocked window opens it from the row.
 */
export function ShareValueUsageCorrection({ shareId, revision, grant, status, onSaved, open, onOpenChange }: {
  shareId: string; revision: number; grant: ShareUserGrant; status: ValueStatus; onSaved: (rows: ValueStatus[]) => void;
  open: boolean; onOpenChange: (open: boolean) => void;
}) {
  const { t } = useTranslation(); const render = useValueMessage(); const q = "share.userLimit.valueQuota.";
  const [amount, setAmount] = useState(""); const [explanation, setExplanation] = useState("");
  const [working, setWorking] = useState(false); const [failure, setFailure] = useState(""); const [result, setResult] = useState("");
  const operation = useRef<{ key: string; body: ValueUsageEdit } | undefined>(undefined);
  const id = useId().replace(/:/g, "");
  const blocked = status.meteringStatus === "blocked";
  const apply = async (action: "set" | "clear") => {
    const target = action === "set" ? parseValueUsage(amount) : undefined;
    if (!explanation.trim() || (action === "set" && (target == null || BigInt(target) < BigInt(status.observedPicos)))) {
      setFailure(t(q + "correctionInvalid")); return;
    }
    const key = JSON.stringify([action, target, explanation, status.windowId, status.usageWatermark, grant.revision]);
    const body = operation.current?.key === key ? operation.current.body : { action, targetPicos: target, reason: explanation.trim(),
      expectedGrantRevision: grant.revision ?? 0, windowId: status.windowId, expectedWatermark: status.usageWatermark, operationId: crypto.randomUUID() };
    operation.current = { key, body }; setWorking(true); setFailure(""); setResult("");
    try {
      const response = await shareApi.editValueUsage(shareId, revision, { [grant.email]: body });
      operation.current = undefined;
      // A refusal for this user keeps the last good row and the panel open.
      const { rows, errors } = splitValueRows(response.items);
      if (rows.length) onSaved(rows);
      const email = grant.email.trim().toLowerCase();
      const refused = errors.find((item) => item.email === email);
      if (refused) { setFailure(render(valueErrorMessage({ code: refused.code }))); return; }
      const updated = rows.find((item) => item.email === email);
      setResult(t(q + "applied", { used: valueUsageLabel(updated?.usedPicos ?? target ?? status.observedPicos) }));
      setAmount(""); setExplanation(""); onOpenChange(false);
    } catch (error) { setFailure(render(valueErrorMessage(error))); } finally { setWorking(false); }
  };
  return <div className="mt-1 max-w-md space-y-1 text-xs">
    <div className="flex flex-wrap items-center gap-2">
      <button type="button" aria-expanded={open} aria-controls={`${id}-panel`} onClick={() => { setResult(""); onOpenChange(!open); }}
        className={`inline-flex items-center gap-1 rounded font-medium focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-blue-500/40 ${blocked ? "text-destructive" : "text-muted-foreground hover:text-foreground"}`}>
        {open ? <ChevronDown className="h-3.5 w-3.5" aria-hidden="true" /> : <ChevronRight className="h-3.5 w-3.5" aria-hidden="true" />}
        {t(q + (blocked ? "reconcile" : "correction"))}
      </button>
      {result ? <span role="status" className="text-emerald-600 dark:text-emerald-400">{result}</span> : null}
    </div>
    {open ? <div id={`${id}-panel`} className="space-y-2 rounded-md border border-border-default bg-muted/30 p-2.5">
      <p className="text-muted-foreground">{t(q + "observed")}: {valueUsageLabel(status.observedPicos)} · {t(q + "offset")}: {valueUsageLabel(status.manualOffsetPicos)}</p>
      <label className="block space-y-1" htmlFor={`${id}-target`}><span className="text-muted-foreground">{t(q + "target")}</span>
        <Input id={`${id}-target`} value={amount} onChange={(event) => setAmount(event.target.value)} disabled={working} inputMode="decimal" placeholder={valueUsageLabel(status.observedPicos)} /></label>
      <label className="block space-y-1" htmlFor={`${id}-reason`}><span className="text-muted-foreground">{t(q + "reason")}</span>
        <Input id={`${id}-reason`} value={explanation} onChange={(event) => setExplanation(event.target.value)} disabled={working} maxLength={1000} /></label>
      <div className="flex flex-wrap items-center gap-2">
        <Button type="button" size="sm" disabled={working} aria-busy={working || undefined} onClick={() => void apply("set")}>{t(q + (working ? "applying" : "applyNow"))}</Button>
        <Button type="button" size="sm" variant="outline" disabled={working} onClick={() => void apply("clear")}>{t(q + "clearOffset")}</Button>
        <ShareValueInfo label={t(q + "applyNow")}><p>{t(q + "applyNowHint")}</p></ShareValueInfo>
      </div>
      {failure ? <p className="text-destructive" role="alert">{failure}</p> : null}
    </div> : null}
  </div>;
}
