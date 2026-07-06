const AUTH_KEY = "cc_switch_router_auth_v1";
const SERVER_PASSWORD_KEY = "cc_switch_server_password";

let refreshInFlight: { promise: Promise<boolean> | null } = { promise: null };
let reloginInFlight: { promise: Promise<boolean> | null } = { promise: null };

export interface RouterAuthState {
  authProvider?: "router" | "apiToken" | "password" | null;
  installationId?: string | null;
  publicKey?: string | null;
  privateKey?: string | null;
  email?: string | null;
  accessToken?: string | null;
  refreshToken?: string | null;
  apiToken?: string | null;
  expiresAt?: string | null;
  refreshExpiresAt?: string | null;
}

export interface RouterSessionStatus {
  authenticated: boolean;
  user?: {
    id: string;
    email: string;
  } | null;
  expiresAt?: string | null;
  installationOwnerEmail?: string | null;
  isAdmin?: boolean;
}

function readAuthState(): RouterAuthState {
  try {
    return JSON.parse(localStorage.getItem(AUTH_KEY) || "{}") || {};
  } catch {
    return {};
  }
}

function writeAuthState(state: RouterAuthState): void {
  localStorage.setItem(AUTH_KEY, JSON.stringify(state));
}

function mergeAuthState(patch: RouterAuthState): RouterAuthState {
  const next = { ...readAuthState(), ...patch };
  writeAuthState(next);
  window.dispatchEvent(
    new CustomEvent("router-auth-changed", { detail: next }),
  );
  return next;
}

export function clearRouterSessionTokens(): void {
  const state = readAuthState();
  mergeAuthState({
    installationId: state.installationId || null,
    publicKey: state.publicKey || null,
    privateKey: state.privateKey || null,
    email: null,
    accessToken: null,
    refreshToken: null,
    apiToken: null,
    authProvider: null,
    expiresAt: null,
    refreshExpiresAt: null,
  });
}

export function setRouterApiToken(apiToken: string): void {
  mergeAuthState({
    email: null,
    accessToken: null,
    refreshToken: null,
    apiToken: apiToken.trim(),
    authProvider: "apiToken",
    expiresAt: null,
    refreshExpiresAt: null,
  });
}

export interface WebAuthMethods {
  routerAvailable: boolean;
  passwordConfigured: boolean;
  setupTokenRequired: boolean;
  initialClientSetupRequired: boolean;
  methods: Array<"email" | "apiToken" | "password" | "passwordSetup">;
}

export interface InitialWebSetupInput {
  password: string;
  ownerEmail: string;
  routerDomain: string;
  clientSubdomain?: string;
}

export interface InitialWebSetupSummary {
  ownerEmail: string;
  routerDomain: string;
  clientSubdomain: string;
  clientUrl: string;
  clientTunnelStarted: boolean;
}

interface PasswordAuthResponse {
  accessToken: string;
  refreshToken: string;
  expiresAt: string;
  refreshExpiresAt: string;
}

export interface InitialWebSetupResponse extends PasswordAuthResponse {
  setup: InitialWebSetupSummary;
}

export async function getWebAuthMethods(): Promise<WebAuthMethods> {
  const response = await fetch("/web-api/auth/methods", {
    headers: { accept: "application/json" },
    cache: "no-store",
  });
  return parseJsonResponse<WebAuthMethods>(response);
}

function bytesToBase64(bytes: Uint8Array): string {
  let binary = "";
  bytes.forEach((byte) => {
    binary += String.fromCharCode(byte);
  });
  return btoa(binary);
}

function base64ToBytes(value: string): Uint8Array {
  return Uint8Array.from(atob(value), (ch) => ch.charCodeAt(0));
}

function bytesToArrayBuffer(bytes: Uint8Array): ArrayBuffer {
  return bytes.buffer.slice(
    bytes.byteOffset,
    bytes.byteOffset + bytes.byteLength,
  ) as ArrayBuffer;
}

