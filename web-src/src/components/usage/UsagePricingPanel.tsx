import { FormEvent, useState } from "react";
import { Coins, Database, Loader2, Pencil, Plus, RotateCcw, Save, Trash2 } from "lucide-react";

import { inferIconForText } from "@/config/iconInference";
import type { ModelPricingEntry, UpdateModelPricingInput } from "@/lib/api";
import { useI18n } from "@/lib/i18n";
import { KeyValue } from "@/components/KeyValue";
import { ProviderIcon } from "@/components/ProviderIcon";
import { SimpleModal } from "@/components/SimpleModal";
import { StatusPill } from "@/components/StatusPill";
import { UsageMiniMetric } from "@/components/usage/UsageMiniMetric";
import { formatInt } from "@/components/usage/usageDisplay";

export interface PricingDraft {
  mode: "create" | "edit";
  modelId: string;
  displayName: string;
  inputCostPerMillion: string;
  outputCostPerMillion: string;
  cacheReadCostPerMillion: string;
  cacheCreationCostPerMillion: string;
}

export const pricingDefaultTemplates: ModelPricingEntry[] = [
  pricingTemplate("claude-sonnet-5", "Claude Sonnet 5", "3", "15", "0.30", "3.75"),
  pricingTemplate("claude-opus-4-8", "Claude Opus 4.8", "5", "25", "0.50", "6.25"),
  pricingTemplate("claude-sonnet-4-6", "Claude Sonnet 4.6", "3", "15", "0.30", "3.75"),
  pricingTemplate("claude-haiku-4-5", "Claude Haiku 4.5", "0.80", "4", "0.08", "1"),
  pricingTemplate("gpt-5-5", "GPT-5.5", "2", "10", "0.20", "2.50"),
  pricingTemplate("gpt-5-5-codex-low", "GPT-5.5 Codex Low", "2", "10", "0.20", "2.50"),
  pricingTemplate("gpt-5-5-codex-medium", "GPT-5.5 Codex Medium", "2", "10", "0.20", "2.50"),
  pricingTemplate("gpt-5-5-codex-high", "GPT-5.5 Codex High", "2", "10", "0.20", "2.50"),
  pricingTemplate("gemini-3-pro", "Gemini 3 Pro", "1.25", "10", "0.31", "1.25"),
  pricingTemplate("gemini-3-flash", "Gemini 3 Flash", "0.30", "2.50", "0.075", "0.30"),
  pricingTemplate("kimi-k2", "Kimi K2", "0.60", "2.50", "0", "0"),
  pricingTemplate("glm-5-2", "GLM 5.2", "0.50", "2", "0", "0"),
  pricingTemplate("deepseek-v4-pro", "DeepSeek V4 Pro", "0.50", "2", "0", "0"),
];

export function UsagePricingPanel({
  models,
  busy,
  onAdd,
  onDefaults,
  onEdit,
  onDelete,
}: {
  models: ModelPricingEntry[];
  busy: string | null;
  onAdd: () => void;
  onDefaults: () => void;
  onEdit: (model: ModelPricingEntry) => void;
  onDelete: (modelId: string) => void;
}) {
  const { tx } = useI18n();
  const configuredCacheModels = models.filter(
    (model) => Number(model.cacheReadCostPerMillion) > 0 || Number(model.cacheCreationCostPerMillion) > 0,
  ).length;
  return (
    <section className="usage-panel-card">
      <div className="section-heading">
        <div className="section-title-row compact-title">
          <Coins size={17} />
          <h2>{tx("Model Pricing")}</h2>
        </div>
        <div className="provider-toolbar-actions">
          <button className="secondary-button" type="button" onClick={onDefaults}>
            <Database size={15} />
            <span>{tx("Defaults")}</span>
          </button>
          <button className="primary-button" type="button" onClick={onAdd}>
            <Plus size={15} />
            <span>{tx("Add Pricing")}</span>
          </button>
        </div>
      </div>
      <div className="usage-pricing-summary">
        <UsageMiniMetric label="models" value={formatInt(models.length)} detail={tx("configured")} />
        <UsageMiniMetric label="cache pricing" value={formatInt(configuredCacheModels)} detail={tx("models")} />
        <UsageMiniMetric label="defaults" value={formatInt(pricingDefaultTemplates.length)} detail={tx("templates")} />
      </div>
      {models.length ? (
        <div className="usage-pricing-grid">
          {models.map((model) => (
            <PricingCard
              key={model.modelId}
              model={model}
              deleting={busy === `delete:${model.modelId}`}
              onEdit={onEdit}
              onDelete={onDelete}
            />
          ))}
        </div>
      ) : (
        <div className="provider-empty">
          <Coins size={22} />
          <span>{tx("No pricing models")}</span>
        </div>
      )}
    </section>
  );
}

