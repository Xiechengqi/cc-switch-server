import { FormEvent } from "react";
import { Loader2, X } from "lucide-react";

import type { BindingDraft } from "@/components/share/ShareEditModals";
import type { AppKind, ShareBinding, ShareRecord, StoredProvider, UpsertShareInput } from "@/lib/api";
import { useI18n } from "@/lib/i18n";

export interface ShareDraft {
  mode: "create" | "edit";
  id: string;
  originalOwnerEmail: string;
  displayName: string;
  ownerEmail: string;
  primaryApp: AppKind;
  bindings: Record<AppKind, string>;
  enabled: boolean;
  status: string;
  tokenLimit: string;
  parallelLimit: string;
  expiresAt: string;
  subdomain: string;
  description: string;
  autoStart: boolean;
  forSale: boolean;
  saleMarketKind: string;
  officialPricePercent: string;
  aclEmails: string;
  marketAccessMode: string;
}

const apps: Array<{ id: AppKind; label: string }> = [
  { id: "claude", label: "Claude" },
  { id: "codex", label: "Codex" },
  { id: "gemini", label: "Gemini" },
];

export function ShareFormModal({
  draft,
  providersByApp,
  saving,
  onChange,
  onClose,
  onSubmit,
}: {
  draft: ShareDraft;
  providersByApp: Record<AppKind, StoredProvider[]>;
  saving: boolean;
  onChange: (draft: ShareDraft) => void;
  onClose: () => void;
  onSubmit: (event: FormEvent) => void;
}) {
  const { tx } = useI18n();
  function patch(next: Partial<ShareDraft>) {
    onChange({ ...draft, ...next });
  }
  function patchBinding(app: AppKind, providerId: string) {
    onChange({ ...draft, bindings: { ...draft.bindings, [app]: providerId } });
  }
  return (
    <div className="modal-backdrop" role="presentation">
      <form className="provider-form-modal share-form-modal" onSubmit={onSubmit}>
        <header>
          <div>
            <h2>{tx(draft.mode === "create" ? "Create Share" : "Edit Share")}</h2>
            <p>{tx("Share routes expose selected providers through router and market flows.")}</p>
          </div>
          <button className="icon-button" type="button" onClick={onClose} aria-label={tx("Close")}>
            <X size={16} />
          </button>
        </header>
        <div className="provider-form-grid">
          <label>
            <span>{tx("Name")}</span>
            <input value={draft.displayName} onChange={(event) => patch({ displayName: event.target.value })} />
          </label>
          <label>
            <span>{tx("Owner email")}</span>
            <input value={draft.ownerEmail} onChange={(event) => patch({ ownerEmail: event.target.value })} />
          </label>
          <label>
            <span>{tx("Primary app")}</span>
            <select value={draft.primaryApp} onChange={(event) => patch({ primaryApp: event.target.value as AppKind })}>
              {apps.map((app) => (
                <option key={app.id} value={app.id}>
                  {app.label}
                </option>
              ))}
            </select>
          </label>
          <label>
            <span>{tx("Status")}</span>
            <select value={draft.status} onChange={(event) => patch({ status: event.target.value })}>
              <option value="active">active</option>
              <option value="paused">paused</option>
              <option value="disabled">disabled</option>
            </select>
          </label>
          {apps.map((app) => (
            <label key={app.id}>
              <span>{tx("{{app}} provider", { app: app.label })}</span>
              <select value={draft.bindings[app.id]} onChange={(event) => patchBinding(app.id, event.target.value)}>
                <option value="">{tx("Unbound")}</option>
                {providersByApp[app.id].map((provider) => (
                  <option key={provider.provider.id} value={provider.provider.id}>
                    {provider.provider.name} ({provider.providerTypeId})
                  </option>
                ))}
              </select>
            </label>
          ))}
          <label>
            <span>{tx("Token limit")}</span>
            <input value={draft.tokenLimit} onChange={(event) => patch({ tokenLimit: event.target.value })} />
          </label>
          <label>
            <span>{tx("Parallel limit")}</span>
            <input value={draft.parallelLimit} onChange={(event) => patch({ parallelLimit: event.target.value })} />
          </label>
          <label>
            <span>{tx("Expires at")}</span>
            <input type="datetime-local" value={draft.expiresAt} onChange={(event) => patch({ expiresAt: event.target.value })} />
          </label>
          <label>
            <span>{tx("Subdomain")}</span>
            <input value={draft.subdomain} onChange={(event) => patch({ subdomain: event.target.value })} />
          </label>
          <label>
            <span>{tx("Sale kind")}</span>
            <select value={draft.saleMarketKind} onChange={(event) => patch({ saleMarketKind: event.target.value })}>
              <option value="share">share</option>
              <option value="token">token</option>
            </select>
          </label>
          <label>
            <span>{tx("Official price %")}</span>
            <input value={draft.officialPricePercent} onChange={(event) => patch({ officialPricePercent: event.target.value })} />
          </label>
          <label className="toggle-row">
            <input type="checkbox" checked={draft.enabled} onChange={(event) => patch({ enabled: event.target.checked })} />
            <span>{tx("Enabled")}</span>
          </label>
          <label className="toggle-row">
            <input type="checkbox" checked={draft.autoStart} onChange={(event) => patch({ autoStart: event.target.checked })} />
            <span>{tx("Auto start tunnel")}</span>
          </label>
          <label className="toggle-row">
            <input type="checkbox" checked={draft.forSale} onChange={(event) => patch({ forSale: event.target.checked })} />
            <span>{tx("For sale")}</span>
          </label>
          <label>
            <span>{tx("Market ACL mode")}</span>
            <select value={draft.marketAccessMode} onChange={(event) => patch({ marketAccessMode: event.target.value })}>
              <option value="selected">{tx("selected")}</option>
              <option value="all">{tx("all")}</option>
            </select>
          </label>
          <label className="wide-field">
            <span>{tx("Shared emails")}</span>
            <input value={draft.aclEmails} onChange={(event) => patch({ aclEmails: event.target.value })} />
          </label>
          <label className="wide-field">
            <span>{tx("Description")}</span>
            <textarea value={draft.description} onChange={(event) => patch({ description: event.target.value })} />
          </label>
        </div>
        <footer>
          <button className="secondary-button" type="button" onClick={onClose}>
            {tx("Cancel")}
          </button>
          <button className="primary-button" type="submit" disabled={saving}>
            {saving && <Loader2 size={15} />}
            <span>{tx("Save Share")}</span>
          </button>
        </footer>
      </form>
    </div>
  );
}