function platformLabel(): string {
  const ua = navigator.userAgent || "";
  if (/Mac/i.test(ua)) return "web-macos";
  if (/Windows/i.test(ua)) return "web-windows";
  if (/Linux/i.test(ua)) return "web-linux";
  return "web";
}

function randomId(): string {
  return crypto.randomUUID
    ? crypto.randomUUID()
    : `${Date.now()}-${Math.random()}`;
}

async function parseJsonResponse<T>(response: Response): Promise<T> {
  const data = await response.json().catch(() => ({}));
  if (!response.ok) {
    throw new Error(data?.message || data?.error || `HTTP ${response.status}`);
  }
  return data as T;
}

async function generateInstallationKeys(): Promise<{
  publicKey: string;
  privateKey: string;
}> {
  const keyPair = (await crypto.subtle.generateKey(
    { name: "Ed25519" } as AlgorithmIdentifier,
    true,
    ["sign", "verify"],
  )) as CryptoKeyPair;
  const publicKey = bytesToBase64(
    new Uint8Array(await crypto.subtle.exportKey("raw", keyPair.publicKey)),
  );
  const privateKey = bytesToBase64(
    new Uint8Array(await crypto.subtle.exportKey("pkcs8", keyPair.privateKey)),
  );
  return { publicKey, privateKey };
}

async function importPrivateKey(privateKeyBase64: string): Promise<CryptoKey> {
  return crypto.subtle.importKey(
    "pkcs8",
    bytesToArrayBuffer(base64ToBytes(privateKeyBase64)),
    { name: "Ed25519" } as AlgorithmIdentifier,
    false,
    ["sign"],
  );
}

async function registerInstallationIdentity(
  publicKey: string,
): Promise<string> {
  const response = await fetch("/v1/installations/register", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({
      publicKey,
      platform: platformLabel(),
      appVersion: "cc-switch-share-web",
      instanceNonce: randomId(),
    }),
  });
  const data = await parseJsonResponse<{ installationId: string }>(response);
  return data.installationId;
}

async function ensureInstallationIdentity(): Promise<{
  installationId: string;
  publicKey: string;
  privateKey: string;
}> {
  const state = readAuthState();
  if (state.installationId && state.publicKey && state.privateKey) {
    return {
      installationId: state.installationId,
      publicKey: state.publicKey,
      privateKey: state.privateKey,
    };
  }
  const keys = await generateInstallationKeys();
  const installationId = await registerInstallationIdentity(keys.publicKey);
  const next = mergeAuthState({
    installationId,
    publicKey: keys.publicKey,
    privateKey: keys.privateKey,
  });
  return {
    installationId,
    publicKey: next.publicKey!,
    privateKey: next.privateKey!,
  };
}

function shouldResetInstallationIdentity(message: string): boolean {
  return /installation|public key|signature/i.test(message || "");
}

function resetInstallationIdentity(): void {
  const state = readAuthState();
  mergeAuthState({
    ...state,
    installationId: null,
    publicKey: null,
    privateKey: null,
  });
}

async function signAuthPayload(
  action: string,
  payload: Record<string, unknown>,
): Promise<{
  installationId: string;
  timestampMs: number;
  nonce: string;
  signature: string;
}> {
  const identity = await ensureInstallationIdentity();
  const timestampMs = Date.now();
  const nonce = randomId();
  const payloadJson = JSON.stringify(payload);
  const body = `${identity.installationId}\n${action}\n${payloadJson}\n${timestampMs}\n${nonce}`;
  const privateKey = await importPrivateKey(identity.privateKey);
  const encodedBody = new TextEncoder().encode(body);
  const signature = bytesToBase64(
    new Uint8Array(
      await crypto.subtle.sign(
        { name: "Ed25519" } as AlgorithmIdentifier,
        privateKey,
        bytesToArrayBuffer(encodedBody),
      ),
    ),
  );
  return {
    installationId: identity.installationId,
    timestampMs,
    nonce,
    signature,
  };
}

function readCachedServerPassword(): string | null {
  return localStorage.getItem(SERVER_PASSWORD_KEY);
}

