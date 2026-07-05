import { CSS } from "@dnd-kit/utilities";
import { useSortable } from "@dnd-kit/sortable";
import { Copy, Edit3, Globe, GripVertical, RotateCcw, Trash2 } from "lucide-react";
import { CSSProperties, HTMLAttributes, useState } from "react";

import { UniversalProvider } from "@/lib/api";
import { useI18n } from "@/lib/i18n";
import { inferIconForText } from "@/config/iconInference";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { IconAction } from "@/components/IconAction";
import { KeyValue } from "@/components/KeyValue";
import { JsonPreview } from "@/components/JsonPreview";
import { ProviderIcon } from "@/components/ProviderIcon";
import { StatusPill } from "@/components/StatusPill";

const universalApps = ["claude", "codex", "gemini"] as const;

export function SortableUniversalCard(props: UniversalCardProps) {
  const { attributes, listeners, setActivatorNodeRef, setNodeRef, transform, transition, isDragging } =
    useSortable({ id: props.provider.id });
  const style: CSSProperties = {
    transform: CSS.Transform.toString(transform),
    transition,
  };
  const dragHandleProps: DragHandleProps = {
    ...attributes,
    ...listeners,
    ref: setActivatorNodeRef,
  };
  return (
    <UniversalCard
      {...props}
      dragHandleProps={dragHandleProps}
      nodeRef={setNodeRef}
      style={style}
      dragging={isDragging}
    />
  );
}

type DragHandleProps = HTMLAttributes<HTMLButtonElement> & {
  ref?: (node: HTMLButtonElement | null) => void;
};


interface UniversalCardProps {
  provider: UniversalProvider;
  busy: string | null;
  onEdit: () => void;
  onSync: () => void;
  onDuplicate: () => void;
  onDelete: () => void;
}

