import {
  closestCenter,
  DndContext,
  DragEndEvent,
  KeyboardSensor,
  PointerSensor,
  useSensor,
  useSensors,
} from "@dnd-kit/core";
import {
  arrayMove,
  SortableContext,
  sortableKeyboardCoordinates,
  useSortable,
  verticalListSortingStrategy,
} from "@dnd-kit/sortable";
import { CSS } from "@dnd-kit/utilities";
import {
  ArrowUpAZ,
  Boxes,
  CheckCircle2,
  Copy,
  Download,
  FlaskConical,
  GripVertical,
  Link2,
  ListPlus,
  Loader2,
  Pencil,
  RefreshCw,
  Search,
  ServerCog,
  Trash2,
  Users,
  X,
} from "lucide-react";
import {
  CSSProperties,
  FormEvent,
  HTMLAttributes,
  ReactNode,
  useCallback,
  useEffect,
  useMemo,
  useState,
} from "react";

import {
  AccountRecord,
  AccountManagerCapability,
  AppKind,
  createProviderFromPreset,
  deleteProvider,
  fetchProviderModels,
  getCurrentProvider,
  loadProviderDashboardData,
  Provider,
  ProviderHealth,
  ProviderMatrix,
  ProviderMatrixEntry,
  ProviderPresetSummary,
  ProviderPresetsByApp,
  ProviderLimitStatus,
  saveProvider,
  StoredProvider,
  switchProvider,
  testProvider,
  updateProvidersSortOrder,
} from "@/lib/api";
import { useI18n } from "@/lib/i18n";
import { inferIconForText } from "@/config/iconInference";
import { ColorPicker } from "@/components/ColorPicker";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { IconPicker } from "@/components/IconPicker";
import JsonEditor from "@/components/JsonEditor";
import { ProviderIcon } from "@/components/ProviderIcon";
import { presetIcon, storedProviderIcon } from "@/lib/provider-icons";

const apps: Array<{ id: AppKind; label: string }> = [
  { id: "claude", label: "Claude" },
  { id: "codex", label: "Codex" },
  { id: "gemini", label: "Gemini" },
];

interface ProviderDashboardState {
  providers: StoredProvider[];
  matrix: ProviderMatrix | null;
  health: ProviderHealth[];
  accounts: AccountRecord[];
  capabilities: AccountManagerCapability[];
  limits: ProviderLimitStatus[];
  presets: ProviderPresetsByApp;
}

interface ProviderDraft {
  mode: "create" | "edit";
  app: AppKind;
  id: string;
  name: string;
  providerTypeId: string;
  baseUrl: string;
  apiKey: string;
  model: string;
  apiFormat: string;
  accountId: string;
  category: string;
  icon: string;
  iconColor: string;
  modelCatalogJson: string;
  modelMappingJson: string;
  pricingJson: string;
  advancedJson: string;
}

