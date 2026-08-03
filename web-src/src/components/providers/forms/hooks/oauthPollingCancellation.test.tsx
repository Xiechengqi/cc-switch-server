import React from "react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act } from "react-dom/test-utils";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { useClaudeOauth } from "./useClaudeOauth";
import { useManagedAuth } from "./useManagedAuth";

const apiMocks = vi.hoisted(() => ({
  authGetStatus: vi.fn(),
  authStartLogin: vi.fn(),
  authPollForAccount: vi.fn(),
  authCancelLogin: vi.fn(),
  authSubmitOauthCallback: vi.fn(),
  authSubmitOauthCode: vi.fn(),
  authLogout: vi.fn(),
  authRemoveAccount: vi.fn(),
  authSetDefaultAccount: vi.fn(),
  authSetWorkspace: vi.fn(),
  importCursorLocalAuth: vi.fn(),
}));

vi.mock("@/lib/api", () => ({
  authApi: apiMocks,
  isRemoteWebMode: () => false,
}));

interface Deferred<T> {
  promise: Promise<T>;
  resolve: (value: T) => void;
}

function deferred<T>(): Deferred<T> {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}

const emptyStatus = {
  provider: "google_gemini_oauth",
  authenticated: false,
  default_account_id: null,
  codex_oauth: null,
  accounts: [],
};

const deviceLogin = {
  device_code: "device-1",
  user_code: "CODE",
  verification_uri: "https://example.com/device",
  expires_in: 600,
  interval: 1,
  flow: "device",
};

let container: HTMLDivElement;
let root: Root;
let queryClient: QueryClient;

beforeEach(() => {
  vi.clearAllMocks();
  apiMocks.authGetStatus.mockResolvedValue(emptyStatus);
  apiMocks.authStartLogin.mockResolvedValue(deviceLogin);
  apiMocks.authPollForAccount.mockResolvedValue(null);
  apiMocks.authCancelLogin.mockResolvedValue(undefined);
  apiMocks.authLogout.mockResolvedValue(undefined);
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
});

afterEach(async () => {
  await act(async () => root.unmount());
  queryClient.clear();
  container.remove();
  vi.useRealTimers();
});

