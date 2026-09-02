import { beforeEach, describe, expect, it, vi } from "vitest";

const runtimeMocks = vi.hoisted(() => ({
  invokeCommand: vi.fn(),
}));

vi.mock("@/lib/runtime", () => ({
  invokeCommand: runtimeMocks.invokeCommand,
}));

import {
  authPollForAccount,
  authStartLogin,
  authSubmitOauthCallback,
  deepseekAccountAdd,
  importQoderPat,
  isOpenAiCliOAuthOriginAllowed,
} from "./auth";

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

  it("keeps Qoder site and device-flow state in the managed auth contract", async () => {
    runtimeMocks.invokeCommand.mockResolvedValue(null);

    await authStartLogin("qoder_cosy", undefined, "device", undefined, "cn");
    await authPollForAccount(
      "qoder_cosy",
      "device-code",
      undefined,
      "flow-state",
    );

    expect(runtimeMocks.invokeCommand).toHaveBeenNthCalledWith(
      1,
      "auth_start_login",
      {
        authProvider: "qoder_cosy",
        githubDomain: null,
        oauthFlowMode: "device",
        kiroLoginProvider: null,
        qoderSite: "cn",
      },
    );
    expect(runtimeMocks.invokeCommand).toHaveBeenNthCalledWith(
      2,
      "auth_poll_for_account",
      {
        authProvider: "qoder_cosy",
        deviceCode: "device-code",
        githubDomain: null,
        flowState: "flow-state",
      },
    );
  });

  it("keeps CodeBuddy site selection in the managed auth contract", async () => {
    runtimeMocks.invokeCommand.mockResolvedValue(null);

    await authStartLogin(
      "codebuddy_oauth",
      undefined,
      undefined,
      undefined,
      undefined,
      "cn",
    );

    expect(runtimeMocks.invokeCommand).toHaveBeenCalledWith(
      "auth_start_login",
      {
        authProvider: "codebuddy_oauth",
        githubDomain: null,
        oauthFlowMode: null,
        kiroLoginProvider: null,
        qoderSite: null,
        codeBuddySite: "cn",
      },
    );
  });

  it("submits a complete Trae callback against the active flow", async () => {
    runtimeMocks.invokeCommand.mockResolvedValue({ id: "trae-account" });

    await authSubmitOauthCallback(
      "trae_solo",
      "trae-flow",
      "http://localhost:15721/api/accounts/trae/login/callback?code=x",
    );

    expect(runtimeMocks.invokeCommand).toHaveBeenCalledWith(
      "auth_submit_oauth_code",
      {
        authProvider: "trae_solo",
        deviceCode: "trae-flow",
        callbackUrl:
          "http://localhost:15721/api/accounts/trae/login/callback?code=x",
      },
    );
  });

  it("imports Qoder PAT through the dedicated secret-bearing command", async () => {
    runtimeMocks.invokeCommand.mockResolvedValue({
      ok: true,
      account: { id: "qoder-account" },
    });

    await importQoderPat("pt-secret");

    expect(runtimeMocks.invokeCommand).toHaveBeenCalledWith(
      "qoder_import_pat",
      { personalToken: "pt-secret" },
    );
  });
});