function PricingCard({
  model,
  deleting,
  onEdit,
  onDelete,
}: {
  model: ModelPricingEntry;
  deleting: boolean;
  onEdit: (model: ModelPricingEntry) => void;
  onDelete: (modelId: string) => void;
}) {
  const { tx } = useI18n();
  const icon = inferIconForText(model.modelId, model.displayName);
  return (
    <article className="usage-pricing-card">
      <header>
        <span className="provider-icon-frame">
          <ProviderIcon icon={icon.icon} color={icon.iconColor} name={model.displayName || model.modelId} size={22} />
        </span>
        <div>
          <strong>{model.displayName || model.modelId}</strong>
          <span title={model.modelId}>{model.modelId}</span>
        </div>
      </header>
      <div className="usage-rate-grid">
        <KeyValue label="input" value={formatPriceString(model.inputCostPerMillion)} />
        <KeyValue label="output" value={formatPriceString(model.outputCostPerMillion)} />
        <KeyValue label="cache read" value={formatPriceString(model.cacheReadCostPerMillion)} />
        <KeyValue label="cache write" value={formatPriceString(model.cacheCreationCostPerMillion)} />
      </div>
      <footer>
        <span>{tx("per million tokens")}</span>
        <div className="provider-actions">
          <button className="icon-button" type="button" title={tx("Edit pricing")} aria-label={tx("Edit pricing")} onClick={() => onEdit(model)}>
            <Pencil size={15} />
          </button>
          <button
            className="icon-button danger"
            type="button"
            title={tx("Delete pricing")}
            aria-label={tx("Delete pricing")}
            disabled={deleting}
            onClick={() => onDelete(model.modelId)}
          >
            {deleting ? <Loader2 size={15} /> : <Trash2 size={15} />}
          </button>
        </div>
      </footer>
    </article>
  );
}

