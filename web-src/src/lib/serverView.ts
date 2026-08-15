export const SERVER_VIEW_STORAGE_KEY = "cc-switch-server-view";
export const SERVER_VIEW_QUERY_PARAM = "view";
export const SERVER_VIEW_EMBED_PARAM = "embed";

export const SERVER_VIEWS = [
  "providers",
  "shares",
  "settings",
  "terminal",
] as const;

export type ServerView = (typeof SERVER_VIEWS)[number];

export function isServerView(value: string | null | undefined): value is ServerView {
  return (
    value === "providers" ||
    value === "shares" ||
    value === "settings" ||
    value === "terminal"
  );
}

export function requestedServerView(
  search: string | null | undefined,
): ServerView | null {
  if (!search) return null;
  const query = search.startsWith("?") ? search.slice(1) : search;
  const params = new URLSearchParams(query);
  const view = params.get(SERVER_VIEW_QUERY_PARAM);
  return isServerView(view) ? view : null;
}

/**
 * True when a host page (the Router console window) already frames this app.
 * Embedded visits drop our own chrome so the terminal reads as a plain shell.
 */
export function isEmbeddedServerView(
  search: string | null | undefined,
): boolean {
  if (!search) return false;
  const query = search.startsWith("?") ? search.slice(1) : search;
  const value = new URLSearchParams(query).get(SERVER_VIEW_EMBED_PARAM);
  return value === "1" || value === "true";
}

export function preferredServerView(
  enableWebTerminal: boolean,
  requested: ServerView | null,
): ServerView | null {
  if (!requested) return null;
  if (requested === "terminal" && !enableWebTerminal) return "providers";
  return requested;
}

export function storedServerView(
  stored: string | null | undefined,
  enableWebTerminal: boolean,
): ServerView | null {
  if (!isServerView(stored)) return null;
  if (stored === "terminal" && !enableWebTerminal) return "providers";
  return stored;
}

export function resolveInitialServerView(
  enableWebTerminal: boolean,
  search: string | null | undefined,
  stored: string | null | undefined,
): ServerView {
  return (
    preferredServerView(enableWebTerminal, requestedServerView(search)) ??
    storedServerView(stored, enableWebTerminal) ??
    "providers"
  );
}

export function clientWebTerminalUrl(baseUrl: string): string {
  const trimmed = baseUrl.trim();
  if (!trimmed) return trimmed;
  try {
    const url = new URL(trimmed);
    url.searchParams.set(SERVER_VIEW_QUERY_PARAM, "terminal");
    url.searchParams.set(SERVER_VIEW_EMBED_PARAM, "1");
    return url.toString();
  } catch {
    const [withoutHash, hash = ""] = trimmed.split("#", 2);
    const separator = withoutHash.includes("?") ? "&" : "?";
    const next = `${withoutHash}${separator}${SERVER_VIEW_QUERY_PARAM}=terminal&${SERVER_VIEW_EMBED_PARAM}=1`;
    return hash ? `${next}#${hash}` : next;
  }
}
