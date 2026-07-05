import { useEffect, useState } from "react";

import { loadUsageLogDetail, type UsageLog } from "@/lib/api";
import { KeyValue } from "@/components/KeyValue";
import { JsonPreview } from "@/components/JsonPreview";
import { LoadingBlock } from "@/components/LoadingBlock";
import { SimpleModal } from "@/components/SimpleModal";
import { formatInt, formatLatency, formatUsd, modelRoute, sourceText } from "@/components/usage/usageDisplay";

export function UsageRequestDetailModal({ requestId, onClose }: { requestId: string; onClose: () => void }) {
  const [log, setLog] = useState<UsageLog | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let active = true;
    setLog(null);
    setError(null);
    loadUsageLogDetail(requestId)
      .then((next) => {
        if (active) setLog(next);
      })
      .catch((reason) => {
        if (active) setError(errorMessage(reason));
      });
    return () => {
      active = false;
    };
  }, [requestId]);

  return (
    <SimpleModal title="Request Detail" subtitle={requestId} onClose={onClose}>
      {error && <div className="form-error">{error}</div>}
      {log ? (
        <div className="modal-form-stack">
          <div className="provider-card-meta">
            <KeyValue label="provider" value={log.providerName || log.providerId} />
            <KeyValue label="app" value={log.app} />
            <KeyValue label="model" value={modelRoute(log)} />
            <KeyValue label="pricing" value={log.pricingModel || "-"} />
            <KeyValue label="status" value={log.statusCode} />
            <KeyValue label="duration" value={formatLatency(log)} />
            <KeyValue label="tokens" value={formatInt(log.totalTokens)} />
            <KeyValue label="cost" value={log.totalCostUsd == null ? "-" : formatUsd(log.totalCostUsd, 6)} />
            <KeyValue label="share" value={log.shareName || log.shareId || "-"} />
            <KeyValue label="user" value={log.userEmail || "-"} />
            <KeyValue label="session" value={log.sessionId || "-"} />
            <KeyValue label="source" value={sourceText(log)} />
          </div>
          <JsonPreview value={log} />
        </div>
      ) : (
        <LoadingBlock label="Loading request detail" />
      )}
    </SimpleModal>
  );
}

function errorMessage(reason: unknown): string {
  return reason instanceof Error ? reason.message : String(reason);
}
