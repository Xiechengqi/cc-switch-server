import { invokeCommand } from "@/lib/runtime";
import { isTauriRuntime } from "@/lib/runtime";
import type { UnlistenFn } from "@tauri-apps/api/event";
import type { Provider } from "@/types";
import type { AppId } from "./types";

export interface ProviderSortUpdate {
  id: string;
  sortIndex: number;
}

export type ProviderUpstreamProtocol =
  "anthropic_messages" | "open_ai_chat" | "open_ai_responses" | "gemini_native";

export type ProviderAuthScheme =
  | "none"
  | "api_key"
  | "bearer"
  | "oauth"
  | "aws_sig_v4"
  | "custom_header"
  | "query";

export interface ProviderCustomBinding {
  upstreamProtocol: ProviderUpstreamProtocol;
  authScheme: ProviderAuthScheme;
}

export interface ProviderIdentityView {
  status:
    | "bound"
    | "profile_upgrade_available"
    | "adoption_available"
    | "legacy_compat"
    | "needs_attention";
  suggestedProfileId?: string;
  currentProfileSchemaRevision?: number;
  warning?: string;
}

export interface ProviderResource {
  app: "claude" | "codex" | "gemini";
  provider: Provider;
  providerType: string;
  providerTypeId: string;
  revision: number;
  profileId?: string;
  profileSchemaRevision?: number;
  customBinding?: ProviderCustomBinding;
  identity: ProviderIdentityView;
  orderIndex?: number;
  credentialConfigured: boolean;
  credentialSlots: string[];
}

export type ProviderCredentialPatch =
  | { action: "keep" }
  | { action: "replace"; value: string }
  | { action: "clear" };

export type ProviderCredentialPatches = Record<string, ProviderCredentialPatch>;

export interface ProviderWriteOptions {
  profileId?: string;
  customBinding?: ProviderCustomBinding;
  expectedRevision?: number;
  clientRequestId?: string;
  credentialPatches?: ProviderCredentialPatches;
}

export interface ProviderIdentityActionPreview {
  previewToken: string;
  action: "adopt_profile" | "rebind_custom" | "clone_as_custom";
  sourceRevision: number;
  warnings: string[];
}

export interface ProviderIdentityActionResult {
  ok: boolean;
  mode: "preview" | "apply";
  preview: ProviderIdentityActionPreview;
  stored?: ProviderResource;
}

export interface ProviderStoreMigrationItem {
  app: "claude" | "codex" | "gemini";
  providerId: string;
  status: "ready" | "blocked";
  blockerCodes: string[];
}

export interface ProviderStoreMigrationReport {
  sourceFormat: "s1" | "s2";
  targetFormat: "s1" | "s2";
  keySource: "environment" | "file" | "file_will_be_created" | "unavailable";
  providerCount: number;
  readyCount: number;
  blockedCount: number;
  runtimePlanParity: boolean;
  referenceFingerprint: string;
  canApply: boolean;
  items: ProviderStoreMigrationItem[];
}

export interface ProviderSwitchEvent {
  appType: AppId;
  providerId: string;
}

export interface SwitchResult {
  warnings: string[];
}

export interface OpenTerminalOptions {
  cwd?: string;
}

export interface ClaudeDesktopStatus {
  supported: boolean;
  configured: boolean;
  appliedId?: string | null;
  profilePath?: string | null;
  configLibraryPath?: string | null;
  mode?: "direct" | "proxy" | null;
  expectedBaseUrl?: string | null;
  actualBaseUrl?: string | null;
  proxyRunning: boolean;
  staleRawModels: boolean;
  missingRouteMappings: boolean;
  gatewayTokenConfigured: boolean;
}

export interface ClaudeDesktopDefaultRoute {
  routeId: string;
  envKey: string;
  supports1m: boolean;
}

