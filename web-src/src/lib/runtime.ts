export interface WebRuntimeFeature {
  id: string;
  label: string;
  reason?: string;
}

export interface WebRuntimeCommand {
  name: string;
  support: "native" | "shim" | "excluded";
  implemented: boolean;
  feature: string;
  endpoint?: string;
  notes?: string;
}

export interface WebRuntimeContext {
  mode: "local-admin" | "share" | "client-login";
  appMode?: string;
  platform?: string;
  status?: string;
  permissions?: string[];
  apps?: string[];
  auth?: {
    authenticated?: boolean;
    setupRequired?: boolean;
    ownerEmail?: string | null;
    methods?: string[];
  };
  router?: {
    url?: string | null;
    domain?: string | null;
    clientSubdomain?: string | null;
    clientTunnelStatus?: string | null;
  };
  runtime?: {
    configDir?: string;
    webDistDir?: string | null;
    embeddedWebAssets?: number;
  };
  features?: {
    retained?: WebRuntimeFeature[];
    hidden?: WebRuntimeFeature[];
    excluded?: WebRuntimeFeature[];
  };
  commands?: WebRuntimeCommand[];
  uiAutomation?: {
    allowed?: boolean;
  };
}

const TOKEN_KEY = "cc_switch_server_token";

export function readToken(): string | null {
  return localStorage.getItem(TOKEN_KEY);
}

export function writeToken(token: string | null): void {
  if (token) {
    localStorage.setItem(TOKEN_KEY, token);
  } else {
    localStorage.removeItem(TOKEN_KEY);
  }
}

export async function apiFetch(
  input: RequestInfo | URL,
  init: RequestInit = {},
): Promise<Response> {
  const headers = new Headers(init.headers || {});
  const token = readToken();
  if (token) headers.set("authorization", `Bearer ${token}`);
  return fetch(input, { ...init, headers });
}

export async function jsonFetch<T>(
  input: RequestInfo | URL,
  init: RequestInit = {},
): Promise<T> {
  const response = await apiFetch(input, {
    ...init,
    headers: {
      accept: "application/json",
      ...(init.headers || {}),
    },
  });
  const data = await response.json().catch(() => ({}));
  if (!response.ok) {
    throw new Error(data?.error || data?.message || `HTTP ${response.status}`);
  }
  return data as T;
}

export async function getWebRuntimeContext(): Promise<WebRuntimeContext> {
  const response = await apiFetch("/web-api/context", {
    headers: { accept: "application/json" },
  });
  if (response.status === 401 || response.status === 403) {
    return { mode: "client-login", status: "auth-required" };
  }
  const data = await response.json().catch(() => ({}));
  if (!response.ok) {
    throw new Error(data?.error || data?.message || `HTTP ${response.status}`);
  }
  return data as WebRuntimeContext;
}

export async function invokeCommand<T>(
  command: string,
  args?: Record<string, unknown>,
): Promise<T> {
  return jsonFetch<T>(`/web-api/invoke/${encodeURIComponent(command)}`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(args ?? {}),
  });
}