export function createShareDraft(providersByApp: Record<AppKind, StoredProvider[]>): ShareDraft {
  const firstApp = apps.find((app) => providersByApp[app.id].length)?.id || "claude";
  return {
    mode: "create",
    id: "",
    originalOwnerEmail: "",
    displayName: "",
    ownerEmail: "",
    primaryApp: firstApp,
    bindings: {
      claude: providersByApp.claude[0]?.provider.id || "",
      codex: providersByApp.codex[0]?.provider.id || "",
      gemini: providersByApp.gemini[0]?.provider.id || "",
    },
    enabled: true,
    status: "active",
    tokenLimit: "",
    parallelLimit: "",
    expiresAt: "",
    subdomain: "",
    description: "",
    autoStart: false,
    forSale: false,
    saleMarketKind: "share",
    officialPricePercent: "",
    aclEmails: "",
    marketAccessMode: "selected",
  };
}

export function editShareDraft(share: ShareRecord, providersByApp: Record<AppKind, StoredProvider[]>): ShareDraft {
  const bindings = bindingMap(share);
  return {
    mode: "edit",
    id: share.id,
    originalOwnerEmail: share.ownerEmail || "",
    displayName: share.displayName || "",
    ownerEmail: share.ownerEmail || "",
    primaryApp: share.app || firstBoundApp(bindings) || firstProviderApp(providersByApp),
    bindings,
    enabled: share.enabled,
    status: share.status || "active",
    tokenLimit: share.tokenLimit?.toString() || "",
    parallelLimit: share.parallelLimit?.toString() || "",
    expiresAt: toDateTimeInput(share.expiresAt),
    subdomain: share.tunnelSubdomain || "",
    description: share.description || "",
    autoStart: share.autoStart,
    forSale: share.forSale,
    saleMarketKind: share.saleMarketKind || "share",
    officialPricePercent: share.officialPricePercent?.toString() || "",
    aclEmails: (share.acl?.sharedWithEmails || []).join(", "),
    marketAccessMode: share.acl?.marketAccessMode || "selected",
  };
}