async function reloginWithCachedWebPassword(): Promise<boolean> {
  if (reloginInFlight.promise) {
    return reloginInFlight.promise;
  }
  reloginInFlight.promise = (async () => {
    const password = readCachedServerPassword();
    if (!password) return false;
    try {
      await loginWithWebPassword(password);
      return true;
    } catch {
      return false;
    } finally {
      reloginInFlight.promise = null;
    }
  })();
  return reloginInFlight.promise;
}

function authBearerHeaders(): Record<string, string> {
  const state = readAuthState();
  if (state.authProvider === "apiToken") {
    const token = state.apiToken?.trim();
    return token ? { authorization: `Bearer ${token}` } : {};
  }
  const token = state.accessToken?.trim() || state.apiToken?.trim();
  return token ? { authorization: `Bearer ${token}` } : {};
}

function fetchWithAuth(
  input: RequestInfo | URL,
  init: RequestInit = {},
): Promise<Response> {
  const headers = new Headers(init.headers || {});
  Object.entries(authBearerHeaders()).forEach(([key, value]) =>
    headers.set(key, value),
  );
  return fetch(input, { ...init, headers });
}

async function refreshAccessToken(): Promise<boolean> {
  if (refreshInFlight.promise) {
    return refreshInFlight.promise;
  }
  refreshInFlight.promise = (async () => {
    try {
      const state = readAuthState();
      if (!state.refreshToken) return false;
      if (state.authProvider === "password") {
        const response = await fetch("/web-api/auth/password/refresh", {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({
            refreshToken: state.refreshToken,
          }),
        });
        if (await applyPasswordAuthResponse(response)) return true;
        return reloginWithCachedWebPassword();
      }
      if (state.installationId) {
        const response = await fetch("/v1/auth/session/refresh", {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({
            refreshToken: state.refreshToken,
            installationId: state.installationId,
          }),
        });
        if (await applyRefreshResponse(response)) return true;
      }
      const response = await fetch("/web-api/auth/session/refresh", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({
          refreshToken: state.refreshToken,
        }),
      });
      if (await applyRefreshResponse(response)) return true;
      return reloginWithCachedWebPassword();
    } finally {
      refreshInFlight.promise = null;
    }
  })();
  return refreshInFlight.promise;
}

async function applyPasswordAuthResponse(response: Response): Promise<boolean> {
  const data = await response.json().catch(() => ({}));
  if (!response.ok) return false;
  if (!data.accessToken || !data.refreshToken) return false;
  mergeAuthState({
    authProvider: "password",
    email: "local-admin@cc-switch.local",
    accessToken: data.accessToken,
    refreshToken: data.refreshToken,
    apiToken: null,
    expiresAt: data.expiresAt,
    refreshExpiresAt: data.refreshExpiresAt,
  });
  return true;
}

async function applyPasswordAuthResponseOrThrow<T extends PasswordAuthResponse = PasswordAuthResponse>(
  response: Response,
): Promise<T> {
  const data = await response.json().catch(() => ({}));
  if (!response.ok) {
    throw new Error(data?.message || data?.error || `HTTP ${response.status}`);
  }
  if (!data.accessToken || !data.refreshToken) {
    throw new Error("password login response is missing tokens");
  }
  mergeAuthState({
    authProvider: "password",
    email: "local-admin@cc-switch.local",
    accessToken: data.accessToken,
    refreshToken: data.refreshToken,
    apiToken: null,
    expiresAt: data.expiresAt,
    refreshExpiresAt: data.refreshExpiresAt,
  });
  return data as T;
}

export async function loginWithWebPassword(password: string): Promise<void> {
  await applyPasswordAuthResponseOrThrow(
    await fetch("/web-api/auth/password/login", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ password }),
    }),
  );
}

export async function setupWebPassword(password: string): Promise<void> {
  await applyPasswordAuthResponseOrThrow(
    await fetch("/web-api/auth/password/setup", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ password }),
    }),
  );
}

export async function setupInitialClientWeb(
  input: InitialWebSetupInput,
): Promise<InitialWebSetupSummary> {
  const data = await applyPasswordAuthResponseOrThrow<InitialWebSetupResponse>(
    await fetch("/web-api/auth/initial-setup", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(input),
    }),
  );
  return data.setup;
}