export function ProviderDashboard({
  activeApp: controlledActiveApp,
  onActiveAppChange,
  onOpenImportExport,
}: {
  activeApp?: AppKind;
  onActiveAppChange?: (app: AppKind) => void;
  onOpenImportExport?: () => void;
}) {
  const { t, tx } = useI18n();
  const [localActiveApp, setLocalActiveApp] = useState<AppKind>("claude");
  const activeApp = controlledActiveApp || localActiveApp;
  const setActiveApp = onActiveAppChange || setLocalActiveApp;
  const [data, setData] = useState<ProviderDashboardState>({
    providers: [],
    matrix: null,
    health: [],
    accounts: [],
    capabilities: [],
    limits: [],
    presets: { claude: [], codex: [], gemini: [] },
  });
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [draft, setDraft] = useState<ProviderDraft | null>(null);
  const [saving, setSaving] = useState(false);
  const [busyId, setBusyId] = useState<string | null>(null);
  const [resultById, setResultById] = useState<Record<string, string>>({});
  const [catalogOpen, setCatalogOpen] = useState(false);
  const [currentProviderId, setCurrentProviderId] = useState<string>("");
  const [providerQuery, setProviderQuery] = useState("");
  const sensors = useSensors(
    useSensor(PointerSensor, { activationConstraint: { distance: 6 } }),
    useSensor(KeyboardSensor, { coordinateGetter: sortableKeyboardCoordinates }),
  );

  const refresh = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      setData(await loadProviderDashboardData());
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  useEffect(() => {
    let active = true;
    getCurrentProvider(activeApp)
      .then((id) => {
        if (active) setCurrentProviderId(id || "");
      })
      .catch(() => {
        if (active) setCurrentProviderId("");
      });
    return () => {
      active = false;
    };
  }, [activeApp, data.providers]);

  const entries = useMemo(
    () => (data.matrix?.entries || []).filter((entry) => entry.app === activeApp),
    [activeApp, data.matrix],
  );
  const visibleEntries = useMemo(() => entries.filter((entry) => entry.uiVisible), [entries]);
  const activeProviders = data.providers.filter((provider) => provider.app === activeApp);
  const activePresets = data.presets[activeApp] || [];
  const healthById = new Map(
    data.health
      .filter((health) => health.app === activeApp)
      .map((health) => [health.providerId, health]),
  );
  const accountsById = new Map(data.accounts.map((account) => [account.id, account]));
  const capabilitiesByType = new Map(data.capabilities.map((capability) => [capability.providerType, capability]));
  const limitByProviderKey = new Map(
    data.limits.map((limit) => [providerKey(limit.app, limit.providerId), limit]),
  );
  const visibleProviders = useMemo(
    () => filterProviderList(activeProviders, providerQuery, accountsById),
    [activeProviders, accountsById, providerQuery],
  );

  function openCreate() {
    if (!visibleEntries.length && !activePresets.length) return;
    setCatalogOpen(true);
  }

  function createFromEntry(entry: ProviderMatrixEntry) {
    setCatalogOpen(false);
    setDraft(createDraft(activeApp, entry));
  }

  useEffect(() => {
    const handler = () => openCreate();
    document.addEventListener("cc-switch-server:add-provider", handler);
    return () => document.removeEventListener("cc-switch-server:add-provider", handler);
  }, [activeApp, visibleEntries]);

  function openEdit(provider: StoredProvider) {
    const entry =
      entries.find((item) => item.providerTypeId === provider.providerTypeId) ||
      visibleEntries[0];
    if (!entry) return;
    setDraft(editDraft(provider, entry));
  }

  async function submitDraft(event: FormEvent) {
    event.preventDefault();
    if (!draft) return;
    const entry = entries.find((item) => item.providerTypeId === draft.providerTypeId);
    if (!entry) {
      setError(tx("Provider type is not available for this app"));
      return;
    }
    setSaving(true);
    setError(null);
    try {
      await saveProvider(draft.app, providerFromDraft(draft, entry));
      setDraft(null);
      await refresh();
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setSaving(false);
    }
  }

  async function runAction(
    provider: StoredProvider,
    action: "test" | "network" | "stream" | "models" | "switch" | "duplicate" | "delete",
  ) {
    const key = `${provider.app}:${provider.provider.id}:${action}`;
    setBusyId(key);
    setError(null);
    try {
      if (action === "delete") {
        await deleteProvider(provider.app, provider.provider.id);
        await refresh();
        return;
      }
      if (action === "duplicate") {
        const duplicate = duplicateStoredProvider(
          provider,
          data.providers.filter((item) => item.app === provider.app),
        );
        const stored = await saveProvider(provider.app, duplicate);
        setResultById((current) => ({
          ...current,
          [stored.provider.id]: tx("Duplicated provider {{name}}", { name: provider.provider.name }),
        }));
        await refresh();
        return;
      }
      if (action === "switch") {
        await switchProvider(provider.app, provider.provider.id);
        setCurrentProviderId(provider.provider.id);
        setResultById((current) => ({
          ...current,
          [provider.provider.id]: tx("Switch check passed for server runtime"),
        }));
        return;
      }
      if (action === "models") {
        const result = await fetchProviderModels(provider.app, provider.provider.id, true);
        setResultById((current) => ({
          ...current,
          [provider.provider.id]: tx("Fetched {{models}} models; merged {{merged}}", {
            models: result.models.length,
            merged: result.mergedCount,
          }),
        }));
        await refresh();
        return;
      }
      const result = await testProvider(provider.app, provider.provider.id, {
        network: action === "network" || action === "stream",
        stream: action === "stream",
      });
      const status = result.networkChecked
        ? `${result.networkStatusCode || "no status"}${result.networkLatencyMs ? ` in ${result.networkLatencyMs}ms` : ""}`
        : tx("config only");
      setResultById((current) => ({
        ...current,
        [provider.provider.id]: `${result.support}: ${status}`,
      }));
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setBusyId(null);
    }
  }

  async function createPresetProvider(preset: ProviderPresetSummary) {
    const key = `preset:${activeApp}:${preset.name}`;
    setBusyId(key);
      setError(null);
    try {
      const stored = await createProviderFromPreset(activeApp, preset.name);
      setCatalogOpen(false);
      setResultById((current) => ({
        ...current,
        [stored.provider.id]: tx("Created from preset {{preset}}", { preset: preset.name }),
      }));
      await refresh();
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setBusyId(null);
    }
  }

  async function handleProviderDragEnd(event: DragEndEvent) {
    const { active, over } = event;
    if (!over || active.id === over.id) return;
    const oldIndex = activeProviders.findIndex((provider) => provider.provider.id === active.id);
    const newIndex = activeProviders.findIndex((provider) => provider.provider.id === over.id);
    if (oldIndex < 0 || newIndex < 0) return;
    const reordered = arrayMove(activeProviders, oldIndex, newIndex);
    const updates = reordered.map((provider, index) => ({
      id: provider.provider.id,
      sortIndex: index,
    }));
    setData((current) => {
      const reorderedQueue = reordered.map((provider, index) => ({
        ...provider,
        provider: {
          ...provider.provider,
          sortIndex: index,
        },
      }));
      return {
        ...current,
        providers: current.providers.map((provider) =>
          provider.app === activeApp ? reorderedQueue.shift() || provider : provider,
        ),
      };
    });
    setError(null);
    try {
      await updateProvidersSortOrder(activeApp, updates);
    } catch (reason) {
      setError(errorMessage(reason));
      await refresh();
    }
  }

  return (
    <div className="provider-dashboard">
      <div className="provider-toolbar">
        {!controlledActiveApp && (
          <div className="segmented">
            {apps.map((app) => (
              <button
                key={app.id}
                type="button"
                className={app.id === activeApp ? "active" : ""}
                onClick={() => setActiveApp(app.id)}
              >
                {app.label}
              </button>
            ))}
          </div>
        )}
        <div className="provider-toolbar-actions">
          {error && <span className="error-text">{error}</span>}
          <button className="secondary-button" type="button" onClick={() => void refresh()}>
            <RefreshCw size={15} />
            <span>{t("common.refresh")}</span>
          </button>
          <button
            className="secondary-button"
            type="button"
            onClick={openCreate}
            disabled={!activePresets.length && !visibleEntries.length}
          >
            <ListPlus size={15} />
            <span>{t("server.common.fromPreset")}</span>
          </button>
          <button
            className="primary-button"
            type="button"
            onClick={openCreate}
            disabled={!visibleEntries.length && !activePresets.length}
          >
            <ListPlus size={15} />
            <span>{t("server.providers.addProvider")}</span>
          </button>
        </div>
      </div>

      <ProviderListToolbar
        query={providerQuery}
        visible={visibleProviders.length}
        total={activeProviders.length}
        onQueryChange={setProviderQuery}
      />

      {loading ? (
        <div className="provider-empty">
          <Loader2 size={22} />
          <span>{t("server.providers.loading")}</span>
        </div>
      ) : activeProviders.length ? (
        visibleProviders.length ? (
          <DndContext sensors={sensors} collisionDetection={closestCenter} onDragEnd={(event) => void handleProviderDragEnd(event)}>
            <SortableContext
              items={activeProviders.map((provider) => provider.provider.id)}
              strategy={verticalListSortingStrategy}
            >
              <div className="provider-card-grid">
                {visibleProviders.map((provider) => {
                  const priority = activeProviders.findIndex((item) => item.provider.id === provider.provider.id) + 1;
                  return (
                    <SortableProviderCard
                      key={`${provider.app}:${provider.provider.id}`}
                      provider={provider}
                      priority={priority}
                      entry={entries.find((item) => item.providerTypeId === provider.providerTypeId)}
                      health={healthById.get(provider.provider.id)}
                      account={accountForProvider(provider, accountsById)}
                      capability={capabilityForProvider(provider, capabilitiesByType)}
                      limit={limitByProviderKey.get(providerKey(provider.app, provider.provider.id))}
                      current={provider.provider.id === currentProviderId}
                      result={resultById[provider.provider.id]}
                      busyId={busyId}
                      onEdit={() => openEdit(provider)}
                      onAction={(action) => void runAction(provider, action)}
                    />
                  );
                })}
              </div>
            </SortableContext>
          </DndContext>
        ) : (
          <div className="provider-empty compact-empty">
            <Search size={20} />
            <span>{tx("No providers match the current search")}</span>
          </div>
        )
      ) : (
        <ProviderEmptyState
          app={activeApp}
          canCreate={visibleEntries.length > 0 || activePresets.length > 0}
          onCreate={openCreate}
          onImport={onOpenImportExport}
        />
      )}

      {draft && (
        <ProviderFormModal
          draft={draft}
          entries={entries}
          accounts={data.accounts}
          saving={saving}
          onChange={setDraft}
          onSubmit={submitDraft}
          onClose={() => setDraft(null)}
        />
      )}

      {catalogOpen && (
        <ProviderCatalogModal
          app={activeApp}
          entries={visibleEntries}
          presets={activePresets}
          busyId={busyId}
          onSelectEntry={createFromEntry}
          onSelect={(preset) => void createPresetProvider(preset)}
          onClose={() => setCatalogOpen(false)}
        />
      )}
    </div>
  );
}

function ProviderEmptyState({
  app,
  canCreate,
  onCreate,
  onImport,
}: {
  app: AppKind;
  canCreate: boolean;
  onCreate: () => void;
  onImport?: () => void;
}) {
  const { t, tx } = useI18n();
  const appName = appLabel(app);
  return (
    <div className="provider-empty provider-empty-state">
      <div className="provider-empty-icon">
        <Users size={28} />
      </div>
      <strong>{t("server.providers.noProvidersForApp", { app: appName })}</strong>
      <p>{t("server.providers.noProvidersHint")}</p>
      <p>{tx("Import existing configuration or create a provider from desktop presets.")}</p>
      <div className="provider-empty-actions">
        {onImport && (
          <button className="primary-button" type="button" onClick={onImport}>
            <Download size={15} />
            <span>{t("common.import")}</span>
          </button>
        )}
        <button
          className={onImport ? "secondary-button" : "primary-button"}
          type="button"
          onClick={onCreate}
          disabled={!canCreate}
        >
          <ListPlus size={15} />
          <span>{t("server.providers.addProvider")}</span>
        </button>
      </div>
    </div>
  );
}

