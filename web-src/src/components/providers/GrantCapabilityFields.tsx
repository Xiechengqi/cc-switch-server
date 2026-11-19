import { useEffect, useState, useId } from "react";
import { useTranslation } from "react-i18next";
import { shareApi } from "@/lib/api/share";
import type { ShareOperation } from "@/lib/share-capabilities";
import { grantMediaOperationLabel, type GrantPreview } from "@/lib/grant-capability-preview";
import { Switch } from "@/components/ui/switch";
import { Label } from "@/components/ui/label";
import { useCurrentLogin } from "./shareValueHooks";

interface Props {
  shareId?: string;
  revision?: number;
  email: string;
  operations: ShareOperation[];
  reconfirm: ShareOperation[];
  quota: "usd" | "tokens" | "unlimited";
  disabled?: boolean;
  onChange: (operations: ShareOperation[]) => void;
  onReconfirm: (operation: ShareOperation) => void;
}

export function GrantCapabilityFields({ shareId, revision, email, operations, reconfirm, quota, disabled, onChange, onReconfirm }: Props) {
  const { t } = useTranslation();
  const login = useCurrentLogin();
  const id = useId();
  const key = shareId && revision != null && /^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(email) ? JSON.stringify([login, shareId, revision, email]) : null;
  const [response, setResponse] = useState<{ key: string; preview?: GrantPreview }>();
  const [retry, setRetry] = useState(0);
  useEffect(() => {
    if (!key || !shareId || revision == null) return;
    const controller = new AbortController();
    const timer = setTimeout(async () => {
      try {
        const preview = await shareApi.previewGrantCapabilities(shareId, {
          expectedConfigRevision: revision, subject: email, grantCapabilityEdits: {},
        }, controller.signal);
        if (!controller.signal.aborted) setResponse({ key, preview });
      } catch {
        if (!controller.signal.aborted) setResponse({ key });
      }
    }, 180);
    return () => { clearTimeout(timer); controller.abort(); };
  }, [email, key, retry, revision, shareId]);
  if (!key) return null;
  const result = response?.key === key ? response : undefined;
  const preview = result?.preview;
  const reportedRows = preview?.rows ?? [];
  const rows = [...reportedRows, ...operations.filter(operation => !reportedRows.some(row => row.operation === operation))
    .map(operation => ({ operation, selected: true, available: false, needsReconfirm: false }))];
  if (preview?.ready && !preview.unavailable && !rows.length && !operations.length) return null;
  const unavailable = !!result && (!preview?.ready || preview.unavailable);

  return (
    <fieldset className="min-w-0 space-y-2 border-t border-border pt-3 sm:col-span-2">
      <legend className="pr-2 text-sm font-medium">{t("share.grantMedia.title", "Media generation and tools")}</legend>
      {!result && <p role="status" className="text-xs text-muted-foreground">{t("share.grantMedia.loading", "Loading available operations…")}</p>}
      {unavailable && (
        <p role="status" className="text-xs">
          {t("share.grantMedia.unavailable", "Available operations could not be confirmed.")}{" "}
          <button type="button" className="underline" onClick={() => { setResponse(undefined); setRetry(value => value + 1); }}>
            {t("common.retry", "Retry")}
          </button>
        </p>
      )}
      {rows.map(row => {
        const checked = operations.includes(row.operation);
        const controlId = id + "-" + row.operation;
        return (
          <div key={row.operation} className="space-y-1">
            <div className="flex min-h-9 items-center justify-between gap-3">
              <Label htmlFor={controlId}>{grantMediaOperationLabel(row.operation)}</Label>
              <Switch id={controlId} checked={checked}
                disabled={disabled || (!checked && (!preview?.ready || unavailable || !row.available))}
                onCheckedChange={enabled => onChange(enabled ? [...operations, row.operation] : operations.filter(op => op !== row.operation))} />
            </div>
            {checked && row.needsReconfirm && !unavailable && !reconfirm.includes(row.operation) && (
              <button type="button" className="text-xs underline" disabled={disabled} onClick={() => onReconfirm(row.operation)}>
                {t("share.grantMedia.reconfirm", "Entry points changed. Confirm to allow.")}
              </button>
            )}
            {result && checked && !row.available && <p className="text-xs text-muted-foreground">{t("share.grantMedia.remove", "Unavailable. You can remove permission.")}</p>}
          </div>
        );
      })}
      {operations.length > 0 && quota !== "unlimited" && (
        <p className="text-xs text-muted-foreground">{quota === "tokens"
          ? t("share.grantMedia.tokenHint", "Token limits do not cover media quantities or tool calls.")
          : t("share.grantMedia.usdHint", "Usage debits the USD limit. Requests without a price are rejected.")}</p>
      )}
    </fieldset>
  );
}
