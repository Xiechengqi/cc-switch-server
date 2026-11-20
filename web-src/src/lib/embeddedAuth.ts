/** Credentials are scoped to this Client window and never written to browser storage. */
const HEADER = "x-cc-switch-client-web-session";
let credential: { token: string; expiresAt: number } | null = null;
let pending: Promise<void> | null = null;
let disabled = false;
let listenerInstalled = false;

function parentOrigin(): string | null {
  if (window.parent === window) return null;
  try {
    const value = new URLSearchParams(window.location.search).get("routerOrigin");
    if (!value) return null;
    const url = new URL(value);
    if (url.origin !== value || !["https:", "http:"].includes(url.protocol)) return null;
    return url.origin;
  } catch { return null; }
}

export function embeddedSessionToken(): string | null {
  return credential && credential.expiresAt > Date.now() ? credential.token : null;
}

export function clearEmbeddedSession(): void {
  const token = credential?.token;
  credential = null;
  disabled = true;
  if (token) void fetch("/web-api/auth/router-logout", {
    method: "POST", headers: { [HEADER]: token }, keepalive: true,
  }).catch(() => undefined);
}

export async function initializeEmbeddedAuth(): Promise<void> {
  const origin = parentOrigin();
  if (!origin || typeof globalThis.crypto?.randomUUID !== "function" || disabled || (credential && credential.expiresAt > Date.now() + 30_000)) return;
  if (pending) return pending;
  if (!listenerInstalled) {
    listenerInstalled = true;
    window.addEventListener("message", (event) => {
      if (event.source !== window.parent || event.origin !== origin || event.data?.type !== "cc-client-login-reset") return;
      clearEmbeddedSession();
      window.dispatchEvent(new CustomEvent("cc-switch-server-auth-expired"));
    });
  }
  pending = new Promise<void>((resolve) => {
    const nonce = crypto.randomUUID();
    let finished = false;
    const finish = () => {
      if (finished) return;
      finished = true;
      clearTimeout(timeout);
      window.removeEventListener("message", onMessage);
      resolve();
    };
    const onMessage = async (event: MessageEvent) => {
      if (event.source !== window.parent || event.origin !== origin
        || event.data?.type !== "cc-client-login-ticket" || event.data.nonce !== nonce) return;
      if (event.data.kind === "terminal") {
        const url = new URL(window.location.href);
        url.searchParams.set("view", "terminal");
        window.history.replaceState(null, "", url);
      }
      if (typeof event.data.ticket !== "string") { finish(); return; }
      try {
        const response = await fetch("/web-api/auth/router-exchange", {
          method: "POST", headers: { "content-type": "application/json" }, cache: "no-store",
          body: JSON.stringify({ ticket: event.data.ticket, nonce }),
        });
        if (!response.ok) { finish(); return; }
        const data = await response.json();
        if (!finished && !disabled && typeof data.token === "string" && Number.isFinite(Date.parse(data.expiresAt))) {
          credential = { token: data.token, expiresAt: Date.parse(data.expiresAt) };
        }
      } catch { /* A standalone Client login remains available. */ }
      finish();
    };
    const timeout = window.setTimeout(finish, 8_000);
    window.addEventListener("message", onMessage);
    window.parent.postMessage({ type: "cc-client-login-request", nonce }, origin);
  }).finally(() => { pending = null; });
  return pending;
}

export async function embeddedAuthHeaders(): Promise<Record<string, string>> {
  await initializeEmbeddedAuth();
  const token = embeddedSessionToken();
  return token ? { [HEADER]: token } : {};
}

export async function renewEmbeddedSession(): Promise<boolean> {
  if (disabled || !credential) return false;
  credential = null;
  await initializeEmbeddedAuth();
  return embeddedSessionToken() !== null;
}