function ProviderListToolbar({
  query,
  visible,
  total,
  onQueryChange,
}: {
  query: string;
  visible: number;
  total: number;
  onQueryChange: (value: string) => void;
}) {
  const { tx } = useI18n();
  return (
    <section className="provider-list-toolbar">
      <label className="provider-list-search">
        <Search size={15} />
        <input
          value={query}
          onChange={(event) => onQueryChange(event.target.value)}
          placeholder={tx("Search providers")}
        />
      </label>
      <span className="provider-list-count">{tx("{{visible}}/{{total}} providers", { visible, total })}</span>
    </section>
  );
}

function filterProviderList(
  providers: StoredProvider[],
  query: string,
  accountsById: Map<string, AccountRecord>,
): StoredProvider[] {
  const normalizedQuery = query.trim().toLowerCase();
  if (!normalizedQuery) return providers;
  return providers.filter((provider) => {
    const accountId = provider.provider.meta?.authBinding?.accountId || "";
    const account = accountId ? accountsById.get(accountId) : undefined;
    return [
      provider.provider.id,
      provider.provider.name,
      provider.providerTypeId,
      modelFromProvider(provider.provider),
      baseUrlFromProvider(provider.provider, provider.app),
      apiFormatFromProvider(provider.provider),
      accountId,
      account?.email,
      account?.subscriptionLevel,
      provider.provider.category,
    ]
      .filter(Boolean)
      .join(" ")
      .toLowerCase()
      .includes(normalizedQuery);
  });
}