async function applyRefreshResponse(response: Response): Promise<boolean> {
  const data = await response.json().catch(() => ({}));
  if (!response.ok) return false;
  if (!data.accessToken || !data.refreshToken) return false;
  mergeAuthState({
    accessToken: data.accessToken,
    refreshToken: data.refreshToken,
    expiresAt: data.expiresAt,
    refreshExpiresAt: data.refreshExpiresAt,
  });
  return true;
}

export async function routerAuthFetch(
  input: RequestInfo | URL,
  init: RequestInit = {},
): Promise<Response> {
  let response = await fetchWithAuth(input, init);
  if (response.status === 401 && (await refreshAccessToken())) {
    response = await fetchWithAuth(input, init);
  }
  if (response.status === 401 && (await reloginWithCachedWebPassword())) {
    response = await fetchWithAuth(input, init);
  }
  return response;
}

export async function getRouterSessionStatus(): Promise<RouterSessionStatus> {
  const state = readAuthState();
  const params = new URLSearchParams();
  if (state.installationId) params.set("installationId", state.installationId);
  const response = await routerAuthFetch(
    `/v1/auth/session/me${params.toString() ? `?${params}` : ""}`,
    { cache: "no-store" },
  );
  if (!response.ok) return { authenticated: false };
  return response.json() as Promise<RouterSessionStatus>;
}

export async function requestRouterEmailCode(
  email: string,
  options?: { clientWeb?: boolean },
): Promise<{ maskedDestination: string; cooldownSecs?: number }> {
  const normalizedEmail = email.trim().toLowerCase();
  if (options?.clientWeb) {
    const response = await fetch("/web-api/auth/email/request-code", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ email: normalizedEmail }),
    });
    return parseJsonResponse(response);
  }
  const signed = await signAuthPayload("auth_request_code", {
    email: normalizedEmail,
    purpose: "login",
  });
  const response = await fetch("/v1/auth/email/request-code", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ email: normalizedEmail, ...signed }),
  });
  return parseJsonResponse(response);
}

export async function requestRouterEmailCodeWithIdentityRetry(
  email: string,
  options?: { clientWeb?: boolean },
): Promise<{ maskedDestination: string; cooldownSecs?: number }> {
  try {
    return await requestRouterEmailCode(email, options);
  } catch (error) {
    if (options?.clientWeb) throw error;
    const message = error instanceof Error ? error.message : String(error);
    if (!shouldResetInstallationIdentity(message)) throw error;
    resetInstallationIdentity();
    return requestRouterEmailCode(email);
  }
}

export async function verifyRouterEmailCode(
  email: string,
  code: string,
  options?: { clientWeb?: boolean },
): Promise<RouterSessionStatus> {
  const endpoint = options?.clientWeb
    ? "/web-api/auth/email/verify-code"
    : "/v1/auth/email/verify-code";
  const identity = options?.clientWeb
    ? null
    : await ensureInstallationIdentity();
  const response = await fetch(endpoint, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({
      email: email.trim().toLowerCase(),
      code: code.trim(),
      ...(identity ? { installationId: identity.installationId } : {}),
    }),
  });
  const data = await parseJsonResponse<{
    user?: { id: string; email: string };
    accessToken: string;
    refreshToken: string;
    expiresAt: string;
    refreshExpiresAt: string;
  }>(response);
  mergeAuthState({
    email: data.user?.email || email.trim().toLowerCase(),
    accessToken: data.accessToken,
    refreshToken: data.refreshToken,
    apiToken: null,
    authProvider: "router",
    expiresAt: data.expiresAt,
    refreshExpiresAt: data.refreshExpiresAt,
  });
  if (options?.clientWeb) {
    return {
      authenticated: true,
      user: data.user || {
        id: email.trim().toLowerCase(),
        email: email.trim().toLowerCase(),
      },
      expiresAt: data.expiresAt,
    };
  }
  return getRouterSessionStatus();
}
