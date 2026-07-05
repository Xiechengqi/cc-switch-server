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
  BarChart3,
  CheckCircle2,
  Copy,
  Download,
  FlaskConical,
  GripVertical,
  Link2,
  ListPlus,
  Loader2,
  Minus,
  Pencil,
  Play,
  Plus,
  RefreshCw,
  Search,
  ServerCog,
  Trash2,
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
  FailoverSnapshot,
  fetchProviderModels,
  getCurrentProvider,
  loadProviderListData,
  Provider,
  ProviderBreaker,
  ProviderHealth,
  ProviderMatrix,
  ProviderMatrixEntry,
  ProviderPresetSummary,
  ProviderPresetsByApp,
  ProviderLimitStatus,
  resetFailoverProvider,
  saveProvider,
  StoredProvider,
  switchProvider,
  updateFailoverApp,
  testProvider,
  updateProvidersSortOrder,
} from "@/lib/api";
import { useI18n } from "@/lib/i18n";
import { inferIconForText } from "@/config/iconInference";
import { ColorPicker } from "@/components/ColorPicker";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { IconAction } from "@/components/IconAction";
import { LoadingBlock } from "@/components/LoadingBlock";
import { KeyValue } from "@/components/KeyValue";
import { IconPicker } from "@/components/IconPicker";
import JsonEditor from "@/components/JsonEditor";
import { FailoverPriorityBadge } from "@/components/providers/FailoverPriorityBadge";
import { ProviderEmptyState } from "@/components/providers/ProviderEmptyState";
import { ProviderHealthIndicator } from "@/components/providers/ProviderHealthIndicator";
import { ProviderListToolbar } from "@/components/providers/ProviderListToolbar";
import { ProviderCatalogModal } from "@/components/providers/ProviderCatalogModal";
import { ProviderFormModal } from "@/components/providers/ProviderFormModal";
import { SortableProviderCard } from "@/components/providers/ProviderCard";
import { apiKeyFromProvider, apiFormatFromProvider, appLabel, asRecord, baseUrlFromProvider, getString, modelFromProvider, setting } from "@/components/providers/providerDisplay";
import { ProviderIcon } from "@/components/ProviderIcon";
import { StatusPill } from "@/components/StatusPill";
import { presetIcon, storedProviderIcon } from "@/lib/provider-icons";

const apps: Array<{ id: AppKind; label: string }> = [
  { id: "claude", label: "Claude" },
  { id: "codex", label: "Codex" },
  { id: "gemini", label: "Gemini" },
];

interface ProviderListState {
  providers: StoredProvider[];
  matrix: ProviderMatrix | null;
  health: ProviderHealth[];
  accounts: AccountRecord[];
  capabilities: AccountManagerCapability[];
  limits: ProviderLimitStatus[];
  presets: ProviderPresetsByApp;
  failover: FailoverSnapshot;
}

export interface ProviderDraft {
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
  isFullUrl: boolean;
  endpointAutoSelect: boolean;
  customUserAgent: string;
  localProxyHeadersJson: string;
  localProxyBodyJson: string;
  codexSupportsThinking: boolean;
  codexSupportsEffort: boolean;
  codexThinkingParam: string;
  codexEffortParam: string;
  codexEffortValueMode: string;
  codexOutputFormat: string;
  modelCatalogJson: string;
  modelMappingJson: string;
  pricingJson: string;
  advancedJson: string;
}