describe("OAuth polling cancellation", () => {
  it("ignores a managed-auth poll that succeeds after cancellation", async () => {
    const pollResult = deferred<unknown>();
    apiMocks.authPollForAccount.mockReturnValue(pollResult.promise);
    let latest!: ReturnType<typeof useManagedAuth>;

    function Harness() {
      latest = useManagedAuth("google_gemini_oauth");
      return null;
    }

    await act(async () => {
      root.render(
        <QueryClientProvider client={queryClient}>
          <Harness />
        </QueryClientProvider>,
      );
      await Promise.resolve();
    });
    await act(async () => {
      latest.addAccount();
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(apiMocks.authPollForAccount).toHaveBeenCalledTimes(1);
    const statusCallsBeforeCancel = apiMocks.authGetStatus.mock.calls.length;

    act(() => latest.cancelAuth());
    expect(apiMocks.authCancelLogin).toHaveBeenCalledWith(
      "google_gemini_oauth",
      "device-1",
    );

    await act(async () => {
      pollResult.resolve({ id: "account-new" });
      await pollResult.promise;
      await Promise.resolve();
    });

    expect(apiMocks.authGetStatus).toHaveBeenCalledTimes(
      statusCallsBeforeCancel,
    );
    expect(latest.pollingState).toBe("idle");
    expect(latest.deviceCode).toBeNull();
    expect(latest.error).toBeNull();
  });

  it("cancels Claude remotely and ignores its in-flight local poll", async () => {
    vi.useFakeTimers();
    const pollResult = deferred<unknown>();
    apiMocks.authPollForAccount.mockReturnValue(pollResult.promise);
    let latest!: ReturnType<typeof useClaudeOauth>;

    function Harness() {
      latest = useClaudeOauth();
      return null;
    }

    await act(async () => {
      root.render(
        <QueryClientProvider client={queryClient}>
          <Harness />
        </QueryClientProvider>,
      );
      await Promise.resolve();
    });
    await act(async () => {
      latest.addAccount("localhost");
      await Promise.resolve();
      await Promise.resolve();
    });
    expect(latest.authState).toBe("waiting_browser");
    await act(async () => {
      vi.advanceTimersByTime(1_000);
      await Promise.resolve();
    });

    expect(apiMocks.authPollForAccount).toHaveBeenCalledTimes(1);
    const statusCallsBeforeCancel = apiMocks.authGetStatus.mock.calls.length;

    act(() => latest.cancelAuth());
    expect(apiMocks.authCancelLogin).toHaveBeenCalledWith(
      "claude_oauth",
      "device-1",
    );

    await act(async () => {
      pollResult.resolve({ id: "claude-account-new" });
      await pollResult.promise;
      await Promise.resolve();
    });

    expect(apiMocks.authGetStatus).toHaveBeenCalledTimes(
      statusCallsBeforeCancel,
    );
    expect(latest.authState).toBe("idle");
    expect(latest.deviceCode).toBeNull();
    expect(latest.error).toBeNull();
  });

  it("does not let a managed-auth poll recreate an account after logout", async () => {
    const pollResult = deferred<unknown>();
    apiMocks.authPollForAccount.mockReturnValue(pollResult.promise);
    apiMocks.authLogout.mockResolvedValue(undefined);
    let latest!: ReturnType<typeof useManagedAuth>;

    function Harness() {
      latest = useManagedAuth("google_gemini_oauth");
      return null;
    }

    await act(async () => {
      root.render(
        <QueryClientProvider client={queryClient}>
          <Harness />
        </QueryClientProvider>,
      );
      await Promise.resolve();
    });
    await act(async () => {
      latest.addAccount();
      await Promise.resolve();
      await Promise.resolve();
    });
    expect(apiMocks.authPollForAccount).toHaveBeenCalledTimes(1);

    await act(async () => {
      await latest.logoutAsync();
    });
    const statusCallsAfterLogout = apiMocks.authGetStatus.mock.calls.length;
    expect(apiMocks.authCancelLogin).toHaveBeenCalledWith(
      "google_gemini_oauth",
      "device-1",
    );

    await act(async () => {
      pollResult.resolve({ id: "account-after-logout" });
      await pollResult.promise;
      await Promise.resolve();
    });

    expect(apiMocks.authGetStatus).toHaveBeenCalledTimes(
      statusCallsAfterLogout,
    );
    expect(latest.pollingState).toBe("idle");
    expect(latest.deviceCode).toBeNull();
  });

  it("does not let a Claude poll recreate an account after logout", async () => {
    vi.useFakeTimers();
    const pollResult = deferred<unknown>();
    apiMocks.authPollForAccount.mockReturnValue(pollResult.promise);
    apiMocks.authLogout.mockResolvedValue(undefined);
    let latest!: ReturnType<typeof useClaudeOauth>;

    function Harness() {
      latest = useClaudeOauth();
      return null;
    }

    await act(async () => {
      root.render(
        <QueryClientProvider client={queryClient}>
          <Harness />
        </QueryClientProvider>,
      );
      await Promise.resolve();
    });
    await act(async () => {
      latest.addAccount("localhost");
      await Promise.resolve();
      await Promise.resolve();
    });
    await act(async () => {
      vi.advanceTimersByTime(1_000);
      await Promise.resolve();
    });
    expect(apiMocks.authPollForAccount).toHaveBeenCalledTimes(1);

    await act(async () => {
      await latest.logoutAsync();
    });
    const statusCallsAfterLogout = apiMocks.authGetStatus.mock.calls.length;
    expect(apiMocks.authCancelLogin).toHaveBeenCalledWith(
      "claude_oauth",
      "device-1",
    );

    await act(async () => {
      pollResult.resolve({ id: "claude-account-after-logout" });
      await pollResult.promise;
      await Promise.resolve();
    });

    expect(apiMocks.authGetStatus).toHaveBeenCalledTimes(
      statusCallsAfterLogout,
    );
    expect(latest.authState).toBe("idle");
    expect(latest.deviceCode).toBeNull();
  });

  it("waits for managed-auth cancellation before logging out", async () => {
    const cancellation = deferred<void>();
    const events: string[] = [];
    apiMocks.authCancelLogin.mockImplementation(() => {
      events.push("cancel-started");
      return cancellation.promise;
    });
    apiMocks.authLogout.mockImplementation(async () => {
      events.push("logout-started");
    });
    let latest!: ReturnType<typeof useManagedAuth>;

    function Harness() {
      latest = useManagedAuth("google_gemini_oauth");
      return null;
    }

    await act(async () => {
      root.render(
        <QueryClientProvider client={queryClient}>
          <Harness />
        </QueryClientProvider>,
      );
      await Promise.resolve();
    });
    await act(async () => {
      latest.addAccount();
      await Promise.resolve();
      await Promise.resolve();
    });
    await vi.waitFor(() => {
      expect(latest.deviceCode?.device_code).toBe("device-1");
    });

    let logoutPromise!: Promise<unknown>;
    act(() => {
      logoutPromise = latest.logoutAsync();
    });
    expect(events).toEqual(["cancel-started"]);

    await act(async () => {
      cancellation.resolve(undefined);
      await logoutPromise;
    });
    expect(events).toEqual(["cancel-started", "logout-started"]);
  });

  it("waits for Claude cancellation before logging out", async () => {
    const cancellation = deferred<void>();
    const events: string[] = [];
    apiMocks.authCancelLogin.mockImplementation(() => {
      events.push("cancel-started");
      return cancellation.promise;
    });
    apiMocks.authLogout.mockImplementation(async () => {
      events.push("logout-started");
    });
    let latest!: ReturnType<typeof useClaudeOauth>;

    function Harness() {
      latest = useClaudeOauth();
      return null;
    }

    await act(async () => {
      root.render(
        <QueryClientProvider client={queryClient}>
          <Harness />
        </QueryClientProvider>,
      );
      await Promise.resolve();
    });
    await act(async () => {
      latest.addAccount("localhost");
      await Promise.resolve();
      await Promise.resolve();
    });
    await vi.waitFor(() => {
      expect(latest.deviceCode?.device_code).toBe("device-1");
    });

    let logoutPromise!: Promise<unknown>;
    act(() => {
      logoutPromise = latest.logoutAsync();
    });
    expect(events).toEqual(["cancel-started"]);

    await act(async () => {
      cancellation.resolve(undefined);
      await logoutPromise;
    });
    expect(events).toEqual(["cancel-started", "logout-started"]);
  });
});
