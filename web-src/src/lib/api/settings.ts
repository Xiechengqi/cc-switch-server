import { invokeCommand } from "@/lib/runtime";
import type { Settings } from "@/types";

export const settingsApi = {
  async get(): Promise<Settings> {
    return await invokeCommand("get_settings");
  },

  async save(settings: Settings): Promise<boolean> {
    return await invokeCommand("save_settings", { settings });
  },

  async openExternal(url: string): Promise<void> {
    let parsed: URL;
    try {
      parsed = new URL(url);
    } catch {
      throw new Error("Invalid URL");
    }
    if (parsed.protocol !== "http:" && parsed.protocol !== "https:") {
      throw new Error("Unsupported URL scheme");
    }
    window.open(parsed.href, "_blank", "noopener,noreferrer");
  },

  async getLogConfig(): Promise<LogConfig> {
    return await invokeCommand("get_log_config");
  },

  async setLogConfig(config: LogConfig): Promise<boolean> {
    return await invokeCommand("set_log_config", { config });
  },

  async getApiManagement(): Promise<ApiManagementConfig> {
    return await invokeCommand("get_api_management");
  },

  async setApiManagement(
    config: ApiManagementConfig,
  ): Promise<ApiManagementConfig> {
    return await invokeCommand("set_api_management", { config });
  },

  async generateDebugToken(ttlHours: number): Promise<GeneratedDebugToken> {
    return await invokeCommand("generate_debug_token", { ttlHours });
  },

  async revokeDebugToken(): Promise<{ ok: boolean }> {
    return await invokeCommand("revoke_debug_token");
  },
};

export interface LogConfig {
  enabled: boolean;
  level: "error" | "warn" | "info" | "debug" | "trace";
  collectionEnabled: boolean;
}

export interface ApiManagementConfig {
  diagnosticsEnabled: boolean;
  logEnabled: boolean;
  restartEnabled: boolean;
  upgradeEnabled: boolean;
  logTailLines: number;
  tokenConfigured?: boolean;
  tokenExpiresAtMs?: number | null;
}

export interface GeneratedDebugToken {
  token: string;
  expiresAtMs: number;
  ttlHours: number;
}

export interface BackupEntry {
  filename: string;
  sizeBytes: number;
  createdAt: string;
}

export const backupsApi = {
  async createDbBackup(): Promise<string> {
    return await invokeCommand("create_db_backup");
  },

  async listDbBackups(): Promise<BackupEntry[]> {
    return await invokeCommand("list_db_backups");
  },

  async restoreDbBackup(filename: string): Promise<string> {
    return await invokeCommand("restore_db_backup", { filename });
  },

  async renameDbBackup(oldFilename: string, newName: string): Promise<string> {
    return await invokeCommand("rename_db_backup", { oldFilename, newName });
  },

  async deleteDbBackup(filename: string): Promise<void> {
    await invokeCommand("delete_db_backup", { filename });
  },
};
