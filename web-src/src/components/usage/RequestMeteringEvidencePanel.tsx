import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { shareApi, type RequestMeteringEvidence } from "@/lib/api/share";
import { grantMediaOperationLabel } from "@/lib/grant-capability-preview";
import { useCurrentLogin } from "@/components/providers/shareValueHooks";

function amount(picos: string | null | undefined) {
  if (typeof picos !== "string" || !/^\d{1,40}$/.test(picos)) return;
  const value = BigInt(picos);
  const divisor = 1_000_000_000_000n;
  return "$" + (value / divisor) + "." + (value % divisor).toString().padStart(12, "0").replace(/0+$/, "").padEnd(2, "0");
}

export function RequestMeteringEvidencePanel({ shareId, requestId }: { shareId: string; requestId: string }) {
  const { t, i18n } = useTranslation();
  const zh = i18n.language.startsWith("zh");
  const login = useCurrentLogin();
  const key = JSON.stringify([login, shareId, requestId]);
  const [open, setOpen] = useState(false);
  const [retry, setRetry] = useState(0);
  const [response, setResponse] = useState<{ key: string; rows?: RequestMeteringEvidence[] }>();
  const report = response?.key === key ? response : undefined;
  useEffect(() => {
    if (!open) return;
    const abort = new AbortController();
    setResponse(undefined);
    void shareApi.requestMetering(shareId, requestId, abort.signal).then(rows => {
      if (!abort.signal.aborted) setResponse({ key, rows });
    }).catch(() => { if (!abort.signal.aborted) setResponse({ key }); });
    return () => abort.abort();
  }, [key, open, requestId, retry, shareId]);

  const messages = (row: RequestMeteringEvidence) => {
    if (row.chargeMode === "estimate") return t("share.grantMedia.estimateOnly", "Estimate only. No USD quota debit.");
    if (row.chargeMode === "none") return t("share.grantMedia.noUsd", "No USD quota");
    if (row.chargeMode === "unknown") return t("share.grantMedia.historicalUnknown", "Historical debit cannot be confirmed");
    if (row.meteringStatus === "pending") return t("share.grantMedia.pending", "Pending settlement");
    const debit = amount(row.settledPicos);
    return debit ? t("share.grantMedia.debited", { amount: debit, defaultValue: "Debited from USD quota {{amount}}" })
      : t("share.grantMedia.historicalUnknown", "Historical debit cannot be confirmed");
  };

  return (
    <details className="border-t pt-3" onToggle={event => setOpen(event.currentTarget.open)}>
      <summary className="cursor-pointer text-sm font-medium">{t("share.grantMedia.metering", "Usage and USD quota")}</summary>
      {open && <div className="mt-2 space-y-3 text-xs">
        {!report ? <p role="status">{t("common.loading", "Loading…")}</p> : !report.rows ? (
          <p role="status">
            {t("usage.queryFailed", "Metering could not be loaded.")}{" "}
            <button type="button" className="underline" onClick={() => setRetry(n => n + 1)}>{t("common.retry", "Retry")}</button>
          </p>
        ) : !report.rows.length ? <p>{t("share.grantMedia.noEvidence", "No metering evidence is available.")}</p> : report.rows.map(row => (
          <div key={row.attempt} className="space-y-1">
            <p className="font-medium">{t("share.grantMedia.attempt", { count: row.attempt, defaultValue: "Attempt {{count}}" })} · {messages(row)}</p>
            {!!row.operations.length && <p>{row.operations.map(grantMediaOperationLabel).join(zh ? "、" : ", ")}</p>}
            {row.unpricedReason && <p>{t("share.grantMedia.unpriced", "Contains unpriced usage")}</p>}
            {row.estimated && <p>{t("share.grantMedia.estimatedQuantity", "Contains estimated resource quantities")}</p>}
            {row.chargeMode === "debited" && row.pendingRiskPicos && row.pendingRiskPicos !== "0" && <p>
              {t("share.grantMedia.reserve", { amount: amount(row.pendingRiskPicos), defaultValue: "Pending reservation {{amount}}" })}
            </p>}
            <details>
              <summary className="cursor-pointer">{t("share.grantMedia.technical", "Technical details")}</summary>
              <div className="mt-1 space-y-1 break-all text-muted-foreground">
                <p>{[row.model, row.meteringStatus, row.unpricedReason].filter(Boolean).join(" · ")}</p>
                {row.chargeMode !== "debited" && amount(row.settledPicos) && <p>
                  {t("share.grantMedia.recordedAmount", { amount: amount(row.settledPicos), defaultValue: "Recorded amount {{amount}}" })}
                </p>}
                {row.dimensions.map(item => <p key={item.dimension}>{item.dimension} · {item.quantity}{item.pricing === "explicit_free" ? " · " + t("share.grantMedia.explicitFree", "Explicit zero rate")
                    : amount(item.amountPicos) ? " · " + amount(item.amountPicos) : ""}</p>)}
                {row.pricingRevision && <p>{t("share.grantMedia.tariff", "Tariff revision")} {row.pricingRevision}</p>}
              </div>
            </details>
          </div>
        ))}
      </div>}
    </details>
  );
}
