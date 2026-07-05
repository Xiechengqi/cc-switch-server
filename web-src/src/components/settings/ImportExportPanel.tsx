import { Download, FileJson, Loader2, Upload } from "lucide-react";
import { useState } from "react";

import { ConfirmDialog } from "@/components/ConfirmDialog";
import { SectionHeader } from "@/components/settings/SettingsSectionHeader";
import {
  exportProviders,
  exportShares,
  exportUniversalProviders,
  importProviders,
  importShares,
  importUniversalProviders,
  ShareRecord,
  StoredProvider,
  UniversalProvider,
} from "@/lib/api";
import { useI18n } from "@/lib/i18n";

export function ImportExportPanel({
  busy,
  runAction,
}: {
  busy: string | null;
  runAction: (action: string, task: () => Promise<string>) => Promise<void>;
}) {
  const { tx } = useI18n();
  return (
    <section className="settings-card wide">
      <SectionHeader
        icon={<FileJson size={17} />}
        title={tx("Import / Export")}
        subtitle={tx("Move server provider, share, and universal provider JSON data")}
      />
      <div className="settings-import-export-grid">
        <ImportExportCard<StoredProvider>
          title={tx("Providers")}
          subtitle={tx("Claude, Codex, and Gemini provider configurations")}
          actionKey="providers"
          busy={busy}
          exportData={exportProviders}
          importData={importProviders}
          normalize={normalizeProvidersImport}
          runAction={runAction}
        />
        <ImportExportCard<ShareRecord>
          title={tx("Shares")}
          subtitle={tx("Share records, bindings, ACL, tunnel, and market metadata")}
          actionKey="shares"
          busy={busy}
          exportData={exportShares}
          importData={importShares}
          normalize={normalizeSharesImport}
          runAction={runAction}
        />
        <ImportExportCard<UniversalProvider>
          title={tx("Universal Providers")}
          subtitle={tx("Reusable provider templates shared across supported apps")}
          actionKey="universal"
          exportKey="providers"
          busy={busy}
          exportData={exportUniversalProviders}
          importData={importUniversalProviders}
          normalize={normalizeUniversalProvidersImport}
          runAction={runAction}
        />
      </div>
    </section>
  );
}

function ImportExportCard<T>({
  title,
  subtitle,
  actionKey,
  exportKey,
  busy,
  exportData,
  importData,
  normalize,
  runAction,
}: {
  title: string;
  subtitle: string;
  actionKey: string;
  exportKey?: string;
  busy: string | null;
  exportData: () => Promise<T[]>;
  importData: (items: T[]) => Promise<number>;
  normalize: (value: unknown) => T[];
  runAction: (action: string, task: () => Promise<string>) => Promise<void>;
}) {
  const { tx } = useI18n();
  const [text, setText] = useState("");
  const [importConfirmOpen, setImportConfirmOpen] = useState(false);
  const exportBusy = busy === `import-export:${actionKey}:export`;
  const importBusy = busy === `import-export:${actionKey}:import`;

  async function exportAction() {
    await runAction(`import-export:${actionKey}:export`, async () => {
      const items = await exportData();
      setText(formatExportJson(exportKey || actionKey, items));
      return tx("exported {{count}} {{name}}", { count: items.length, name: title });
    });
  }

  async function importAction() {
    await runAction(`import-export:${actionKey}:import`, async () => {
      const items = normalize(parseJsonText(text));
      const count = await importData(items);
      return tx("imported {{count}} {{name}}", { count, name: title });
    });
  }

  return (
    <>
      <article className="settings-import-export-card">
        <header>
          <div>
            <h3>{title}</h3>
            <span>{subtitle}</span>
          </div>
        </header>
        <textarea
          value={text}
          onChange={(event) => setText(event.target.value)}
          spellCheck={false}
          placeholder={tx("Export JSON appears here, or paste JSON to import")}
        />
        <div className="settings-actions">
          <button className="secondary-button" type="button" onClick={() => void exportAction()} disabled={exportBusy}>
            {exportBusy ? <Loader2 size={15} /> : <Download size={15} />}
            <span>{tx("Export")}</span>
          </button>
          <button
            className="primary-button"
            type="button"
            onClick={() => setImportConfirmOpen(true)}
            disabled={importBusy || !text.trim()}
          >
            {importBusy ? <Loader2 size={15} /> : <Upload size={15} />}
            <span>{tx("Import")}</span>
          </button>
        </div>
      </article>
      <ConfirmDialog
        isOpen={importConfirmOpen}
        title={tx("Import {{name}}", { name: title })}
        message={tx("Import pasted JSON into {{name}}? Existing records with matching IDs may be updated.", { name: title })}
        confirmText={tx("Import")}
        variant="info"
        onConfirm={() => {
          setImportConfirmOpen(false);
          void importAction();
        }}
        onCancel={() => setImportConfirmOpen(false)}
      />
    </>
  );
}

function formatExportJson(key: string, items: unknown[]): string {
  return JSON.stringify({ [key]: items }, null, 2);
}

function parseJsonText(text: string): unknown {
  if (!text.trim()) {
    throw new Error("import JSON is required");
  }
  try {
    return JSON.parse(text);
  } catch (error) {
    throw new Error(`import JSON is invalid: ${errorMessage(error)}`);
  }
}

function normalizeProvidersImport(value: unknown): StoredProvider[] {
  return normalizeArrayProperty<StoredProvider>(value, "providers");
}

function normalizeSharesImport(value: unknown): ShareRecord[] {
  return normalizeArrayProperty<ShareRecord>(value, "shares");
}

function normalizeUniversalProvidersImport(value: unknown): UniversalProvider[] {
  return normalizeArrayProperty<UniversalProvider>(value, "universal");
}

function normalizeArrayProperty<T>(value: unknown, key: string): T[] {
  if (Array.isArray(value)) return value as T[];
  if (isRecord(value)) {
    const byKey = value[key];
    if (Array.isArray(byKey)) return byKey as T[];
    if (key === "universal" && Array.isArray(value.providers)) return value.providers as T[];
  }
  throw new Error(`${key} import must be an array or { "${key}": [...] }`);
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

function errorMessage(reason: unknown): string {
  return reason instanceof Error ? reason.message : String(reason);
}