export function shareOwnerChanged(draft: ShareDraft): boolean {
  return (
    draft.mode === "edit" &&
    Boolean(draft.ownerEmail.trim()) &&
    draft.ownerEmail.trim().toLowerCase() !== draft.originalOwnerEmail.trim().toLowerCase()
  );
}

export function shareInputFromDraft(
  draft: ShareDraft,
  providersByApp: Record<AppKind, StoredProvider[]>,
): UpsertShareInput {
  const bindings = apps
    .map((app) => {
      const providerId = draft.bindings[app.id];
      if (!providerId) return null;
      const provider = providersByApp[app.id].find((item) => item.provider.id === providerId);
      if (!provider) return null;
      return {
        app: app.id,
        providerId,
        providerType: provider.providerTypeId,
      };
    })
    .filter(Boolean) as ShareBinding[];
  if (!bindings.length) throw new Error("share requires at least one provider binding");
  const primary =
    bindings.find((binding) => binding.app === draft.primaryApp) ||
    bindings[0];
  const input: UpsertShareInput = {
    app: primary.app,
    providerId: primary.providerId,
    providerType: primary.providerType,
    bindings,
    enabled: draft.enabled,
    status: draft.status,
    forSale: draft.forSale,
    saleMarketKind: draft.saleMarketKind || "share",
    autoStart: draft.autoStart,
    acl: {
      sharedWithEmails: splitList(draft.aclEmails),
      marketAccessMode: draft.marketAccessMode || "selected",
    },
  };
  if (draft.id) input.id = draft.id;
  assignString(input, "displayName", draft.displayName);
  assignString(input, "ownerEmail", draft.ownerEmail);
  assignString(input, "tunnelSubdomain", draft.subdomain);
  assignString(input, "description", draft.description);
  assignNumber(input, "tokenLimit", draft.tokenLimit);
  assignNumber(input, "parallelLimit", draft.parallelLimit);
  assignNumber(input, "officialPricePercent", draft.officialPricePercent);
  const expiresAt = parseDateTime(draft.expiresAt);
  if (expiresAt != null) input.expiresAt = expiresAt;
  return input;
}

export function createBindingDraft(share: ShareRecord, app: AppKind): BindingDraft {
  return {
    share,
    app,
    providerId: bindingMap(share)[app],
  };
}

function bindingMap(share: ShareRecord): Record<AppKind, string> {
  const result: Record<AppKind, string> = { claude: "", codex: "", gemini: "" };
  for (const binding of share.bindings || []) {
    result[binding.app] = binding.providerId;
  }
  if (!result[share.app]) result[share.app] = share.providerId;
  return result;
}

function firstBoundApp(bindings: Record<AppKind, string>): AppKind | null {
  return apps.find((app) => bindings[app.id])?.id || null;
}

function firstProviderApp(providersByApp: Record<AppKind, StoredProvider[]>): AppKind {
  return apps.find((app) => providersByApp[app.id].length)?.id || "claude";
}

function splitList(value: string): string[] {
  return value
    .split(/[\s,;]+/)
    .map((item) => item.trim())
    .filter(Boolean);
}

function assignString(target: UpsertShareInput, key: keyof UpsertShareInput, value: string) {
  const trimmed = value.trim();
  if (trimmed) {
    (target as unknown as Record<string, unknown>)[key] = trimmed;
  }
}

function assignNumber(target: UpsertShareInput, key: keyof UpsertShareInput, value: string) {
  if (!value.trim()) return;
  const parsed = Number(value);
  if (!Number.isFinite(parsed)) throw new Error(`${String(key)} must be a number`);
  (target as unknown as Record<string, unknown>)[key] = parsed;
}

function parseDateTime(value: string): number | undefined {
  if (!value.trim()) return undefined;
  const parsed = Date.parse(value);
  if (!Number.isFinite(parsed)) throw new Error("expires at is invalid");
  return parsed;
}

function toDateTimeInput(value?: number | null): string {
  if (!value) return "";
  const millis = value < 10_000_000_000 ? value * 1000 : value;
  const date = new Date(millis);
  if (Number.isNaN(date.getTime())) return "";
  const offset = date.getTimezoneOffset() * 60_000;
  return new Date(date.getTime() - offset).toISOString().slice(0, 16);
}