function SortableProviderCard(props: ProviderCardProps) {
  const { attributes, listeners, setActivatorNodeRef, setNodeRef, transform, transition, isDragging } =
    useSortable({ id: props.provider.provider.id });
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
    <ProviderCard
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

interface ProviderCardProps {
  provider: StoredProvider;
  priority: number;
  entry?: ProviderMatrixEntry;
  health?: ProviderHealth;
  account?: AccountRecord;
  capability?: AccountManagerCapability;
  limit?: ProviderLimitStatus;
  current: boolean;
  result?: string;
  busyId: string | null;
  onEdit: () => void;
  onAction: (action: "test" | "network" | "stream" | "models" | "switch" | "duplicate" | "delete") => void;
}

function ProviderCard({
  provider,
  priority,
  entry,
  health,
  account,
  capability,
  limit,
  current,
  result,
  busyId,
  onEdit,
  onAction,
  dragHandleProps,
  nodeRef,
  style,
  dragging,
}: ProviderCardProps & {
  dragHandleProps?: DragHandleProps;
  nodeRef?: (node: HTMLElement | null) => void;
  style?: CSSProperties;
  dragging?: boolean;
}) {
  const { tx } = useI18n();
  const [deleteConfirmOpen, setDeleteConfirmOpen] = useState(false);
  const model = modelFromProvider(provider.provider);
  const baseUrl = baseUrlFromProvider(provider.provider, provider.app);
  const providerIcon = storedProviderIcon(provider);
  const accountId = provider.provider.meta?.authBinding?.accountId;
  const accountValue = account
    ? accountSummary(account)
    : accountId || tx("direct config");
  const busyPrefix = `${provider.app}:${provider.provider.id}:`;
  return (
    <>
    <article
      ref={nodeRef}
      className={[current ? "provider-card current" : "provider-card", dragging ? "dragging" : ""]
        .filter(Boolean)
        .join(" ")}
      style={style}
    >
      <header className="provider-card-header">
        <div className="provider-card-title-row">
          <button
            {...dragHandleProps}
            className="provider-drag-handle"
            type="button"
            aria-label={tx("Drag provider")}
            title={tx("Drag provider")}
          >
            <GripVertical size={16} />
          </button>
          <div className="provider-icon-frame">
            <ProviderIcon
              icon={providerIcon.icon}
              name={provider.provider.name}
              color={providerIcon.color}
              size={22}
            />
          </div>
          <div className="provider-title-stack">
            <div className="provider-name-row">
              <h3>{provider.provider.name}</h3>
              <FailoverPriorityBadge priority={priority} />
              {current && <StatusPill tone="success">{tx("current")}</StatusPill>}
              {account?.subscriptionLevel && (
                <StatusPill tone="success">{account.subscriptionLevel}</StatusPill>
              )}
            </div>
            <p>{entry?.label || provider.providerTypeId}</p>
          </div>
        </div>
        <div className="provider-card-right">
          <ProviderHealthIndicator health={health} />
          <span>{tx("{{count}} recent requests", { count: health?.requests ?? 0 })}</span>
        </div>
      </header>
      {baseUrl && (
        <a className="provider-url-row" href={baseUrl} target="_blank" rel="noreferrer">
          <Link2 size={14} />
          <span>{baseUrl}</span>
        </a>
      )}
      <div className="provider-card-meta compact">
        <KeyValue label="model" value={model || "-"} />
        <KeyValue label="api format" value={apiFormatFromProvider(provider.provider) || "-"} />
        <KeyValue label="account" value={accountValue} />
        <KeyValue label="last status" value={health?.lastStatusCode || "-"} />
      </div>
      {entry && <ProviderReadinessPanel entry={entry} capability={capability} />}
      {account && <ProviderAccountFooter account={account} />}
      {limit && <ProviderLimitFooter limit={limit} />}
      <div className="provider-card-result">
        {result || health?.reason || tx("{{count}} recent requests", { count: health?.requests ?? 0 })}
      </div>
      <div className="provider-actions">
        <IconAction title="Edit" onClick={onEdit}>
          <Pencil size={15} />
        </IconAction>
        <IconAction
          title="Duplicate"
          onClick={() => onAction("duplicate")}
          busy={busyId === `${busyPrefix}duplicate`}
        >
          <Copy size={15} />
        </IconAction>
        <IconAction
          title="Config test"
          onClick={() => onAction("test")}
          busy={busyId === `${busyPrefix}test`}
        >
          <CheckCircle2 size={15} />
        </IconAction>
        <IconAction
          title="Network test"
          onClick={() => onAction("network")}
          busy={busyId === `${busyPrefix}network`}
        >
          <FlaskConical size={15} />
        </IconAction>
        <IconAction
          title="Stream test"
          onClick={() => onAction("stream")}
          busy={busyId === `${busyPrefix}stream`}
        >
          <RefreshCw size={15} />
        </IconAction>
        <IconAction
          title="Fetch models"
          onClick={() => onAction("models")}
          busy={busyId === `${busyPrefix}models`}
        >
          <ServerCog size={15} />
        </IconAction>
        <button className="secondary-button compact" type="button" onClick={() => onAction("switch")}>
          {tx(current ? "current" : "switch")}
        </button>
        <IconAction
          title="Delete"
          onClick={() => setDeleteConfirmOpen(true)}
          busy={busyId === `${busyPrefix}delete`}
          danger
        >
          <Trash2 size={15} />
        </IconAction>
      </div>
    </article>
      <ConfirmDialog
        isOpen={deleteConfirmOpen}
        title={tx("Delete provider")}
        message={tx("Delete provider {{name}}?", { name: provider.provider.name })}
        confirmText={tx("Delete")}
        onConfirm={() => {
          setDeleteConfirmOpen(false);
          onAction("delete");
        }}
        onCancel={() => setDeleteConfirmOpen(false)}
      />
    </>
  );
}

function ProviderHealthIndicator({ health }: { health?: ProviderHealth }) {
  const { tx } = useI18n();
  const status = providerHealthStatus(health);
  const latency = health?.avgLatencyMs == null ? null : `${Math.round(health.avgLatencyMs)}ms`;
  return (
    <div className={`provider-health-indicator ${status}`}>
      <span className="provider-health-dot" />
      <span>
        {tx(status)}
        {latency ? ` (${latency})` : ""}
      </span>
    </div>
  );
}

function providerHealthStatus(health?: ProviderHealth): "operational" | "degraded" | "failed" {
  if (!health) return "degraded";
  if (!health.healthy) return "failed";
  if ((health.failures || 0) > 0 || (health.successRate != null && health.successRate < 0.95)) {
    return "degraded";
  }
  return "operational";
}

function FailoverPriorityBadge({ priority }: { priority: number }) {
  const { tx } = useI18n();
  const label = priority <= 1 ? tx("primary") : tx("fallback {{rank}}", { rank: priority });
  return (
    <span className={priority <= 1 ? "failover-priority-badge primary" : "failover-priority-badge"}>
      {label}
    </span>
  );
}

function ProviderFormModal({
  draft,
  entries,
  accounts,
  saving,
  onChange,
  onSubmit,
  onClose,
}: {
  draft: ProviderDraft;
  entries: ProviderMatrixEntry[];
  accounts: AccountRecord[];
  saving: boolean;
  onChange: (draft: ProviderDraft) => void;
  onSubmit: (event: FormEvent) => void;
  onClose: () => void;
}) {
  const { tx } = useI18n();
  const entry = entries.find((item) => item.providerTypeId === draft.providerTypeId) || entries[0];
  const accountOptions = accounts.filter((account) =>
    accountMatchesProviderType(account, draft.providerTypeId),
  );
  const hasAdvancedConfig = Boolean(
    draft.modelCatalogJson.trim() ||
      draft.modelMappingJson.trim() ||
      draft.pricingJson.trim() ||
      draft.advancedJson.trim(),
  );
  const inferredPreviewIcon = inferIconForText(
    draft.name,
    draft.providerTypeId,
    draft.baseUrl,
    draft.apiFormat,
  );
  const previewIcon = draft.icon
    ? { icon: draft.icon, color: draft.iconColor }
    : { icon: inferredPreviewIcon.icon, color: inferredPreviewIcon.iconColor };
  function patch(next: Partial<ProviderDraft>) {
    onChange({ ...draft, ...next });
  }
  return (
    <div className="modal-backdrop" role="presentation">
      <form className="provider-form-modal" onSubmit={onSubmit}>
        <header>
          <div>
            <h2>{tx(draft.mode === "create" ? "Add Provider" : "Edit Provider")}</h2>
            <p>{entry?.note || tx("Server provider configuration")}</p>
          </div>
          <button className="icon-button" type="button" onClick={onClose} aria-label={tx("Close")}>
            <X size={16} />
          </button>
        </header>
        <div className="provider-form-grid">
          <label>
            <span>{tx("App")}</span>
            <select value={draft.app} disabled>
              {apps.map((app) => (
                <option key={app.id} value={app.id}>
                  {app.label}
                </option>
              ))}
            </select>
          </label>
          <label>
            <span>{tx("Provider type")}</span>
            <select
              value={draft.providerTypeId}
              onChange={(event) => {
                const nextEntry = entries.find((item) => item.providerTypeId === event.target.value);
                patch({
                  providerTypeId: event.target.value,
                  baseUrl: nextEntry?.defaults.baseUrl || draft.baseUrl,
                  apiFormat: nextEntry?.defaults.apiFormat || draft.apiFormat,
                  model: nextEntry?.defaults.model || draft.model,
                });
              }}
            >
              {entries
                .filter((item) => item.uiVisible)
                .map((item) => (
                  <option key={item.providerTypeId} value={item.providerTypeId}>
                    {item.label}
                  </option>
                ))}
            </select>
          </label>
          <label>
            <span>{tx("Name")}</span>
            <input value={draft.name} onChange={(event) => patch({ name: event.target.value })} />
          </label>
          <div className="universal-icon-editor provider-icon-editor">
            <div className="provider-icon-frame universal-icon-frame">
              <ProviderIcon
                icon={previewIcon.icon}
                name={draft.name || draft.providerTypeId || "Provider"}
                color={previewIcon.color}
                size={24}
              />
            </div>
            <IconPicker
              label={tx("Icon")}
              value={draft.icon}
              fallbackIcon={inferredPreviewIcon.icon}
              fallbackColor={previewIcon.color}
              providerName={draft.name || draft.providerTypeId || "Provider"}
              onChange={(value) => patch({ icon: value })}
            />
            <ColorPicker
              label={tx("Color")}
              value={draft.iconColor}
              fallback={colorInputValue(previewIcon.color)}
              onChange={(value) => patch({ iconColor: value })}
            />
          </div>
          <label>
            <span>{tx("Model")}</span>
            <input value={draft.model} onChange={(event) => patch({ model: event.target.value })} />
          </label>
          <label className="wide-field">
            <span>{tx("Category")}</span>
            <input
              value={draft.category}
              onChange={(event) => patch({ category: event.target.value })}
            />
          </label>
          {entry && (
            <ProviderAuthSection
              draft={draft}
              entry={entry}
              accountOptions={accountOptions}
              onPatch={patch}
            />
          )}
          <details className="wide-field provider-advanced-section" open={hasAdvancedConfig || undefined}>
            <summary>
              <Boxes size={16} />
              <span>{tx("Advanced configuration")}</span>
              <small>{tx("Model catalog, mapping, pricing, and provider JSON overrides")}</small>
            </summary>
            <div className="universal-json-section">
              <div className="universal-json-grid">
                <ProviderJsonField
                  title={tx("Model Catalog")}
                  label="modelCatalog JSON"
                  value={draft.modelCatalogJson}
                  placeholder={providerModelCatalogPlaceholder(draft.app)}
                  onChange={(value) => patch({ modelCatalogJson: value })}
                />
                <ProviderJsonField
                  title={tx("Model Mapping")}
                  label="modelMapping JSON"
                  value={draft.modelMappingJson}
                  placeholder={providerModelMappingPlaceholder(draft.app)}
                  onChange={(value) => patch({ modelMappingJson: value })}
                />
                <ProviderJsonField
                  title={tx("Pricing")}
                  label="pricing JSON"
                  value={draft.pricingJson}
                  placeholder={providerPricingPlaceholder()}
                  onChange={(value) => patch({ pricingJson: value })}
                />
              </div>
              <div className="json-editor-field">
                <span>{tx("Advanced provider JSON")}</span>
                <JsonEditor
                  value={draft.advancedJson}
                  onChange={(value) => patch({ advancedJson: value })}
                  rows={10}
                />
              </div>
            </div>
          </details>
        </div>
        <footer>
          <button className="secondary-button" type="button" onClick={onClose}>
            {tx("Cancel")}
          </button>
          <button className="primary-button" type="submit" disabled={saving}>
            {saving && <Loader2 size={15} />}
            <span>{tx("Save Provider")}</span>
          </button>
        </footer>
      </form>
    </div>
  );
}

function ProviderAuthSection({
  draft,
  entry,
  accountOptions,
  onPatch,
}: {
  draft: ProviderDraft;
  entry: ProviderMatrixEntry;
  accountOptions: AccountRecord[];
  onPatch: (next: Partial<ProviderDraft>) => void;
}) {
  const { tx } = useI18n();
  return (
    <section className="wide-field provider-auth-section">
      <div className="section-title-row compact-title">
        <ServerCog size={16} />
        <div>
          <h3>{tx("Authentication and endpoint")}</h3>
          <span>{tx("Manual token, managed account, and upstream routing settings")}</span>
        </div>
      </div>
      <div className="provider-auth-grid">
        <article className="provider-auth-card">
          <header>
            <StatusPill tone={entry.directConfigSupported ? "success" : "warning"}>
              {tx(entry.directConfigSupported ? "direct" : "limited")}
            </StatusPill>
            <span>{tx(entry.credentialMode)}</span>
          </header>
          <label>
            <span>{tx(entry.defaults.key || "API key")}</span>
            <input
              type="password"
              value={draft.apiKey}
              onChange={(event) => onPatch({ apiKey: event.target.value })}
              placeholder={entry.credentialMode === "oauth_or_manual_token" ? tx("optional account token") : ""}
            />
          </label>
        </article>
        <article className="provider-auth-card">
          <header>
            <StatusPill tone={entry.accountSupported ? "success" : "warning"}>
              {tx(entry.managedAccountRecommended ? "recommended" : "account")}
            </StatusPill>
            <span>{tx("{{count}} accounts", { count: accountOptions.length })}</span>
          </header>
          <label>
            <span>{tx("Managed account")}</span>
            <select value={draft.accountId} onChange={(event) => onPatch({ accountId: event.target.value })}>
              <option value="">{tx("Direct config")}</option>
              {accountOptions.map((account) => (
                <option key={account.id} value={account.id}>
                  {account.email || account.id}
                  {account.subscriptionLevel ? ` (${account.subscriptionLevel})` : ""}
                </option>
              ))}
            </select>
          </label>
        </article>
        <article className="provider-auth-card">
          <header>
            <StatusPill tone="success">{tx("endpoint")}</StatusPill>
            <span>{entry.defaults.apiFormat || tx("api format")}</span>
          </header>
          <label>
            <span>{tx("Base URL")}</span>
            <input
              value={draft.baseUrl}
              onChange={(event) => onPatch({ baseUrl: event.target.value })}
            />
          </label>
          <label>
            <span>{tx("API format")}</span>
            <input
              value={draft.apiFormat}
              onChange={(event) => onPatch({ apiFormat: event.target.value })}
            />
          </label>
        </article>
      </div>
    </section>
  );
}

function ProviderCatalogModal({
  app,
  entries,
  presets,
  busyId,
  onSelectEntry,
  onSelect,
  onClose,
}: {
  app: AppKind;
  entries: ProviderMatrixEntry[];
  presets: ProviderPresetSummary[];
  busyId: string | null;
  onSelectEntry: (entry: ProviderMatrixEntry) => void;
  onSelect: (preset: ProviderPresetSummary) => void;
  onClose: () => void;
}) {
  const { tx } = useI18n();
  const [query, setQuery] = useState("");
  const [sortMode, setSortMode] = useState<"recommended" | "name">("recommended");
  const visiblePresets = useMemo(
    () => filterCatalogPresets(presets, query, sortMode),
    [presets, query, sortMode],
  );
  const visibleEntries = useMemo(
    () => filterCatalogEntries(entries, query, sortMode),
    [entries, query, sortMode],
  );
  return (
    <div className="modal-backdrop" role="presentation">
      <section className="provider-form-modal simple-modal provider-catalog-modal">
        <header>
          <div>
            <h2>{tx("Add Provider")}</h2>
            <p>{tx("Choose a desktop preset or provider type for {{app}}", { app: appLabel(app) })}</p>
          </div>
          <button className="icon-button" type="button" onClick={onClose} aria-label={tx("Close")}>
            <X size={16} />
          </button>
        </header>
        <div className="provider-catalog-body">
          <div className="provider-catalog-toolbar">
            <label className="provider-catalog-search">
              <Search size={15} />
              <input
                value={query}
                onChange={(event) => setQuery(event.target.value)}
                placeholder={tx("Search presets and provider types")}
              />
            </label>
            <button
              className={sortMode === "name" ? "secondary-button compact active" : "secondary-button compact"}
              type="button"
              onClick={() => setSortMode((current) => (current === "name" ? "recommended" : "name"))}
              aria-label={tx("Sort presets")}
              title={tx("Sort presets")}
            >
              <ArrowUpAZ size={14} />
              <span>{tx(sortMode === "name" ? "A-Z" : "recommended")}</span>
            </button>
            <span className="provider-catalog-count">
              {tx("{{presets}} presets / {{types}} types", {
                presets: visiblePresets.length,
                types: visibleEntries.length,
              })}
            </span>
          </div>
          <section className="provider-catalog-section">
            <div className="section-title-row compact-title">
              <ListPlus size={16} />
              <div>
                <h3>{tx("Presets")}</h3>
                <span>{tx("Create with curated desktop defaults")}</span>
              </div>
            </div>
            <div className="provider-preset-grid">
              {visiblePresets.length ? (
                visiblePresets.map((preset) => {
                  const busy = busyId === `preset:${app}:${preset.name}`;
                  const icon = presetIcon(preset);
                  return (
                    <button
                      className="provider-preset-card"
                      type="button"
                      key={preset.name}
                      onClick={() => onSelect(preset)}
                      disabled={busy}
                    >
                      <span className="provider-preset-title">
                        <span className="provider-icon-frame small">
                          <ProviderIcon
                            icon={icon.icon}
                            name={preset.name}
                            color={icon.color}
                            size={18}
                          />
                        </span>
                        <strong>{preset.name}</strong>
                      </span>
                      <span>{preset.providerType || "provider"}</span>
                      <small>{preset.apiFormat || "api format -"} · {preset.baseUrl || "base URL -"}</small>
                      {busy && <Loader2 size={15} />}
                    </button>
                  );
                })
              ) : (
                <div className="provider-empty inline-empty">
                  {query.trim() ? tx("No presets match this search") : tx("No presets for {{app}}", { app: appLabel(app) })}
                </div>
              )}
            </div>
          </section>

          <section className="provider-catalog-section">
            <div className="section-title-row compact-title">
              <ServerCog size={16} />
              <div>
                <h3>{tx("Provider Types")}</h3>
                <span>{tx("Start from a server-supported adapter type")}</span>
              </div>
            </div>
            <div className="provider-type-grid catalog-type-grid">
              {visibleEntries.length ? (
                visibleEntries.map((entry) => {
                  const icon = entryIcon(entry);
                  return (
                    <button
                      className="provider-type-option catalog-type-option"
                      type="button"
                      key={entry.providerTypeId}
                      onClick={() => onSelectEntry(entry)}
                    >
                      <span className="provider-preset-title">
                        <span className="provider-icon-frame small">
                          <ProviderIcon
                            icon={icon.icon}
                            name={entry.label}
                            color={icon.color}
                            size={18}
                          />
                        </span>
                        <strong>{entry.label}</strong>
                      </span>
                      <span>{entry.defaults.apiFormat || entry.providerType}</span>
                      <small>{entry.defaults.baseUrl || entry.note || tx("Manual configuration")}</small>
                    </button>
                  );
                })
              ) : (
                <div className="provider-empty inline-empty">
                  {query.trim() ? tx("No provider types match this search") : tx("No provider types for {{app}}", { app: appLabel(app) })}
                </div>
              )}
            </div>
          </section>
        </div>
      </section>
    </div>
  );
}

function filterCatalogPresets(
  presets: ProviderPresetSummary[],
  query: string,
  sortMode: "recommended" | "name",
): ProviderPresetSummary[] {
  const normalizedQuery = query.trim().toLowerCase();
  const filtered = normalizedQuery
    ? presets.filter((preset) =>
        [
          preset.name,
          preset.providerType,
          preset.apiFormat,
          preset.baseUrl,
        ]
          .filter(Boolean)
          .join(" ")
          .toLowerCase()
          .includes(normalizedQuery),
      )
    : presets;
  if (sortMode === "recommended") return filtered;
  return [...filtered].sort((left, right) => left.name.localeCompare(right.name));
}

function filterCatalogEntries(
  entries: ProviderMatrixEntry[],
  query: string,
  sortMode: "recommended" | "name",
): ProviderMatrixEntry[] {
  const normalizedQuery = query.trim().toLowerCase();
  const filtered = normalizedQuery
    ? entries.filter((entry) =>
        [
          entry.label,
          entry.providerType,
          entry.providerTypeId,
          entry.defaults.apiFormat,
          entry.defaults.baseUrl,
          entry.note,
        ]
          .filter(Boolean)
          .join(" ")
          .toLowerCase()
          .includes(normalizedQuery),
      )
    : entries;
  if (sortMode === "recommended") return filtered;
  return [...filtered].sort((left, right) => left.label.localeCompare(right.label));
}

function entryIcon(entry: ProviderMatrixEntry): { icon?: string; color?: string } {
  const inferred = inferIconForText(
    entry.label,
    entry.providerType,
    entry.providerTypeId,
    entry.defaults.baseUrl,
    entry.defaults.apiFormat,
  );
  return { icon: inferred.icon, color: inferred.iconColor };
}

function ProviderJsonField({
  title,
  label,
  value,
  placeholder,
  onChange,
}: {
  title: string;
  label: string;
  value: string;
  placeholder: string;
  onChange: (value: string) => void;
}) {
  const { tx } = useI18n();
  return (
    <section className="universal-json-card">
      <h4>{tx(title)}</h4>
      <div className="json-editor-field">
        <span>{tx(label)}</span>
        <JsonEditor
          value={value}
          onChange={onChange}
          placeholder={placeholder}
          rows={7}
        />
      </div>
    </section>
  );
}

function KeyValue({ label, value }: { label: string; value: ReactNode }) {
  const { tx } = useI18n();
  return (
    <div className="compact-kv">
      <span>{tx(label)}</span>
      <strong>{value}</strong>
    </div>
  );
}

function ProviderReadinessPanel({
  entry,
  capability,
}: {
  entry: ProviderMatrixEntry;
  capability?: AccountManagerCapability;
}) {
  return (
    <div className="provider-readiness-panel">
      <div className="provider-readiness-header">
        <StatusPill tone={entry.uiVisible ? "success" : "warning"}>
          {entry.visibility === "diagnostic_only" ? "diagnostic" : "creatable"}
        </StatusPill>
        <span>{entry.credentialMode}</span>
      </div>
      <div className="provider-readiness-grid">
        <ReadinessFlag label="direct" enabled={entry.directConfigSupported} />
        <ReadinessFlag label="account" enabled={entry.accountSupported} />
        <ReadinessFlag label="managed" enabled={entry.managedAccountRecommended} />
        <ReadinessFlag label="refresh" enabled={capability?.supportsRefresh} />
        <ReadinessFlag label="quota" enabled={capability?.supportsQuota} />
        <ReadinessFlag label="plan" enabled={capability?.supportsRefreshPlan} />
      </div>
      <div className="provider-readiness-note">
        {capability?.serverNativeStage || capability?.status || "direct-config"}
        {entry.note ? ` · ${entry.note}` : ""}
      </div>
    </div>
  );
}

function ReadinessFlag({ label, enabled }: { label: string; enabled?: boolean }) {
  const { tx } = useI18n();
  return (
    <span className={enabled ? "readiness-flag active" : "readiness-flag"}>
      {tx(label)}
    </span>
  );
}

function ProviderAccountFooter({ account }: { account: AccountRecord }) {
  const { tx } = useI18n();
  const quotaPercent = accountQuotaPercent(account);
  const tiers = account.quota?.tiers || [];
  return (
    <div className="provider-account-footer">
      <div className="provider-account-line">
        <span>{account.email || account.id}</span>
        <span>{account.subscriptionLevel || tx("account")}</span>
        <span>{quotaPercent == null ? tx("quota -") : `${quotaPercent.toFixed(1)}%`}</span>
        <span>{formatTime(account.expiresAt)}</span>
      </div>
      {quotaPercent != null && (
        <div className="provider-quota-meter" aria-label={tx("quota")}>
          <span style={{ width: `${clampPercent(quotaPercent)}%` }} />
        </div>
      )}
      {tiers.length > 0 && (
        <div className="provider-quota-tiers">
          {tiers.slice(0, 3).map((tier) => (
            <div className="provider-quota-tier" key={tier.name}>
              <div>
                <strong>{tier.name}</strong>
                <span>{tierLine(tier)}</span>
              </div>
              <div className="provider-quota-tier-meter">
                <span style={{ width: `${clampPercent(tier.utilization ?? 0)}%` }} />
              </div>
            </div>
          ))}
        </div>
      )}
      {account.lastRefreshError && <strong>{account.lastRefreshError}</strong>}
    </div>
  );
}

type ProviderQuotaTier = NonNullable<NonNullable<AccountRecord["quota"]>["tiers"]>[number];

function accountQuotaPercent(account: AccountRecord): number | null {
  if (account.quotaPercent != null) return account.quotaPercent;
  const utilization = account.quota?.tiers?.find((tier) => tier.utilization != null)?.utilization;
  return utilization == null ? null : utilization;
}

function clampPercent(value: number): number {
  if (!Number.isFinite(value)) return 0;
  return Math.max(0, Math.min(100, value));
}

function tierLine(tier: ProviderQuotaTier): string {
  const usage = tier.used != null && tier.limit != null
    ? `${formatCompactNumber(tier.used)}/${formatCompactNumber(tier.limit)}`
    : tier.utilization == null
      ? "-"
      : `${tier.utilization.toFixed(1)}%`;
  const unit = tier.unit ? ` ${tier.unit}` : "";
  const reset = tier.resetsAt == null ? "" : ` · ${formatTime(tier.resetsAt)}`;
  return `${usage}${unit}${reset}`;
}

function formatCompactNumber(value: number): string {
  if (!Number.isFinite(value)) return "-";
  if (Math.abs(value) >= 1_000_000) return `${(value / 1_000_000).toFixed(1)}m`;
  if (Math.abs(value) >= 1_000) return `${(value / 1_000).toFixed(1)}k`;
  return Number.isInteger(value) ? String(value) : value.toFixed(1);
}

function ProviderLimitFooter({ limit }: { limit: ProviderLimitStatus }) {
  const shareWarnings = limit.shares.filter((share) => share.blocked || share.warnings.length);
  const warnings = [...limit.warnings, ...shareWarnings.flatMap((share) => share.warnings.map((warning) => `${share.shareName}: ${warning}`))];
  return (
    <div className="provider-limit-footer">
      <div className="provider-limit-grid">
        <LimitMetric
          label="daily"
          value={limitLine(limit.dailyUsageUsd, limit.dailyLimitUsd)}
          tone={limit.dailyExceeded ? "danger" : "success"}
        />
        <LimitMetric
          label="monthly"
          value={limitLine(limit.monthlyUsageUsd, limit.monthlyLimitUsd)}
          tone={limit.monthlyExceeded ? "danger" : "success"}
        />
        <LimitMetric
          label="quota"
          value={limit.accountQuotaPercent == null ? "-" : `${limit.accountQuotaPercent.toFixed(1)}%`}
          tone={limit.quotaDispatchExceeded ? "danger" : "success"}
        />
        <LimitMetric
          label="shares"
          value={`${limit.shares.filter((share) => share.blocked).length}/${limit.shares.length} blocked`}
          tone={shareWarnings.length ? "warning" : "success"}
        />
      </div>
      {(limit.accountEmail || limit.accountLastRefreshError || limit.quotaDispatchLimitPercent != null) && (
        <div className="provider-limit-line">
          <span>{limit.accountEmail || "account -"}</span>
          <span>{limit.quotaDispatchLimitPercent == null ? "dispatch -" : `dispatch ${limit.quotaDispatchLimitPercent.toFixed(1)}%`}</span>
          <span>{limit.accountQuotaRefreshedAt == null ? "quota refresh -" : formatTime(limit.accountQuotaRefreshedAt)}</span>
          {limit.accountLastRefreshError && <strong>{limit.accountLastRefreshError}</strong>}
        </div>
      )}
      {warnings.length > 0 && (
        <div className="provider-warning-list">
          {warnings.slice(0, 4).map((warning, index) => (
            <span key={`${warning}:${index}`}>{warning}</span>
          ))}
        </div>
      )}
    </div>
  );
}

function LimitMetric({
  label,
  value,
  tone,
}: {
  label: string;
  value: string;
  tone: "success" | "warning" | "danger";
}) {
  const { tx } = useI18n();
  return (
    <div className="limit-metric">
      <span>{tx(label)}</span>
      <StatusPill tone={tone}>{value}</StatusPill>
    </div>
  );
}

function StatusPill({
  children,
  tone,
}: {
  children: ReactNode;
  tone: "success" | "warning" | "danger";
}) {
  return <span className={`status-pill ${tone}`}>{children}</span>;
}

function IconAction({
  title,
  children,
  busy,
  danger,
  onClick,
}: {
  title: string;
  children: ReactNode;
  busy?: boolean;
  danger?: boolean;
  onClick: () => void;
}) {
  const { tx } = useI18n();
  const translatedTitle = tx(title);
  return (
    <button
      className={danger ? "icon-button danger" : "icon-button"}
      type="button"
      title={translatedTitle}
      aria-label={translatedTitle}
      onClick={onClick}
      disabled={busy}
    >
      {busy ? <Loader2 size={15} /> : children}
    </button>
  );
}

function createDraft(app: AppKind, entry: ProviderMatrixEntry): ProviderDraft {
  const provider: Provider = {
    id: "",
    name: entry.label,
    settingsConfig: {},
    category: "",
    meta: {
      providerType: entry.providerTypeId,
      apiFormat: entry.defaults.apiFormat,
    },
  };
  return {
    mode: "create",
    app,
    id: "",
    name: entry.label,
    providerTypeId: entry.providerTypeId,
    baseUrl: entry.defaults.baseUrl,
    apiKey: "",
    model: entry.defaults.model,
    apiFormat: entry.defaults.apiFormat,
    accountId: "",
    category: "",
    icon: "",
    iconColor: "",
    modelCatalogJson: "",
    modelMappingJson: "",
    pricingJson: "",
    advancedJson: JSON.stringify(provider, null, 2),
  };
}

function editDraft(stored: StoredProvider, entry: ProviderMatrixEntry): ProviderDraft {
  const provider = stored.provider;
  return {
    mode: "edit",
    app: stored.app,
    id: provider.id,
    name: provider.name,
    providerTypeId: stored.providerTypeId,
    baseUrl: baseUrlFromProvider(provider, stored.app) || entry.defaults.baseUrl,
    apiKey: apiKeyFromProvider(provider, entry) || "",
    model: modelFromProvider(provider) || entry.defaults.model,
    apiFormat: apiFormatFromProvider(provider) || entry.defaults.apiFormat,
    accountId: provider.meta?.authBinding?.accountId || "",
    category: provider.category || "",
    icon: getString(provider.icon) || "",
    iconColor: getString(provider.iconColor) || "",
    modelCatalogJson: providerSettingJson(provider, ["modelCatalog"]),
    modelMappingJson: providerSettingJson(provider, ["modelMapping"]),
    pricingJson: providerSettingJson(provider, ["pricing", "modelPricing"]),
    advancedJson: JSON.stringify(provider, null, 2),
  };
}

function providerFromDraft(draft: ProviderDraft, entry: ProviderMatrixEntry): Provider {
  const parsed = parseProviderJson(draft.advancedJson);
  const settings = asRecord(parsed.settingsConfig);
  const env = { ...asRecord(settings.env) };
  const baseKey = baseUrlKeyFor(draft.app, entry);
  if (draft.baseUrl.trim()) env[baseKey] = draft.baseUrl.trim();
  const key = entry.defaults.key || "API_KEY";
  if (draft.apiKey.trim()) env[key] = draft.apiKey.trim();
  if (draft.model.trim()) settings.model = draft.model.trim();
  if (draft.apiFormat.trim()) settings.apiFormat = draft.apiFormat.trim();
  if (draft.providerTypeId === "claude_auth") {
    settings.auth_mode = "bearer_only";
    env.AUTH_MODE = "bearer_only";
  }
  settings.env = env;
  setOptionalJsonSetting(settings, "modelCatalog", draft.modelCatalogJson, "modelCatalog");
  setOptionalJsonSetting(settings, "modelMapping", draft.modelMappingJson, "modelMapping");
  setPricingJsonSetting(settings, draft.pricingJson);

  const meta = { ...asRecord(parsed.meta) };
  meta.providerType = draft.providerTypeId;
  if (draft.apiFormat.trim()) meta.apiFormat = draft.apiFormat.trim();
  if (draft.accountId.trim()) {
    meta.authBinding = {
      source: "managed_account",
      authProvider: authProviderForType(draft.providerTypeId),
      accountId: draft.accountId.trim(),
    };
  } else {
    delete meta.authBinding;
  }

  return {
    ...parsed,
    id: draft.id || parsed.id || "",
    name: draft.name.trim(),
    category: draft.category.trim() || null,
    icon: draft.icon.trim() || undefined,
    iconColor: draft.iconColor.trim() || undefined,
    settingsConfig: settings,
    meta,
  };
}

function duplicateStoredProvider(provider: StoredProvider, existing: StoredProvider[]): Provider {
  const source = provider.provider;
  const nextId = uniqueProviderId(source.id || source.name || provider.providerTypeId, existing);
  const maxSortIndex = existing.reduce(
    (max, item, index) => Math.max(max, numberValue(item.provider.sortIndex) ?? index),
    -1,
  );
  return {
    ...source,
    id: nextId,
    name: `${source.name || provider.providerTypeId} copy`,
    sortIndex: maxSortIndex + 1,
  };
}

function uniqueProviderId(seed: string, existing: StoredProvider[]): string {
  const existingIds = new Set(existing.map((item) => item.provider.id));
  const base = `${seed}-copy`
    .toLowerCase()
    .replace(/[^a-z0-9_-]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 48) || "provider-copy";
  if (!existingIds.has(base)) return base;
  for (let index = 2; index < 1000; index += 1) {
    const candidate = `${base}-${index}`;
    if (!existingIds.has(candidate)) return candidate;
  }
  return `${base}-${Date.now()}`;
}

function numberValue(value: unknown): number | null {
  return typeof value === "number" && Number.isFinite(value) ? value : null;
}

function colorInputValue(value?: string): string {
  return value && /^#[0-9a-f]{6}$/i.test(value) ? value : "#111827";
}

function parseProviderJson(value: string): Provider {
  const parsed = JSON.parse(value || "{}") as Provider;
  if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) {
    throw new Error("advanced provider JSON must be an object");
  }
  return parsed;
}

