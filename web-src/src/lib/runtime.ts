import { isRemoteWebMode } from "@/lib/api/auth";
import {
  clearRouterSessionTokens,
  loginWithWebPassword,
  readRouterAccessToken,
  routerAuthFetch,
} from "@/lib/routerAuth";

function resolveRequestPath(input: RequestInfo | URL): string {
  if (typeof input === "string") {
    return input.startsWith("http") ? new URL(input).pathname : input;
  }
  if (input instanceof URL) {
    return input.pathname;
  }
  if (typeof input === "object" && input !== null && "url" in input) {
    const url = (input as Request).url;
    if (typeof url === "string") {
      try {
        return new URL(url, window.location.origin).pathname;
      } catch {
        return url;
      }
    }
  }
  return "";
}

function assertClientTunnelCompatiblePath(input: RequestInfo | URL): void {
  if (!isRemoteWebMode()) {
    return;
  }
  const path = resolveRequestPath(input);
  if (path.startsWith("/api/")) {
    throw new Error(
      `Legacy admin API ${path} is unavailable on client tunnel URLs. Use /web-api/invoke or /web-api/* instead.`,
    );
  }
}

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
const PASSWORD_KEY = "cc_switch_server_password";

export function readToken(): string | null {
  return localStorage.getItem(TOKEN_KEY);
}

/** Session token for EventSource/SSE: local admin token or remote web JWT. */
export function readWebSessionToken(): string | null {
  return readToken() || readRouterAccessToken();
}

export function writeToken(token: string | null): void {
  if (token) {
    localStorage.setItem(TOKEN_KEY, token);
  } else {
    localStorage.removeItem(TOKEN_KEY);
  }
}

export function readCachedPassword(): string | null {
  return localStorage.getItem(PASSWORD_KEY);
}

export function writeCachedPassword(password: string | null): void {
  if (password) {
    localStorage.setItem(PASSWORD_KEY, password);
  } else {
    localStorage.removeItem(PASSWORD_KEY);
  }
}

export async function loginWithPassword(password: string): Promise<string> {
  const response = await apiFetch("/api/auth/login", {
    method: "POST",
    headers: {
      accept: "application/json",
      "content-type": "application/json",
    },
    body: JSON.stringify({ method: "password", password }),
  });
  const data = await response.json().catch(() => ({}));
  if (!response.ok) {
    throw new Error(data?.error || data?.message || `HTTP ${response.status}`);
  }
  const token = String(data?.token ?? "");
  if (!token) {
    throw new Error("login response is missing token");
  }
  clearRouterSessionTokens();
  writeToken(token);
  writeCachedPassword(password);
  return token;
}

async function localApiFetch(
  input: RequestInfo | URL,
  init: RequestInit = {},
): Promise<Response> {
  const request = () => {
    const headers = new Headers(init.headers || {});
    const token = readToken();
    if (token) headers.set("authorization", `Bearer ${token}`);
    return fetch(input, { ...init, headers });
  };

  let response = await request();
  if (response.status !== 401) {
    return response;
  }

  const cachedPassword = readCachedPassword();
  if (!cachedPassword) {
    writeToken(null);
    return response;
  }

  try {
    await loginWithPassword(cachedPassword);
  } catch {
    writeToken(null);
    writeCachedPassword(null);
    return response;
  }

  return request();
}

export async function apiFetch(
  input: RequestInfo | URL,
  init: RequestInit = {},
): Promise<Response> {
  assertClientTunnelCompatiblePath(input);
  if (isRemoteWebMode()) {
    return routerAuthFetch(input, init);
  }
  return localApiFetch(input, init);
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

export async function getWebRuntimeContext(
  allowAutoLogin = true,
): Promise<WebRuntimeContext> {
  if (isRemoteWebMode()) {
    return getRemoteWebRuntimeContext(allowAutoLogin);
  }
  const response = await apiFetch("/web-api/context", {
    headers: { accept: "application/json" },
  });
  if (response.status === 401) {
    writeToken(null);
    return tryAutoLoginFromCache(allowAutoLogin);
  }
  if (response.status === 403) {
    return tryAutoLoginFromCache(allowAutoLogin);
  }
  const data = (await response.json().catch(() => ({}))) as WebRuntimeContext;
  if (!response.ok) {
    throw new Error(
      (data as { error?: string; message?: string })?.error ||
        (data as { error?: string; message?: string })?.message ||
        `HTTP ${response.status}`,
    );
  }
  if (data.mode !== "local-admin") {
    writeToken(null);
    return tryAutoLoginFromCache(allowAutoLogin, data);
  }
  return data;
}

async function getRemoteWebRuntimeContext(
  allowAutoLogin: boolean,
): Promise<WebRuntimeContext> {
  const response = await routerAuthFetch("/web-api/context", {
    headers: { accept: "application/json" },
    cache: "no-store",
  });
  if (response.status === 401) {
    const retried = await tryRemoteAutoLogin(allowAutoLogin);
    if (retried.mode !== "local-admin") {
      clearRouterSessionTokens();
    }
    return retried;
  }
  const data = (await response.json().catch(() => ({}))) as WebRuntimeContext;
  if (!response.ok) {
    throw new Error(
      (data as { error?: string; message?: string })?.error ||
        (data as { error?: string; message?: string })?.message ||
        `HTTP ${response.status}`,
    );
  }
  if (data.mode !== "local-admin") {
    return tryRemoteAutoLogin(allowAutoLogin, data);
  }
  return data;
}

async function tryRemoteAutoLogin(
  allowAutoLogin: boolean,
  fallback: WebRuntimeContext = {
    mode: "client-login",
    status: "auth-required",
  },
): Promise<WebRuntimeContext> {
  if (!allowAutoLogin || fallback.auth?.setupRequired) {
    return fallback;
  }
  const cachedPassword = readCachedPassword();
  if (!cachedPassword) {
    return fallback;
  }
  try {
    await loginWithWebPassword(cachedPassword);
    return getRemoteWebRuntimeContext(false);
  } catch {
    writeCachedPassword(null);
    return fallback;
  }
}

async function tryAutoLoginFromCache(
  allowAutoLogin: boolean,
  fallback: WebRuntimeContext = {
    mode: "client-login",
    status: "auth-required",
  },
): Promise<WebRuntimeContext> {
  if (!allowAutoLogin || fallback.auth?.setupRequired) {
    return fallback;
  }
  const cachedPassword = readCachedPassword();
  if (!cachedPassword) {
    return fallback;
  }
  try {
    await loginWithPassword(cachedPassword);
    return getWebRuntimeContext(false);
  } catch {
    writeCachedPassword(null);
    return fallback;
  }
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

/** Server web UI always runs outside Tauri; desktop components gate on this flag. */
export function isTauriRuntime(): boolean {
  return false;
}

/** True when running in cc-switch-server embedded/admin web UI (not desktop Tauri). */
export function isServerWebRuntime(): boolean {
  return !isTauriRuntime();
}
