import { afterEach, beforeEach, expect, it, vi } from "vitest";

const origin = "https://router.example.test";
let parent: { postMessage: ReturnType<typeof vi.fn> };

beforeEach(() => {
  vi.resetModules();
  window.history.replaceState(null, "", `/?embed=1&routerOrigin=${encodeURIComponent(origin)}`);
  parent = { postMessage: vi.fn() };
  vi.stubGlobal("parent", parent);
  vi.stubGlobal("fetch", vi.fn().mockResolvedValue(Response.json({ token: "client-window-only", expiresAt: new Date(Date.now() + 600_000).toISOString() })));
});

afterEach(() => {
  vi.unstubAllGlobals();
  document.querySelectorAll('meta[name^="cc-lease-"]').forEach((node) => node.remove());
  window.history.replaceState(null, "", "/");
});

function standaloneTicket() {
  vi.stubGlobal("parent", window);
  const ticket = "q".repeat(48);
  const nonce = crypto.randomUUID();
  for (const [name, content] of [
    ["ticket", ticket],
    ["nonce", nonce],
  ]) {
    const meta = document.createElement("meta");
    meta.name = "cc-lease-" + name;
    meta.content = content;
    document.head.appendChild(meta);
  }
  return { ticket, nonce };
}

it("exchanges launch metadata once and coalesces parallel standalone rotations", async () => {
  const input = standaloneTicket();
  const auth = await import("./embeddedAuth");
  await Promise.all([auth.initializeEmbeddedAuth(), auth.initializeEmbeddedAuth()]);
  expect(fetch).toHaveBeenCalledTimes(1);
  expect(fetch).toHaveBeenCalledWith(
    "/web-api/auth/router-exchange",
    expect.objectContaining({ body: JSON.stringify(input) }),
  );
  expect(document.querySelector('meta[name^="cc-lease-"]')).toBeNull();
  expect(location.pathname).toBe("/");
  vi.mocked(fetch).mockResolvedValue(
    Response.json({ token: "rotated", expiresAt: new Date(Date.now() + 600_000).toISOString() }),
  );
  expect(await Promise.all([auth.renewEmbeddedSession(), auth.renewEmbeddedSession()])).toEqual([true, true]);
  expect(fetch).toHaveBeenCalledTimes(2);
  expect(fetch).toHaveBeenCalledWith(
    "/web-api/auth/router-renew",
    expect.objectContaining({ headers: { "x-cc-switch-client-web-session": "client-window-only" } }),
  );
  expect(auth.embeddedSessionToken()).toBe("rotated");
});

it("ignores a standalone renewal arriving after explicit logout", async () => {
  standaloneTicket();
  const auth = await import("./embeddedAuth");
  await auth.initializeEmbeddedAuth();
  let finish!: (response: Response) => void;
  vi.mocked(fetch).mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
  );
  const renewal = auth.renewEmbeddedSession();
  auth.clearEmbeddedSession();
  finish(Response.json({ token: "late", expiresAt: new Date(Date.now() + 600_000).toISOString() }));
  expect(await renewal).toBe(false);
  expect(auth.embeddedSessionToken()).toBeNull();
  expect(auth.shouldResumeStandalone()).toBe(false);
});

function ticket(nonce: string, options: { source?: unknown; origin?: string; nonce?: string } = {}) {
  window.dispatchEvent(new MessageEvent("message", { source: (options.source ?? parent) as Window,
    origin: options.origin ?? origin,
    data: { type: "cc-client-login-ticket", nonce: options.nonce ?? nonce, kind: "terminal", ticket: "one-time-ticket" },
  }));
}

it("accepts only the matching parent origin, source and nonce, and keeps credentials out of storage", async () => {
  const { initializeEmbeddedAuth, embeddedSessionToken, embeddedAuthHeaders, clearEmbeddedSession } = await import("./embeddedAuth");
  localStorage.clear();
  const ready = initializeEmbeddedAuth();
  const nonce = parent.postMessage.mock.calls[0][0].nonce;
  ticket(nonce, { origin: "https://attacker.example.test" });
  ticket(nonce, { source: window });
  ticket(nonce, { nonce: crypto.randomUUID() });
  expect(fetch).not.toHaveBeenCalled();
  ticket(nonce);
  await ready;
  expect(fetch).toHaveBeenCalledWith("/web-api/auth/router-exchange", expect.objectContaining({
    body: JSON.stringify({ ticket: "one-time-ticket", nonce }),
  }));
  expect(embeddedSessionToken()).toBe("client-window-only");
  expect(await embeddedAuthHeaders()).toEqual({ "x-cc-switch-client-web-session": "client-window-only" });
  expect(localStorage.length).toBe(0);
  expect(window.location.href).not.toContain("one-time-ticket");
  expect(window.location.search).toContain("view=terminal");
  clearEmbeddedSession();
  expect(embeddedSessionToken()).toBeNull();
});

it("coalesces concurrent initialization and discards late credentials after logout", async () => {
  const { initializeEmbeddedAuth, embeddedSessionToken, clearEmbeddedSession } = await import("./embeddedAuth");
  const first = initializeEmbeddedAuth();
  const second = initializeEmbeddedAuth();
  expect(parent.postMessage).toHaveBeenCalledTimes(1);
  const nonce = parent.postMessage.mock.calls[0][0].nonce;
  clearEmbeddedSession();
  ticket(nonce);
  await Promise.all([first, second]);
  expect(embeddedSessionToken()).toBeNull();
});