function asRecord(value: unknown): Record<string, unknown> {
  return value && typeof value === "object" && !Array.isArray(value)
    ? { ...(value as Record<string, unknown>) }
    : {};
}

function getString(value: unknown): string | null {
  return typeof value === "string" && value.trim() ? value : null;
}

function env(provider: Provider): Record<string, unknown> {
  return asRecord(asRecord(provider.settingsConfig).env);
}

function setting(provider: Provider, keys: string[]): string | null {
  const settings = asRecord(provider.settingsConfig);
  const environment = env(provider);
  for (const key of keys) {
    const direct = getString(settings[key]);
    if (direct) return direct;
    const nested = getString(environment[key]);
    if (nested) return nested;
  }
  return null;
}

function providerSettingJson(provider: Provider, keys: string[]): string {
  const settings = asRecord(provider.settingsConfig);
  for (const key of keys) {
    if (settings[key] !== undefined && settings[key] !== null) {
      return jsonText(settings[key]);
    }
  }
  return "";
}

function setOptionalJsonSetting(
  settings: Record<string, unknown>,
  key: string,
  raw: string,
  label: string,
) {
  const parsed = optionalJson(raw, label);
  if (parsed === undefined) {
    delete settings[key];
  } else {
    settings[key] = parsed;
  }
}

