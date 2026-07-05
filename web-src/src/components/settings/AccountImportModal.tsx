import { Loader2, X } from "lucide-react";
import type { FormEvent } from "react";

import JsonEditor from "@/components/JsonEditor";
import { KeyValue } from "@/components/KeyValue";
import { providerLabel } from "@/components/settings/accountDisplay";
import type { AccountImportTemplate, AccountManagerCapability, UpsertAccountInput } from "@/lib/api";
import { useI18n } from "@/lib/i18n";

export interface AccountImportDraft {
  providerType: string;
  id: string;
  email: string;
  accessToken: string;
  refreshToken: string;
  idToken: string;
  tokenType: string;
  apiKey: string;
  scopes: string;
  subscriptionLevel: string;
  quotaPercent: string;
  expiresAt: string;
  profileJson: string;
  rawJson: string;
  quotaJson: string;
}

export function AccountImportModal({
  draft,
  templates,
  capabilities,
  saving,
  onChange,
  onSubmit,
  onClose,
}: {
  draft: AccountImportDraft;
  templates: AccountImportTemplate[];
  capabilities: AccountManagerCapability[];
  saving: boolean;
  onChange: (draft: AccountImportDraft) => void;
  onSubmit: (event: FormEvent) => void;
  onClose: () => void;
}) {
  const { tx } = useI18n();
  const providerTypes = Array.from(
    new Set([...templates.map((item) => item.providerType), ...capabilities.map((item) => item.providerType)]),
  ).sort((left, right) => providerLabel(left).localeCompare(providerLabel(right)));
  const template = templates.find((item) => item.providerType === draft.providerType);
  function patch(next: Partial<AccountImportDraft>) {
    onChange({ ...draft, ...next });
  }
  return (
    <div className="modal-backdrop" role="presentation">
      <form className="provider-form-modal account-import-modal" onSubmit={onSubmit}>
        <header>
          <div>
            <h2>{tx("Import Account")}</h2>
            <p>{template?.credentialKind || tx("manual credential")}</p>
          </div>
          <button className="icon-button" type="button" onClick={onClose} aria-label={tx("Close")}>
            <X size={16} />
          </button>
        </header>
        <div className="provider-form-grid">
          <label>
            <span>{tx("Provider type")}</span>
            <select value={draft.providerType} onChange={(event) => patch({ providerType: event.target.value })}>
              {providerTypes.map((providerType) => (
                <option key={providerType} value={providerType}>
                  {providerLabel(providerType)}
                </option>
              ))}
            </select>
          </label>
          <label>
            <span>{tx("Account ID")}</span>
            <input value={draft.id} onChange={(event) => patch({ id: event.target.value })} />
          </label>
          <label>
            <span>{tx("Email")}</span>
            <input value={draft.email} onChange={(event) => patch({ email: event.target.value })} />
          </label>
          <label>
            <span>{tx("Subscription")}</span>
            <input
              value={draft.subscriptionLevel}
              onChange={(event) => patch({ subscriptionLevel: event.target.value })}
            />
          </label>
          <label>
            <span>{tx("Quota percent")}</span>
            <input value={draft.quotaPercent} onChange={(event) => patch({ quotaPercent: event.target.value })} />
          </label>
          <label>
            <span>{tx("Expires at")}</span>
            <input
              type="datetime-local"
              value={draft.expiresAt}
              onChange={(event) => patch({ expiresAt: event.target.value })}
            />
          </label>
          <label>
            <span>{tx("Access token")}</span>
            <input
              type="password"
              value={draft.accessToken}
              onChange={(event) => patch({ accessToken: event.target.value })}
            />
          </label>
          <label>
            <span>{tx("Refresh token")}</span>
            <input
              type="password"
              value={draft.refreshToken}
              onChange={(event) => patch({ refreshToken: event.target.value })}
            />
          </label>
          <label>
            <span>{tx("ID token")}</span>
            <input type="password" value={draft.idToken} onChange={(event) => patch({ idToken: event.target.value })} />
          </label>
          <label>
            <span>{tx("API key")}</span>
            <input type="password" value={draft.apiKey} onChange={(event) => patch({ apiKey: event.target.value })} />
          </label>
          <label>
            <span>{tx("Token type")}</span>
            <input value={draft.tokenType} onChange={(event) => patch({ tokenType: event.target.value })} />
          </label>
          <label>
            <span>{tx("Scopes")}</span>
            <input value={draft.scopes} onChange={(event) => patch({ scopes: event.target.value })} />
          </label>
          <div className="wide-field json-editor-field">
            <span>{tx("Profile JSON")}</span>
            <JsonEditor value={draft.profileJson} onChange={(value) => patch({ profileJson: value })} rows={6} />
          </div>
          <div className="wide-field json-editor-field">
            <span>{tx("Raw JSON")}</span>
            <JsonEditor value={draft.rawJson} onChange={(value) => patch({ rawJson: value })} rows={6} />
          </div>
          <div className="wide-field json-editor-field">
            <span>{tx("Quota JSON")}</span>
            <JsonEditor value={draft.quotaJson} onChange={(value) => patch({ quotaJson: value })} rows={6} />
          </div>
          <div className="wide-field template-note">
            <KeyValue label="required" value={(template?.requiredFields || []).join(", ") || "-"} />
            <p>{template?.notes || "-"}</p>
          </div>
        </div>
        <footer>
          <button className="secondary-button" type="button" onClick={onClose}>
            {tx("Cancel")}
          </button>
          <button className="primary-button" type="submit" disabled={saving}>
            {saving && <Loader2 size={15} />}
            <span>{tx("Save Account")}</span>
          </button>
        </footer>
      </form>
    </div>
  );
}