export const providersApi = {
  async getAll(appId: AppId): Promise<Record<string, Provider>> {
    return await invokeCommand("get_providers", { app: appId });
  },

  async getResources(appId: AppId): Promise<ProviderResource[]> {
    return await invokeCommand("get_provider_resources", { app: appId });
  },

  async getCredential(
    appId: AppId,
    providerId: string,
    slot: string,
  ): Promise<string> {
    return await invokeCommand(
      "get_provider_credential",
      {
        app: appId,
        providerId,
        slot,
      },
      { cache: "no-store" },
    );
  },

  async getStoreMigration(): Promise<ProviderStoreMigrationReport> {
    return await invokeCommand("get_provider_store_migration");
  },

  async getCurrent(appId: AppId): Promise<string> {
    return await invokeCommand("get_current_provider", { app: appId });
  },

  async add(
    provider: Provider,
    appId: AppId,
    addToLive?: boolean,
    options: ProviderWriteOptions = {},
  ): Promise<ProviderResource> {
    return await invokeCommand("add_provider", {
      provider,
      app: appId,
      addToLive,
      ...options,
    });
  },

  async update(
    provider: Provider,
    appId: AppId,
    originalId?: string,
    options: ProviderWriteOptions = {},
  ): Promise<ProviderResource> {
    return await invokeCommand("update_provider", {
      provider,
      app: appId,
      originalId,
      ...options,
    });
  },

  async delete(
    id: string,
    appId: AppId,
    expectedRevision?: number,
  ): Promise<boolean> {
    return await invokeCommand("delete_provider", {
      id,
      app: appId,
      ...(expectedRevision === undefined ? {} : { expectedRevision }),
    });
  },

  async adoptProfile(options: {
    app: AppId;
    providerId: string;
    expectedRevision: number;
    profileId: string;
    accountId?: string;
    mode: "preview" | "apply";
    previewToken?: string;
  }): Promise<ProviderIdentityActionResult> {
    return await invokeCommand("adopt_provider_profile", options);
  },

  async rebindCustom(options: {
    app: AppId;
    providerId: string;
    expectedRevision: number;
    customBinding: ProviderCustomBinding;
    credentialPatches?: ProviderCredentialPatches;
    mode: "preview" | "apply";
    previewToken?: string;
  }): Promise<ProviderIdentityActionResult> {
    return await invokeCommand("rebind_custom_provider", options);
  },

  async cloneAsCustom(options: {
    app: AppId;
    providerId: string;
    expectedRevision: number;
    targetProviderId: string;
    targetName: string;
    customBinding: ProviderCustomBinding;
    clientRequestId: string;
    mode: "preview" | "apply";
    previewToken?: string;
  }): Promise<ProviderIdentityActionResult> {
    return await invokeCommand("clone_provider_as_custom", options);
  },

  /**
   * Remove provider from live config only (for additive mode apps like OpenCode)
   * Does NOT delete from database - provider remains in the list
   */
  async removeFromLiveConfig(id: string, appId: AppId): Promise<boolean> {
    return await invokeCommand("remove_provider_from_live_config", {
      id,
      app: appId,
    });
  },

  async switch(id: string, appId: AppId): Promise<SwitchResult> {
    return await invokeCommand("switch_provider", { id, app: appId });
  },

  async clearCurrent(appId: AppId): Promise<SwitchResult> {
    return await invokeCommand("clear_current_provider", { app: appId });
  },

  async ensureClaudeDesktopOfficialProvider(): Promise<boolean> {
    return await invokeCommand("ensure_claude_desktop_official_provider");
  },

  async getClaudeDesktopStatus(): Promise<ClaudeDesktopStatus> {
    return await invokeCommand("get_claude_desktop_status");
  },

  async getClaudeDesktopDefaultRoutes(): Promise<ClaudeDesktopDefaultRoute[]> {
    return await invokeCommand("get_claude_desktop_default_routes");
  },

  async updateTrayMenu(): Promise<boolean> {
    return await invokeCommand("update_tray_menu");
  },

  async updateSortOrder(
    updates: ProviderSortUpdate[],
    appId: AppId,
  ): Promise<boolean> {
    return await invokeCommand("update_providers_sort_order", {
      updates,
      app: appId,
    });
  },

  async onSwitched(
    handler: (event: ProviderSwitchEvent) => void,
  ): Promise<UnlistenFn> {
    if (!isTauriRuntime()) {
      return () => undefined;
    }
    const { listen } = await import("@tauri-apps/api/event");
    return await listen("provider-switched", (event) => {
      const payload = event.payload as ProviderSwitchEvent;
      handler(payload);
    });
  },

  /**
   * 打开指定提供商的终端
   * 任何提供商都可以打开终端，不受是否为当前激活提供商的限制
   * 终端会使用该提供商特定的 API 配置，不影响全局设置
   */
  async openTerminal(
    providerId: string,
    appId: AppId,
    options?: OpenTerminalOptions,
  ): Promise<boolean> {
    const { cwd } = options ?? {};
    return await invokeCommand("open_provider_terminal", {
      providerId,
      app: appId,
      cwd,
    });
  },

  /**
   * 从 OpenCode live 配置导入供应商到数据库
   * OpenCode 特有功能：由于累加模式，用户可能已在 opencode.json 中配置供应商
   */
  async importOpenCodeFromLive(): Promise<number> {
    return await invokeCommand("import_opencode_providers_from_live");
  },

  /**
   * 获取 OpenCode live 配置中的供应商 ID 列表
   * 用于前端判断供应商是否已添加到 opencode.json
   */
  async getOpenCodeLiveProviderIds(): Promise<string[]> {
    return await invokeCommand("get_opencode_live_provider_ids");
  },

  /**
   * 获取 OpenClaw live 配置中的供应商 ID 列表
   * 用于前端判断供应商是否已添加到 openclaw.json
   */
  async getOpenClawLiveProviderIds(): Promise<string[]> {
    return await invokeCommand("get_openclaw_live_provider_ids");
  },

  /**
   * 获取 Hermes live 配置中的供应商 ID 列表
   * 用于前端判断供应商是否已添加到 Hermes 配置
   */
  async getHermesLiveProviderIds(): Promise<string[]> {
    return await invokeCommand("get_hermes_live_provider_ids");
  },

  /**
   * 从 OpenClaw live 配置导入供应商到数据库
   * OpenClaw 特有功能：由于累加模式，用户可能已在 openclaw.json 中配置供应商
   */
  async importOpenClawFromLive(): Promise<number> {
    return await invokeCommand("import_openclaw_providers_from_live");
  },

  /**
   * 从 Hermes live 配置导入供应商到数据库
   * Hermes 特有功能：由于累加模式，用户可能已在 Hermes 配置中配置供应商
   */
  async importHermesFromLive(): Promise<number> {
    return await invokeCommand("import_hermes_providers_from_live");
  },
};