function setPricingJsonSetting(settings: Record<string, unknown>, raw: string) {
  const parsed = optionalJson(raw, "pricing");
  if (parsed === undefined) {
    delete settings.pricing;
    delete settings.modelPricing;
  } else {
    settings.pricing = parsed;
    delete settings.modelPricing;
  }
}

function optionalJson(raw: string, label: string): unknown {
  const trimmed = raw.trim();
  if (!trimmed) return undefined;
  try {
    return JSON.parse(trimmed) as unknown;
  } catch (reason) {
    const suffix = reason instanceof Error ? reason.message : String(reason);
    throw new Error(`${label} JSON is invalid: ${suffix}`);
  }
}

function jsonText(value: unknown): string {
  return value === undefined || value === null ? "" : JSON.stringify(value, null, 2);
}

function baseUrlFromProvider(provider: Provider, app: AppKind): string | null {
  const keys =
    app === "claude"
      ? ["ANTHROPIC_BASE_URL", "BASE_URL", "baseUrl", "base_url"]
      : app === "codex"
        ? ["OPENAI_BASE_URL", "CODEX_BASE_URL", "BASE_URL", "baseUrl", "base_url"]
        : ["GOOGLE_GEMINI_BASE_URL", "GEMINI_BASE_URL", "BASE_URL", "baseUrl", "base_url"];
  return setting(provider, keys);
}