export function createAccountImportDraft(providerType: string): AccountImportDraft {
  return {
    providerType,
    id: "",
    email: "",
    accessToken: "",
    refreshToken: "",
    idToken: "",
    tokenType: "",
    apiKey: "",
    scopes: "",
    subscriptionLevel: "",
    quotaPercent: "",
    expiresAt: "",
    profileJson: "",
    rawJson: "",
    quotaJson: "",
  };
}

export function accountInputFromDraft(draft: AccountImportDraft): UpsertAccountInput {
  const input: UpsertAccountInput = {
    providerType: draft.providerType,
  };
  assignString(input, "id", draft.id);
  assignString(input, "email", draft.email);
  assignString(input, "accessToken", draft.accessToken);
  assignString(input, "refreshToken", draft.refreshToken);
  assignString(input, "idToken", draft.idToken);
  assignString(input, "tokenType", draft.tokenType);
  assignString(input, "apiKey", draft.apiKey);
  assignString(input, "subscriptionLevel", draft.subscriptionLevel);
  const scopes = splitScopes(draft.scopes);
  if (scopes.length) input.scopes = scopes;
  const quotaPercent = parseOptionalNumber(draft.quotaPercent, "quota percent");
  if (quotaPercent != null) input.quotaPercent = quotaPercent;
  const expiresAt = parseOptionalDateTime(draft.expiresAt);
  if (expiresAt != null) input.expiresAt = expiresAt;
  const profile = parseOptionalJson(draft.profileJson, "profile JSON");
  if (profile !== undefined) input.profile = profile;
  const raw = parseOptionalJson(draft.rawJson, "raw JSON");
  if (raw !== undefined) input.raw = raw;
  const quota = parseOptionalJson(draft.quotaJson, "quota JSON");
  if (quota !== undefined) input.quota = quota as UpsertAccountInput["quota"];
  if (!input.accessToken && !input.refreshToken && !input.apiKey && !input.raw) {
    throw new Error("account import requires credential material");
  }
  return input;
}

function assignString(target: UpsertAccountInput, key: keyof UpsertAccountInput, value: string) {
  const trimmed = value.trim();
  if (trimmed) {
    (target as unknown as Record<string, unknown>)[key] = trimmed;
  }
}

function splitScopes(value: string): string[] {
  return value
    .split(/[\s,]+/)
    .map((item) => item.trim())
    .filter(Boolean);
}

function parseOptionalNumber(value: string, label: string): number | undefined {
  if (!value.trim()) return undefined;
  const parsed = Number(value);
  if (!Number.isFinite(parsed)) throw new Error(`${label} must be a number`);
  return parsed;
}

function parseOptionalDateTime(value: string): number | undefined {
  if (!value.trim()) return undefined;
  const parsed = Date.parse(value);
  if (!Number.isFinite(parsed)) throw new Error("expires at is invalid");
  return parsed;
}

function parseOptionalJson(value: string, label: string): unknown {
  if (!value.trim()) return undefined;
  try {
    return JSON.parse(value);
  } catch (error) {
    throw new Error(`${label} is invalid: ${errorMessage(error)}`);
  }
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