export function PricingDefaultsModal({
  models,
  busy,
  onApply,
  onApplyMissing,
  onEdit,
  onClose,
}: {
  models: ModelPricingEntry[];
  busy: string | null;
  onApply: (template: ModelPricingEntry) => void;
  onApplyMissing: () => void;
  onEdit: (template: ModelPricingEntry) => void;
  onClose: () => void;
}) {
  const { tx } = useI18n();
  const missingCount = pricingDefaultTemplates.filter((template) => !hasPricingModel(models, template.modelId)).length;
  return (
    <SimpleModal
      title="Default Pricing"
      subtitle="{{count}} model templates"
      subtitleVariables={{ count: pricingDefaultTemplates.length }}
      onClose={onClose}
    >
      <div className="modal-form-stack">
        <div className="modal-inline-footer">
          <span className="usage-result">{tx("{{count}} missing", { count: missingCount })}</span>
          <button className="secondary-button" type="button" onClick={onApplyMissing} disabled={!!busy || missingCount === 0}>
            {busy === "pricing-defaults" ? <Loader2 size={15} /> : <RotateCcw size={15} />}
            <span>{tx("Apply Missing")}</span>
          </button>
        </div>
        <div className="pricing-default-grid">
          {pricingDefaultTemplates.map((template) => {
            const exists = hasPricingModel(models, template.modelId);
            return (
              <div key={template.modelId} className="pricing-default-card">
                <header>
                  <div>
                    <strong>{template.displayName}</strong>
                    <span>{template.modelId}</span>
                  </div>
                  <StatusPill tone={exists ? "success" : "warning"}>{tx(exists ? "exists" : "missing")}</StatusPill>
                </header>
                <div className="pricing-default-rates">
                  <KeyValue label="input" value={formatPriceString(template.inputCostPerMillion)} />
                  <KeyValue label="output" value={formatPriceString(template.outputCostPerMillion)} />
                  <KeyValue label="cache read" value={formatPriceString(template.cacheReadCostPerMillion)} />
                  <KeyValue label="cache write" value={formatPriceString(template.cacheCreationCostPerMillion)} />
                </div>
                <footer>
                  <button className="secondary-button" type="button" onClick={() => onEdit(template)}>
                    <Pencil size={15} />
                    <span>{tx("Edit")}</span>
                  </button>
                  <button
                    className="primary-button"
                    type="button"
                    onClick={() => onApply(template)}
                    disabled={!!busy}
                  >
                    {busy === `template:${template.modelId}` ? <Loader2 size={15} /> : <Save size={15} />}
                    <span>{tx("Apply")}</span>
                  </button>
                </footer>
              </div>
            );
          })}
        </div>
      </div>
    </SimpleModal>
  );
}

export function PricingModal({
  draft,
  saving,
  onChange,
  onClose,
  onSubmit,
}: {
  draft: PricingDraft;
  saving: boolean;
  onChange: (draft: PricingDraft) => void;
  onClose: () => void;
  onSubmit: (input: UpdateModelPricingInput) => void;
}) {
  const { tx } = useI18n();
  const [error, setError] = useState<string | null>(null);
  function patch(next: Partial<PricingDraft>) {
    onChange({ ...draft, ...next });
  }
  function submit(event: FormEvent) {
    event.preventDefault();
    const validation = validatePricingDraft(draft);
    if (validation) {
      setError(tx(validation));
      return;
    }
    onSubmit({
      modelId: draft.modelId.trim(),
      displayName: draft.displayName.trim(),
      inputCostPerMillion: draft.inputCostPerMillion.trim(),
      outputCostPerMillion: draft.outputCostPerMillion.trim(),
      cacheReadCostPerMillion: draft.cacheReadCostPerMillion.trim(),
      cacheCreationCostPerMillion: draft.cacheCreationCostPerMillion.trim(),
    });
  }
  return (
    <SimpleModal title={draft.mode === "create" ? "Add Pricing" : "Edit Pricing"} subtitle={draft.modelId || "new model"} onClose={onClose}>
      <form className="modal-form-stack" onSubmit={submit}>
        {error && <div className="form-error">{error}</div>}
        <label>
          <span>{tx("Model ID")}</span>
          <input value={draft.modelId} disabled={draft.mode === "edit"} onChange={(event) => patch({ modelId: event.target.value })} />
        </label>
        <label>
          <span>{tx("Display name")}</span>
          <input value={draft.displayName} onChange={(event) => patch({ displayName: event.target.value })} />
        </label>
        <label>
          <span>{tx("Input cost /M")}</span>
          <input inputMode="decimal" value={draft.inputCostPerMillion} onChange={(event) => patch({ inputCostPerMillion: event.target.value })} />
        </label>
        <label>
          <span>{tx("Output cost /M")}</span>
          <input inputMode="decimal" value={draft.outputCostPerMillion} onChange={(event) => patch({ outputCostPerMillion: event.target.value })} />
        </label>
        <label>
          <span>{tx("Cache read cost /M")}</span>
          <input inputMode="decimal" value={draft.cacheReadCostPerMillion} onChange={(event) => patch({ cacheReadCostPerMillion: event.target.value })} />
        </label>
        <label>
          <span>{tx("Cache creation cost /M")}</span>
          <input inputMode="decimal" value={draft.cacheCreationCostPerMillion} onChange={(event) => patch({ cacheCreationCostPerMillion: event.target.value })} />
        </label>
        <footer className="modal-inline-footer">
          <button className="secondary-button" type="button" onClick={onClose}>
            {tx("Cancel")}
          </button>
          <button className="primary-button" type="submit" disabled={saving}>
            {saving ? <Loader2 size={15} /> : <Save size={15} />}
            <span>{tx("Save Pricing")}</span>
          </button>
        </footer>
      </form>
    </SimpleModal>
  );
}