function apiKeyFromProvider(provider: Provider, entry: ProviderMatrixEntry): string | null {
  return setting(provider, [entry.defaults.key]);
}

function modelFromProvider(provider: Provider): string | null {
  return setting(provider, ["model", "MODEL"]);
}

function apiFormatFromProvider(provider: Provider): string | null {
  return (
    getString(provider.meta?.apiFormat) ||
    setting(provider, ["apiFormat", "api_format"])
  );
}

function baseUrlKeyFor(app: AppKind, entry: ProviderMatrixEntry): string {
  const fromTemplate = entry.templateEnv.find((key) => key.includes("BASE_URL"));
  if (fromTemplate) return fromTemplate;
  if (app === "claude") return "ANTHROPIC_BASE_URL";
  if (app === "codex") return "OPENAI_BASE_URL";
  return "GOOGLE_GEMINI_BASE_URL";
}

function authProviderForType(providerTypeId: string): string {
  if (providerTypeId === "claude_auth") return "claude_oauth";
  if (providerTypeId === "gemini_cli") return "google_gemini_oauth";
  return providerTypeId;
}

function accountProviderTypeFor(providerTypeId: string): string {
  if (providerTypeId === "claude_auth") return "claude_oauth";
  if (providerTypeId === "gemini_cli") return "gemini_cli";
  return providerTypeId;
}

