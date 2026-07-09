import type { AppId } from "@/lib/api/types";
import { isWindows } from "@/lib/platform";

export interface ToolVersion {
  name: string;
  version: string | null;
  latest_version: string | null;
  error: string | null;
  installed_but_broken: boolean;
  env_type: "windows" | "wsl" | "macos" | "linux" | "unknown";
  wsl_distro: string | null;
}

export const TOOL_NAMES = [
  "claude",
  "codex",
  "gemini",
  "opencode",
  "openclaw",
  "hermes",
] as const;

export type ToolName = (typeof TOOL_NAMES)[number];
export type ToolLifecycleAction = "install" | "update";

export type WslShellPreference = {
  wslShell?: string | null;
  wslShellFlag?: string | null;
};

export const WSL_SHELL_OPTIONS = ["sh", "bash", "zsh", "fish", "dash"] as const;
export const WSL_SHELL_FLAG_OPTIONS = ["-lic", "-lc", "-c"] as const;

export const ENV_BADGE_CONFIG: Record<
  string,
  { labelKey: string; className: string }
> = {
  wsl: {
    labelKey: "settings.envBadge.wsl",
    className:
      "bg-orange-500/10 text-orange-600 dark:text-orange-400 border-orange-500/20",
  },
  windows: {
    labelKey: "settings.envBadge.windows",
    className:
      "bg-blue-500/10 text-blue-600 dark:text-blue-400 border-blue-500/20",
  },
  macos: {
    labelKey: "settings.envBadge.macos",
    className:
      "bg-gray-500/10 text-gray-600 dark:text-gray-400 border-gray-500/20",
  },
  linux: {
    labelKey: "settings.envBadge.linux",
    className:
      "bg-green-500/10 text-green-600 dark:text-green-400 border-green-500/20",
  },
};

const posixScriptInstallCommand = (url: string) =>
  `bash -c 'tmp=$(mktemp) && curl -fsSL ${url} -o $tmp && bash $tmp; status=$?; rm -f $tmp; exit $status'`;

const HERMES_WINDOWS_INSTALL_SCRIPT =
  "irm https://raw.githubusercontent.com/NousResearch/hermes-agent/main/scripts/install.ps1 | iex";

const powershellEncodedCommand = (script: string): string => {
  let binary = "";
  for (let i = 0; i < script.length; i += 1) {
    const code = script.charCodeAt(i);
    binary += String.fromCharCode(code & 0xff, code >> 8);
  }
  return btoa(binary);
};

const HERMES_WINDOWS_INSTALL_COMMAND = `powershell -NoProfile -ExecutionPolicy Bypass -EncodedCommand ${powershellEncodedCommand(
  HERMES_WINDOWS_INSTALL_SCRIPT,
)}`;

const POSIX_ONE_CLICK_INSTALL_COMMANDS = `# Claude Code
${posixScriptInstallCommand("https://claude.ai/install.sh")} || npm i -g @anthropic-ai/claude-code@latest
# Codex
npm i -g @openai/codex@latest
# Gemini CLI
npm i -g @google/gemini-cli@latest
# OpenCode
${posixScriptInstallCommand("https://opencode.ai/install")} || npm i -g opencode-ai@latest
# OpenClaw
npm i -g openclaw@latest
# Hermes
${posixScriptInstallCommand("https://raw.githubusercontent.com/NousResearch/hermes-agent/main/scripts/install.sh")}`;

const WINDOWS_ONE_CLICK_INSTALL_COMMANDS = `# Claude Code
npm i -g @anthropic-ai/claude-code@latest
# Codex
npm i -g @openai/codex@latest
# Gemini CLI
npm i -g @google/gemini-cli@latest
# OpenCode
npm i -g opencode-ai@latest
# OpenClaw
npm i -g openclaw@latest
# Hermes
${HERMES_WINDOWS_INSTALL_COMMAND}`;

export const ONE_CLICK_INSTALL_COMMANDS = isWindows()
  ? WINDOWS_ONE_CLICK_INSTALL_COMMANDS
  : POSIX_ONE_CLICK_INSTALL_COMMANDS;

export const TOOL_DISPLAY_NAMES: Record<ToolName, string> = {
  claude: "Claude Code",
  codex: "Codex",
  gemini: "Gemini CLI",
  opencode: "OpenCode",
  openclaw: "OpenClaw",
  hermes: "Hermes",
};

export const TOOL_APP_IDS: Record<ToolName, AppId> = {
  claude: "claude",
  codex: "codex",
  gemini: "gemini",
  opencode: "opencode",
  openclaw: "openclaw",
  hermes: "hermes",
};

export function toolDisplayName(tool: string): string {
  return TOOL_DISPLAY_NAMES[tool as ToolName] ?? tool;
}

export const TOOL_VERSIONS_CACHE_TTL_MS = 10 * 60 * 1000;

type ToolVersionsCache = { data: ToolVersion[]; at: number };

let toolVersionsCacheState: ToolVersionsCache | null = null;

export function getToolVersionsCache(): ToolVersionsCache | null {
  return toolVersionsCacheState;
}

export function setToolVersionsCache(cache: ToolVersionsCache | null): void {
  toolVersionsCacheState = cache;
}

export function updateToolVersionsCache(
  updater: (current: ToolVersionsCache | null) => ToolVersionsCache | null,
): void {
  toolVersionsCacheState = updater(toolVersionsCacheState);
}

export function mergeToolVersions(
  prev: ToolVersion[],
  updated: ToolVersion[],
): ToolVersion[] {
  if (prev.length === 0) return updated;
  const byName = new Map(updated.map((t) => [t.name, t]));
  const merged = prev.map((t) => byName.get(t.name) ?? t);
  const existing = new Set(prev.map((t) => t.name));
  for (const u of updated) {
    if (!existing.has(u.name)) merged.push(u);
  }
  return merged;
}