function pricingTemplate(
  modelId: string,
  displayName: string,
  inputCostPerMillion: string,
  outputCostPerMillion: string,
  cacheReadCostPerMillion: string,
  cacheCreationCostPerMillion: string,
): ModelPricingEntry {
  return {
    modelId,
    displayName,
    inputCostPerMillion,
    outputCostPerMillion,
    cacheReadCostPerMillion,
    cacheCreationCostPerMillion,
  };
}

function formatPriceString(value: string): string {
  const parsed = Number.parseFloat(value);
  return Number.isFinite(parsed) ? `$${parsed.toFixed(4)}` : `$${value}`;
}

export function emptyPricingDraft(): PricingDraft {
  return {
    mode: "create",
    modelId: "",
    displayName: "",
    inputCostPerMillion: "0",
    outputCostPerMillion: "0",
    cacheReadCostPerMillion: "0",
    cacheCreationCostPerMillion: "0",
  };
}

export function pricingDraftFromModel(model: ModelPricingEntry): PricingDraft {
  return {
    mode: "edit",
    modelId: model.modelId,
    displayName: model.displayName,
    inputCostPerMillion: model.inputCostPerMillion,
    outputCostPerMillion: model.outputCostPerMillion,
    cacheReadCostPerMillion: model.cacheReadCostPerMillion,
    cacheCreationCostPerMillion: model.cacheCreationCostPerMillion,
  };
}

export function pricingDraftFromDefault(model: ModelPricingEntry, exists: boolean): PricingDraft {
  return {
    mode: exists ? "edit" : "create",
    modelId: model.modelId,
    displayName: model.displayName,
    inputCostPerMillion: model.inputCostPerMillion,
    outputCostPerMillion: model.outputCostPerMillion,
    cacheReadCostPerMillion: model.cacheReadCostPerMillion,
    cacheCreationCostPerMillion: model.cacheCreationCostPerMillion,
  };
}

export function pricingInputFromModel(model: ModelPricingEntry): UpdateModelPricingInput {
  return {
    modelId: model.modelId,
    displayName: model.displayName,
    inputCostPerMillion: model.inputCostPerMillion,
    outputCostPerMillion: model.outputCostPerMillion,
    cacheReadCostPerMillion: model.cacheReadCostPerMillion,
    cacheCreationCostPerMillion: model.cacheCreationCostPerMillion,
  };
}

export function hasPricingModel(models: ModelPricingEntry[], modelId: string): boolean {
  const normalized = modelId.trim().toLowerCase();
  return models.some((model) => model.modelId.trim().toLowerCase() === normalized);
}

function validatePricingDraft(draft: PricingDraft): string | null {
  if (!draft.modelId.trim()) return "model id is required";
  if (!draft.displayName.trim()) return "display name is required";
  const values = [
    draft.inputCostPerMillion,
    draft.outputCostPerMillion,
    draft.cacheReadCostPerMillion,
    draft.cacheCreationCostPerMillion,
  ];
  return values.every(isNonNegativeDecimal) ? null : "prices must be non-negative decimals";
}

function isNonNegativeDecimal(value: string): boolean {
  const trimmed = value.trim();
  if (!/^\d+(?:\.\d+)?$/.test(trimmed)) return false;
  const parsed = Number.parseFloat(trimmed);
  return Number.isFinite(parsed) && parsed >= 0;
}