function accountMatchesProviderType(account: AccountRecord, providerTypeId: string): boolean {
  return account.providerType === providerTypeId || account.providerType === accountProviderTypeFor(providerTypeId);
}

function providerKey(app: AppKind, providerId: string): string {
  return `${app}:${providerId}`;
}

function capabilityForProvider(
  provider: StoredProvider,
  capabilitiesByType: Map<string, AccountManagerCapability>,
): AccountManagerCapability | undefined {
  return (
    capabilitiesByType.get(provider.providerTypeId) ||
    capabilitiesByType.get(accountProviderTypeFor(provider.providerTypeId))
  );
}

function accountForProvider(
  provider: StoredProvider,
  accountsById: Map<string, AccountRecord>,
): AccountRecord | undefined {
  const accountId = provider.provider.meta?.authBinding?.accountId;
  if (accountId) return accountsById.get(accountId);
  return undefined;
}

function appLabel(app: AppKind): string {
  return apps.find((item) => item.id === app)?.label || app;
}

function accountSummary(account: AccountRecord): string {
  const parts = [
    account.email || account.id,
    account.subscriptionLevel || null,
    account.quotaPercent == null ? null : `${account.quotaPercent.toFixed(1)}%`,
  ].filter(Boolean);
  return parts.join(" · ");
}

function providerModelCatalogPlaceholder(app: AppKind): string {
  const model =
    app === "gemini"
      ? "gemini-2.5-pro"
      : app === "codex"
        ? "gpt-5.5"
        : "claude-sonnet-5";
  return JSON.stringify(
    {
      models: [
        {
          id: model,
          upstreamModel: model,
          displayName: model,
        },
      ],
    },
    null,
    2,
  );
}

function providerModelMappingPlaceholder(app: AppKind): string {
  const target =
    app === "gemini"
      ? "gemini-2.5-pro"
      : app === "codex"
        ? "gpt-5.5"
        : "claude-sonnet-5";
  return JSON.stringify(
    {
      rules: [
        {
          match: "*",
          upstreamModel: target,
        },
      ],
    },
    null,
    2,
  );
}

function providerPricingPlaceholder(): string {
  return JSON.stringify(
    {
      default: {
        inputUsdPerMillion: 3,
        outputUsdPerMillion: 15,
        cacheReadUsdPerMillion: 0.3,
        cacheCreationUsdPerMillion: 3.75,
      },
    },
    null,
    2,
  );
}

function limitLine(usage: number, limit?: number | null): string {
  if (limit == null) return `${formatUsd(usage)} / -`;
  return `${formatUsd(usage)} / ${formatUsd(limit)}`;
}

function formatUsd(value: number): string {
  if (!Number.isFinite(value)) return "-";
  if (Math.abs(value) >= 1) return `$${value.toFixed(2)}`;
  return `$${value.toFixed(4)}`;
}

function formatTime(value?: number | null): string {
  if (!value) return "expires -";
  const millis = value < 10_000_000_000 ? value * 1000 : value;
  const date = new Date(millis);
  if (Number.isNaN(date.getTime())) return "expires -";
  return new Intl.DateTimeFormat(undefined, { month: "short", day: "numeric" }).format(date);
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
