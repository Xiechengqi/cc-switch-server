export const SERVER_VIEW_STORAGE_KEY = "cc-switch-server-view";
export const SERVER_VIEW_QUERY_PARAM = "view";

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
    return url.toString();
  } catch {
    const [withoutHash, hash = ""] = trimmed.split("#", 2);
    const separator = withoutHash.includes("?") ? "&" : "?";
    const next = `${withoutHash}${separator}${SERVER_VIEW_QUERY_PARAM}=terminal`;
    return hash ? `${next}#${hash}` : next;
  }
}