export function ProviderList({
  activeApp: controlledActiveApp,
  onActiveAppChange,
  onOpenImportExport,
  onOpenUsage,
}: {
  activeApp?: AppKind;
  onActiveAppChange?: (app: AppKind) => void;
  onOpenImportExport?: () => void;
  onOpenUsage?: (target: { app: AppKind; providerId: string; tab: "logs" | "limits" }) => void;
}) {
  const { t, tx } = useI18n();
  const [localActiveApp, setLocalActiveApp] = useState<AppKind>("claude");
  const activeApp = controlledActiveApp || localActiveApp;
  const setActiveApp = onActiveAppChange || setLocalActiveApp;
  const [data, setData] = useState<ProviderListState>({
    providers: [],
    matrix: null,
    health: [],
    accounts: [],
    capabilities: [],
    limits: [],
    presets: { claude: [], codex: [], gemini: [] },
    failover: { apps: {}, breakers: [] },
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
      setData(await loadProviderListData());
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
  const activeFailoverConfig = data.failover.apps[activeApp];
  const activeFailoverQueue = activeFailoverConfig?.providerQueue || [];
  const activeFailoverEnabled = Boolean(activeFailoverConfig?.enabled);
  const healthById = new Map(
    data.health
      .filter((health) => health.app === activeApp)
      .map((health) => [health.providerId, health]),
  );
  const accountsById = new Map(data.accounts.map((account) => [account.id, account]));
  const breakerByProviderKey = new Map(
    data.failover.breakers.map((breaker) => [providerKey(breaker.app, breaker.providerId), breaker]),
  );
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
    action: "test" | "network" | "stream" | "models" | "switch" | "duplicate" | "resetFailover" | "delete",
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
      if (action === "resetFailover") {
        const breaker = await resetFailoverProvider(provider.app, provider.provider.id);
        setData((current) => ({
          ...current,
          failover: {
            ...current.failover,
            breakers: current.failover.breakers.map((item) =>
              item.app === breaker.app && item.providerId === breaker.providerId ? breaker : item,
            ),
          },
        }));
        setResultById((current) => ({
          ...current,
          [provider.provider.id]: tx("Reset failover breaker"),
        }));
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

  async function toggleProviderFailover(provider: StoredProvider, enabled: boolean) {
    const currentConfig = data.failover.apps[provider.app];
    const currentQueue = currentConfig?.providerQueue || [];
    const providerId = provider.provider.id;
    const nextQueue = enabled
      ? [...currentQueue.filter((id) => id !== providerId), providerId]
      : currentQueue.filter((id) => id !== providerId);
    const key = `${provider.app}:${providerId}:failover`;
    setBusyId(key);
    setError(null);
    try {
      const config = await updateFailoverApp(provider.app, {
        enabled: currentConfig?.enabled,
        providerQueue: nextQueue,
      });
      setData((current) => ({
        ...current,
        failover: {
          ...current.failover,
          apps: {
            ...current.failover.apps,
            [provider.app]: config,
          },
        },
      }));
      setResultById((current) => ({
        ...current,
        [providerId]: enabled ? tx("Added to failover queue") : tx("Removed from failover queue"),
      }));
    } catch (reason) {
      setError(errorMessage(reason));
      await refresh();
    } finally {
      setBusyId(null);
    }
  }

  return (
    <div className="provider-list">
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
        <LoadingBlock label="server.providers.loading" />
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
                  const failoverIndex = activeFailoverQueue.indexOf(provider.provider.id);
                  return (
                    <SortableProviderCard
                      key={`${provider.app}:${provider.provider.id}`}
                      provider={provider}
                      priority={priority}
                      failoverEnabled={activeFailoverEnabled}
                      failoverPriority={failoverIndex >= 0 ? failoverIndex + 1 : null}
                      inFailoverQueue={failoverIndex >= 0}
                      breaker={breakerByProviderKey.get(providerKey(provider.app, provider.provider.id))}
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
                      onToggleFailover={(enabled) => void toggleProviderFailover(provider, enabled)}
                      onOpenUsage={
                        onOpenUsage
                          ? () =>
                              onOpenUsage({
                                app: provider.app,
                                providerId: provider.provider.id,
                                tab: limitByProviderKey.has(providerKey(provider.app, provider.provider.id))
                                  ? "limits"
                                  : "logs",
                              })
                          : undefined
                      }
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
    isFullUrl: false,
    endpointAutoSelect: false,
    customUserAgent: "",
    localProxyHeadersJson: "",
    localProxyBodyJson: "",
    codexSupportsThinking: false,
    codexSupportsEffort: false,
    codexThinkingParam: "thinking",
    codexEffortParam: "reasoning_effort",
    codexEffortValueMode: "passthrough",
    codexOutputFormat: "auto",
    modelCatalogJson: "",
    modelMappingJson: "",
    pricingJson: "",
    advancedJson: JSON.stringify(provider, null, 2),
  };
}

function editDraft(stored: StoredProvider, entry: ProviderMatrixEntry): ProviderDraft {
  const provider = stored.provider;
  const meta = asRecord(provider.meta);
  const requestOverrides = asRecord(meta.localProxyRequestOverrides);
  const codexReasoning = asRecord(meta.codexChatReasoning);
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
    isFullUrl: boolValue(meta.isFullUrl),
    endpointAutoSelect: boolValue(meta.endpointAutoSelect),
    customUserAgent: getString(meta.customUserAgent) || "",
    localProxyHeadersJson: jsonText(requestOverrides.headers),
    localProxyBodyJson: jsonText(requestOverrides.body),
    codexSupportsThinking: boolValue(codexReasoning.supportsThinking),
    codexSupportsEffort: boolValue(codexReasoning.supportsEffort),
    codexThinkingParam: getString(codexReasoning.thinkingParam) || "thinking",
    codexEffortParam: getString(codexReasoning.effortParam) || "reasoning_effort",
    codexEffortValueMode: getString(codexReasoning.effortValueMode) || "passthrough",
    codexOutputFormat: getString(codexReasoning.outputFormat) || "auto",
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
  setOptionalBooleanMeta(meta, "isFullUrl", draft.isFullUrl);
  setOptionalBooleanMeta(meta, "endpointAutoSelect", draft.endpointAutoSelect);
  setOptionalStringMeta(meta, "customUserAgent", draft.customUserAgent);
  setOptionalRequestOverrides(meta, draft.localProxyHeadersJson, draft.localProxyBodyJson);
  setOptionalCodexReasoning(meta, draft);
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

function setOptionalBooleanMeta(meta: Record<string, unknown>, key: string, value: boolean) {
  if (value) {
    meta[key] = true;
  } else {
    delete meta[key];
  }
}

function setOptionalStringMeta(meta: Record<string, unknown>, key: string, value: string) {
  const trimmed = value.trim();
  if (trimmed) {
    if (/[\u0000-\u001f\u007f]/.test(trimmed)) {
      throw new Error(`${key} must not contain control characters`);
    }
    meta[key] = trimmed;
  } else {
    delete meta[key];
  }
}

function setOptionalRequestOverrides(
  meta: Record<string, unknown>,
  headersRaw: string,
  bodyRaw: string,
) {
  const headers = optionalJsonObject(headersRaw, "headers overrides");
  const body = optionalJsonObject(bodyRaw, "body overrides");
  const overrides = asRecord(meta.localProxyRequestOverrides);
  if (headers !== undefined) {
    overrides.headers = headers;
  } else {
    delete overrides.headers;
  }
  if (body !== undefined) {
    overrides.body = body;
  } else {
    delete overrides.body;
  }
  if (Object.keys(overrides).length) {
    meta.localProxyRequestOverrides = overrides;
  } else {
    delete meta.localProxyRequestOverrides;
  }
}

function optionalJsonObject(raw: string, label: string): Record<string, unknown> | undefined {
  const parsed = optionalJson(raw, label);
  if (parsed === undefined) return undefined;
  if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) {
    throw new Error(`${label} JSON must be an object`);
  }
  return parsed as Record<string, unknown>;
}

function setOptionalCodexReasoning(meta: Record<string, unknown>, draft: ProviderDraft) {
  if (draft.app !== "codex" || draft.apiFormat !== "openai_chat") {
    delete meta.codexChatReasoning;
    return;
  }
  const reasoning: Record<string, unknown> = {};
  if (draft.codexSupportsThinking || draft.codexSupportsEffort) {
    reasoning.supportsThinking = draft.codexSupportsThinking || draft.codexSupportsEffort;
    reasoning.supportsEffort = draft.codexSupportsEffort;
    reasoning.thinkingParam = draft.codexThinkingParam || "thinking";
    reasoning.effortParam = draft.codexSupportsEffort
      ? draft.codexEffortParam || "reasoning_effort"
      : "none";
    reasoning.effortValueMode = draft.codexEffortValueMode || "passthrough";
    reasoning.outputFormat = draft.codexOutputFormat || "auto";
  }
  if (Object.keys(reasoning).length) {
    meta.codexChatReasoning = reasoning;
  } else {
    delete meta.codexChatReasoning;
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

function boolValue(value: unknown): boolean {
  return value === true;
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

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
