import { beforeEach, describe, expect, it, vi } from "vitest";

const runtimeMocks = vi.hoisted(() => ({
  invokeCommand: vi.fn(),
}));

vi.mock("@/lib/runtime", () => ({
  invokeCommand: runtimeMocks.invokeCommand,
  isTauriRuntime: () => false,
}));

import { deepseekAccountAdd, isOpenAiCliOAuthOriginAllowed } from "./auth";

beforeEach(() => {
  runtimeMocks.invokeCommand.mockReset();
});

describe("isOpenAiCliOAuthOriginAllowed", () => {
  it.each([
    "http://localhost:15721",
    "http://admin.localhost:15721",
    "http://127.0.0.1:15721",
    "http://127.42.0.9:15721",
    "http://[::1]:15721",
  ])("allows loopback origin %s", (origin) => {
    expect(isOpenAiCliOAuthOriginAllowed(origin)).toBe(true);
  });

  it.each([
    "http://192.168.1.20:15721",
    "http://server.example.com",
    "http://0.0.0.0:15721",
    "ftp://client.example.com",
    "not-a-url",
  ])("rejects untrusted origin %s", (origin) => {
    expect(isOpenAiCliOAuthOriginAllowed(origin)).toBe(false);
  });

  it("allows only the exact configured HTTPS Client URL origin", () => {
    const configured = "https://client.example.com/admin";
    expect(
      isOpenAiCliOAuthOriginAllowed("https://client.example.com", configured),
    ).toBe(true);
    expect(
      isOpenAiCliOAuthOriginAllowed(
        "https://client.example.com:443",
        configured,
      ),
    ).toBe(true);
    expect(
      isOpenAiCliOAuthOriginAllowed("https://other.example.com", configured),
    ).toBe(false);
    expect(
      isOpenAiCliOAuthOriginAllowed(
        "https://client.example.com:8443",
        configured,
      ),
    ).toBe(false);
    expect(
      isOpenAiCliOAuthOriginAllowed(
        "https://client.example.com",
        "http://client.example.com",
      ),
    ).toBe(false);
  });

  it("allows the embedded runtime without a secure browser origin", () => {
    expect(
      isOpenAiCliOAuthOriginAllowed(
        "http://server.example.com",
        undefined,
        true,
      ),
    ).toBe(true);
  });

  it("imports DeepSeek accounts with an access token and no password", async () => {
    runtimeMocks.invokeCommand.mockResolvedValue({ id: "deepseek-1" });

    await deepseekAccountAdd({
      identifier: "owner@example.com",
      accessToken: "deepseek-token",
    });

    expect(runtimeMocks.invokeCommand).toHaveBeenCalledWith(
      "deepseek_account_add",
      {
        identifier: "owner@example.com",
        accessToken: "deepseek-token",
      },
    );
    expect(runtimeMocks.invokeCommand.mock.calls[0]?.[1]).not.toHaveProperty(
      "password",
    );
  });
});