function UniversalCard({
  provider,
  busy,
  onEdit,
  onSync,
  onDuplicate,
  onDelete,
  dragHandleProps,
  nodeRef,
  style,
  dragging,
}: UniversalCardProps & {
  dragHandleProps?: DragHandleProps;
  nodeRef?: (node: HTMLElement | null) => void;
  style?: CSSProperties;
  dragging?: boolean;
}) {
  const { tx } = useI18n();
  const [deleteConfirmOpen, setDeleteConfirmOpen] = useState(false);
  const [syncConfirmOpen, setSyncConfirmOpen] = useState(false);
  const icon = universalProviderIcon(provider);
  const enabledApps = enabledUniversalApps(provider);
  return (
    <>
      <article
        ref={nodeRef}
        className={["provider-card universal-card", dragging ? "dragging" : ""]
          .filter(Boolean)
          .join(" ")}
        style={style}
      >
      <header className="universal-card-header">
        <div className="universal-card-title-row">
          <button
            {...dragHandleProps}
            className="provider-drag-handle"
            type="button"
            aria-label={tx("Drag provider")}
            title={tx("Drag provider")}
          >
            <GripVertical size={16} />
          </button>
          <div className="provider-icon-frame universal-icon-frame">
            <ProviderIcon
              icon={icon.icon}
              name={provider.name}
              color={icon.color}
              size={24}
            />
          </div>
          <div className="universal-card-title">
            <h3>{provider.name}</h3>
            <p>{provider.providerType}</p>
          </div>
        </div>
        <div className="universal-card-actions">
          <IconAction title="Sync" busy={busy === `sync:${provider.id}`} onClick={() => setSyncConfirmOpen(true)} wrap={false}>
            <RotateCcw size={15} />
          </IconAction>
          <IconAction title="Duplicate" busy={busy === `duplicate:${provider.id}`} onClick={onDuplicate} wrap={false}>
            <Copy size={15} />
          </IconAction>
          <IconAction title="Edit" onClick={onEdit} wrap={false}>
            <Edit3 size={15} />
          </IconAction>
          <IconAction title="Delete" busy={busy === `delete:${provider.id}`} onClick={() => setDeleteConfirmOpen(true)} danger wrap={false}>
            <Trash2 size={15} />
          </IconAction>
        </div>
      </header>
      <div className="universal-url-row">
        <Globe size={14} />
        <span>{provider.baseUrl || provider.websiteUrl || "-"}</span>
        <StatusPill tone={provider.apiKey ? "success" : "warning"}>
          {tx(provider.apiKey ? "key" : "no key")}
        </StatusPill>
      </div>
      <div className="universal-app-row">
        {enabledApps.length ? (
          enabledApps.map((app) => <AppBadge key={app} label={app} enabled />)
        ) : (
          <span className="universal-no-apps">{tx("No apps enabled")}</span>
        )}
      </div>
      <div className="universal-model-strip">
        {provider.apps.claude && <KeyValue label="claude" value={provider.models?.claude?.model || "-"} />}
        {provider.apps.codex && <KeyValue label="codex" value={provider.models?.codex?.model || "-"} />}
        {provider.apps.gemini && <KeyValue label="gemini" value={provider.models?.gemini?.model || "-"} />}
      </div>
      {provider.notes && <div className="provider-card-result">{provider.notes}</div>}
      <details className="json-details">
        <summary>{tx("Config preview")}</summary>
        <div className="provider-card-meta">
          <KeyValue label="website" value={provider.websiteUrl || "-"} />
          <KeyValue label="catalog" value={configuredModelApps(provider, "modelCatalog")} />
          <KeyValue label="mapping" value={configuredModelApps(provider, "modelMapping")} />
          <KeyValue label="id" value={provider.id} />
        </div>
        <JsonPreview value={redactUniversalProvider(provider)} />
      </details>
      </article>
      <ConfirmDialog
        isOpen={syncConfirmOpen}
        title={tx("Sync universal provider")}
        message={tx("Sync {{name}} to enabled apps? Existing derived providers may be overwritten.", { name: provider.name })}
        confirmText={tx("Sync")}
        onConfirm={() => {
          setSyncConfirmOpen(false);
          onSync();
        }}
        onCancel={() => setSyncConfirmOpen(false)}
      />
      <ConfirmDialog
        isOpen={deleteConfirmOpen}
        title={tx("Delete universal provider")}
        message={tx("Delete universal provider {{name}}? Derived app providers will be removed.", { name: provider.name })}
        confirmText={tx("Delete")}
        onConfirm={() => {
          setDeleteConfirmOpen(false);
          onDelete();
        }}
        onCancel={() => setDeleteConfirmOpen(false)}
      />
    </>
  );
}


function AppBadge({ label, enabled }: { label: string; enabled: boolean }) {
  return <span className={enabled ? "universal-app-badge active" : "universal-app-badge"}>{label}</span>;
}

function enabledUniversalApps(provider: UniversalProvider): string[] {
  return [
    provider.apps.claude ? "Claude" : null,
    provider.apps.codex ? "Codex" : null,
    provider.apps.gemini ? "Gemini" : null,
  ].filter((app): app is string => Boolean(app));
}

function universalProviderIcon(provider: UniversalProvider): { icon?: string; color?: string } {
  if (provider.icon) return { icon: provider.icon, color: provider.iconColor || undefined };
  const inferred = inferIconForText(provider.name, provider.providerType, provider.baseUrl, provider.websiteUrl);
  return { icon: inferred.icon, color: inferred.iconColor };
}

function configuredModelApps(
  provider: UniversalProvider,
  key: "modelCatalog" | "modelMapping",
): string {
  const configured = universalApps.filter((app) => Boolean(provider.models?.[app]?.[key]));
  return configured.length ? configured.join(", ") : "-";
}

function redactUniversalProvider(provider: UniversalProvider): UniversalProvider {
  return {
    ...provider,
    apiKey: provider.apiKey ? "<configured>" : "",
  };
}

