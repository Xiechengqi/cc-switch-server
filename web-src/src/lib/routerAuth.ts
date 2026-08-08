const AUTH_KEY = "cc_switch_router_auth_v2";
const PROTOCOL_EPOCH = "namespace-flat-1";
const AUTH_DEVICE_IDENTITY_LOCK_NAME =
  "cc-switch-server-auth-device-identity-v1";
export const SERVER_AUTH_EXPIRED_EVENT = "cc-switch-server-auth-expired";

let refreshInFlight: { promise: Promise<boolean> | null } = { promise: null };
let authDeviceIdentityInitialization: Promise<AuthDeviceIdentity> | null = null;
let authExpiryNotified = false;

export interface RouterAuthState {
  authProvider?: "router" | "apiToken" | "password" | null;
  authDeviceId?: string | null;
  publicKey?: string | null;
  privateKey?: string | null;
  email?: string | null;
  accessToken?: string | null;
  refreshToken?: string | null;
  apiToken?: string | null;
  expiresAt?: string | null;
  refreshExpiresAt?: string | null;
}

interface AuthDeviceIdentity {
  authDeviceId: string;
  publicKey: string;
  privateKey: string;
}

export interface RouterSessionStatus {
  authenticated: boolean;
  user?: {
    id: string;
    email: string;
  } | null;
  expiresAt?: string | null;
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
    authDeviceId: state.authDeviceId || null,
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
  ownerEmail?: string | null;
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

async function generateAuthDeviceKeys(): Promise<{
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

async function registerAuthDeviceIdentity(
  publicKey: string,
  privateKey: string,
): Promise<string> {
  const kind = "browser";
  const platform = platformLabel();
  const appVersion = "cc-switch-share-web";
  const instanceNonce = randomId();
  const timestampMs = Date.now();
  const canonical = `${PROTOCOL_EPOCH}\nregister_auth_device\n${publicKey}\n${kind}\n${platform}\n${appVersion}\n${instanceNonce}\n${timestampMs}`;
  const privateCryptoKey = await importPrivateKey(privateKey);
  const signature = bytesToBase64(
    new Uint8Array(
      await crypto.subtle.sign(
        { name: "Ed25519" } as AlgorithmIdentifier,
        privateCryptoKey,
        bytesToArrayBuffer(new TextEncoder().encode(canonical)),
      ),
    ),
  );
  const response = await fetch("/v1/auth/devices/register", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({
      protocolEpoch: PROTOCOL_EPOCH,
      publicKey,
      kind,
      platform,
      appVersion,
      instanceNonce,
      timestampMs,
      signature,
    }),
  });
  const data = await parseJsonResponse<{ authDeviceId: string }>(response);
  return data.authDeviceId;
}

function authDeviceIdentityFromState(
  state: RouterAuthState,
): AuthDeviceIdentity | null {
  if (state.authDeviceId && state.publicKey && state.privateKey) {
    return {
      authDeviceId: state.authDeviceId,
      publicKey: state.publicKey,
      privateKey: state.privateKey,
    };
  }
  return null;
}

async function createAuthDeviceIdentityIfMissing(): Promise<AuthDeviceIdentity> {
  const existing = authDeviceIdentityFromState(readAuthState());
  if (existing) return existing;
  const keys = await generateAuthDeviceKeys();
  const authDeviceId = await registerAuthDeviceIdentity(
    keys.publicKey,
    keys.privateKey,
  );
  const identityCreatedElsewhere = authDeviceIdentityFromState(readAuthState());
  if (identityCreatedElsewhere) return identityCreatedElsewhere;
  const next = mergeAuthState({
    authDeviceId,
    publicKey: keys.publicKey,
    privateKey: keys.privateKey,
  });
  return {
    authDeviceId,
    publicKey: next.publicKey!,
    privateKey: next.privateKey!,
  };
}

async function initializeAuthDeviceIdentity(): Promise<AuthDeviceIdentity> {
  const lockManager = navigator.locks;
  if (!lockManager) return createAuthDeviceIdentityIfMissing();
  let lockCallbackStarted = false;
  try {
    return await lockManager.request(AUTH_DEVICE_IDENTITY_LOCK_NAME, async () => {
      lockCallbackStarted = true;
      return createAuthDeviceIdentityIfMissing();
    });
  } catch (error) {
    if (lockCallbackStarted) throw error;
    return createAuthDeviceIdentityIfMissing();
  }
}

async function ensureAuthDeviceIdentity(): Promise<AuthDeviceIdentity> {
  const existing = authDeviceIdentityFromState(readAuthState());
  if (existing) return existing;
  if (authDeviceIdentityInitialization) return authDeviceIdentityInitialization;
  const pending = initializeAuthDeviceIdentity();
  authDeviceIdentityInitialization = pending;
  try {
    return await pending;
  } finally {
    if (authDeviceIdentityInitialization === pending) {
      authDeviceIdentityInitialization = null;
    }
  }
}

function shouldResetAuthDeviceIdentity(message: string): boolean {
  return /auth device|public key|signature/i.test(message || "");
}

async function replaceAuthDeviceIdentity(
  expectedAuthDeviceId: string,
): Promise<AuthDeviceIdentity> {
  const replaceIfCurrent = async () => {
    const current = authDeviceIdentityFromState(readAuthState());
    if (current && current.authDeviceId !== expectedAuthDeviceId) return current;
    mergeAuthState({
      authDeviceId: null,
      publicKey: null,
      privateKey: null,
    });
    return createAuthDeviceIdentityIfMissing();
  };

  const lockManager = navigator.locks;
  if (!lockManager) return replaceIfCurrent();

  let lockCallbackStarted = false;
  try {
    return await lockManager.request(AUTH_DEVICE_IDENTITY_LOCK_NAME, async () => {
      lockCallbackStarted = true;
      return replaceIfCurrent();
    });
  } catch (error) {
    if (lockCallbackStarted) throw error;
    return replaceIfCurrent();
  }
}

async function signAuthPayload(
  action: string,
  payload: Record<string, unknown>,
): Promise<{
  authDeviceId: string;
  timestampMs: number;
  nonce: string;
  signature: string;
}> {
  const identity = await ensureAuthDeviceIdentity();
  return signAuthPayloadWithIdentity(identity, action, payload);
}

async function signAuthPayloadWithIdentity(
  identity: AuthDeviceIdentity,
  action: string,
  payload: Record<string, unknown>,
): Promise<{
  authDeviceId: string;
  timestampMs: number;
  nonce: string;
  signature: string;
}> {
  const timestampMs = Date.now();
  const nonce = randomId();
  const payloadJson = JSON.stringify(payload);
  const body = `${PROTOCOL_EPOCH}\n${identity.authDeviceId}\n${action}\n${payloadJson}\n${timestampMs}\n${nonce}`;
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
    authDeviceId: identity.authDeviceId,
    timestampMs,
    nonce,
    signature,
  };
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

export function readRouterAccessToken(): string | null {
  const state = readAuthState();
  if (state.authProvider === "apiToken") {
    return state.apiToken?.trim() || null;
  }
  return state.accessToken?.trim() || state.apiToken?.trim() || null;
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
        return applyPasswordAuthResponse(response);
      }
      const response = await fetch("/v1/auth/session/refresh", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({
          refreshToken: state.refreshToken,
        }),
      });
      if (await applyRefreshResponse(response)) return true;
      const clientWebResponse = await fetch("/web-api/auth/session/refresh", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({
          refreshToken: state.refreshToken,
        }),
      });
      return applyRefreshResponse(clientWebResponse);
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
  if (response.status === 401) {
    notifyServerAuthExpired();
  } else {
    authExpiryNotified = false;
  }
  return response;
}

