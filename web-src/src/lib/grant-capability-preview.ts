import { OPERATION_LABELS, SHARE_OPERATIONS, type CapabilityConsent, type ShareOperation } from "./share-capabilities";
import type { ShareUserGrantMap, ShareUserPolicy } from "./api/share";
import i18n from "@/i18n";

export type GrantCapabilityEdit = { operations: ShareOperation[]; reconfirm?: ShareOperation[]; expectedRevision: number };
type QuotaEdits = Record<string, { expectedRevision: number; policy: ShareUserPolicy | null }>;
export type GrantPreviewInput = {
  expectedConfigRevision: number;
  subject?: string;
  grantCapabilityEdits: Record<string, GrantCapabilityEdit>;
  userGrantEdits?: QuotaEdits;
  shareTokenLimit?: number;
};
export type GrantPreview = {
  displayVersion: 3;
  shareId: string;
  configRevision: number;
  subject?: string;
  ready: boolean;
  unavailable: boolean;
  rows: { operation: ShareOperation; available: boolean; selected: boolean; needsReconfirm: boolean }[];
  requiredAcknowledgements?: { condition: string; scope: string; operation: ShareOperation }[];
  previewCredential?: string;
  expiresAt?: number;
};

export function parseGrantPreview(raw: unknown, shareId: string, input: GrantPreviewInput): GrantPreview {
  const preview = raw as GrantPreview;
  if (!preview || preview.displayVersion !== 3 || preview.shareId !== shareId
    || preview.configRevision !== input.expectedConfigRevision || preview.subject !== input.subject
    || typeof preview.ready !== "boolean" || typeof preview.unavailable !== "boolean"
    || !Array.isArray(preview.rows) || preview.rows.length > SHARE_OPERATIONS.length
    || preview.rows.some(row => !row || !SHARE_OPERATIONS.includes(row.operation)
      || typeof row.available !== "boolean" || typeof row.selected !== "boolean" || typeof row.needsReconfirm !== "boolean")
    || preview.requiredAcknowledgements && (!Array.isArray(preview.requiredAcknowledgements)
      || preview.requiredAcknowledgements.some(a => !a || typeof a.scope !== "string" || typeof a.condition !== "string"
        || !SHARE_OPERATIONS.includes(a.operation)))) {
    throw new Error("CAPABILITY_STATE_CHANGED");
  }
  return preview;
}

export function grantQuotaTerms(policy: ShareUserPolicy): ShareUserPolicy {
  const { allowedOperations: _operations, valueQuota, ...terms } = policy;
  return { ...terms, ...(valueQuota ? { valueQuota: { ...valueQuota, unpricedUsage: "reject" } } : {}) };
}

export function grantCapabilityEdits(saved: ShareUserGrantMap, drafts: ShareUserGrantMap) {
  const edits: Record<string, GrantCapabilityEdit> = {};
  for (const [email, grant] of Object.entries(drafts)) {
    if (grant.active === false || grant.manager === "routerShareMarket") continue;
    const previous = saved[email];
    const old = previous?.active !== false && previous?.manager !== "routerShareMarket"
      ? previous?.policy.allowedOperations ?? [] : [];
    const operations = [...(grant.policy.allowedOperations ?? [])].sort();
    const reconfirm = grant.reconfirmOperations?.filter(op => operations.includes(op));
    if (!reconfirm?.length && operations.length === old.length && operations.every(op => old.includes(op))) continue;
    edits[email] = { operations, expectedRevision: previous?.revision ?? 0, ...(reconfirm?.length ? { reconfirm } : {}) };
  }
  return edits;
}

export class GrantReviewCancelled extends Error {
  constructor() { super("Permission review cancelled"); }
}

export function grantMediaOperationLabel(operation: ShareOperation): string {
  return i18n.t(`share.grantMedia.operations.${operation.replace(".", "_")}`, {
    defaultValue: OPERATION_LABELS[operation]?.[1] ?? operation,
  });
}

export function confirmGrantPreview(preview: GrantPreview): CapabilityConsent {
  if (!preview.ready || !preview.previewCredential || !preview.expiresAt || preview.expiresAt <= Date.now()) {
    throw new Error(i18n.t("share.grantMedia.expired"));
  }
  const acknowledgements = preview.requiredAcknowledgements ?? [];
  if (acknowledgements.length) {
    const lines = Array.from(new Set(acknowledgements.map(a => `${a.scope} · ${grantMediaOperationLabel(a.operation)}`)));
    const message = i18n.t("share.grantMedia.confirmTokenGap");
    if (!window.confirm(message + "\n\n" + lines.join("\n"))) throw new GrantReviewCancelled();
  }
  return { credential: preview.previewCredential, acceptUncovered: acknowledgements.map(a => a.condition) };
}
