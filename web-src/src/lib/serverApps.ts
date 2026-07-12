import type { AppId } from "@/lib/api";

/** Apps exposed by the token server Web UI and HTTP proxy. */
export const SERVER_MAIN_APPS: AppId[] = ["claude", "codex", "gemini"];

export function isServerMainApp(app: string): app is AppId {
  return (SERVER_MAIN_APPS as string[]).includes(app);
}