function notifyServerAuthExpired(): void {
  if (authExpiryNotified) return;
  authExpiryNotified = true;
  clearRouterSessionTokens();
  window.dispatchEvent(new CustomEvent(SERVER_AUTH_EXPIRED_EVENT));
}

export async function getRouterSessionStatus(): Promise<RouterSessionStatus> {
  const response = await routerAuthFetch("/v1/auth/session/me", {
    cache: "no-store",
  });
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
  const identity = await ensureAuthDeviceIdentity();
  return requestRouterEmailCodeWithIdentity(normalizedEmail, identity);
}

async function requestRouterEmailCodeWithIdentity(
  normalizedEmail: string,
  identity: AuthDeviceIdentity,
): Promise<{ maskedDestination: string; cooldownSecs?: number }> {
  const signed = await signAuthPayloadWithIdentity(identity, "auth_request_code", {
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
  if (options?.clientWeb) return requestRouterEmailCode(email, options);
  const normalizedEmail = email.trim().toLowerCase();
  let identity = await ensureAuthDeviceIdentity();
  try {
    return await requestRouterEmailCodeWithIdentity(normalizedEmail, identity);
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    if (!shouldResetAuthDeviceIdentity(message)) throw error;
    identity = await replaceAuthDeviceIdentity(identity.authDeviceId);
    return requestRouterEmailCodeWithIdentity(normalizedEmail, identity);
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
    : await ensureAuthDeviceIdentity();
  const response = await fetch(endpoint, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({
      email: email.trim().toLowerCase(),
      code: code.trim(),
      ...(identity ? { authDeviceId: identity.authDeviceId } : {}),
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
