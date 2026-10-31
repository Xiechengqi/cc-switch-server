import { afterEach, describe, expect, it, vi } from "vitest";
import { HttpResponseError, jsonFetch, invokeCommand } from "./runtime";

vi.mock("@/lib/api/auth", () => ({ isRemoteWebMode: () => false }));
vi.mock("@/lib/routerAuth", () => ({
  clearRouterSessionTokens: vi.fn(),
  readRouterAccessToken: () => null,
  routerAuthFetch: vi.fn(),
}));
afterEach(() => vi.unstubAllGlobals());

describe("structured HTTP errors", () => {
  it("forwards readonly quota preview cancellation without caching its private response",async()=>{
    const controller=new AbortController();const fetch=vi.fn().mockResolvedValue(Response.json({ready:false}));
    vi.stubGlobal("fetch",fetch);
    await invokeCommand("preview_share_value_quota",{shareId:"share",expectedConfigRevision:1},{signal:controller.signal,cache:"no-store"});
    expect(fetch).toHaveBeenCalledWith("/web-api/invoke/preview_share_value_quota",expect.objectContaining({signal:controller.signal,cache:"no-store",method:"POST"}));
  });
  it.each([
    {
      error: "authentication mismatch",
      code: "cc_switch_share_credential_source_mismatch",
      details: { reason: "auth_mismatch" },
      retryable: false,
    },
    {
      error: {
        message: "authentication mismatch",
        code: "cc_switch_share_credential_source_mismatch",
        details: { reason: "auth_mismatch" },
        retryable: false,
      },
    },
  ])("preserves message, code and safe diagnostics", async (body) => {
    vi.stubGlobal(
      "fetch",
      vi
        .fn()
        .mockResolvedValue(new Response(JSON.stringify(body), { status: 409 })),
    );
    await expect(
      jsonFetch("/web-api/invoke/validate_provider_bundle_draft"),
    ).rejects.toMatchObject({
      name: "HttpResponseError",
      status: 409,
      message: "authentication mismatch",
      code: "cc_switch_share_credential_source_mismatch",
      details: { reason: "auth_mismatch" },
      retryable: false,
    });
  });
  it("handles malformed/non-JSON error bodies without losing the status", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn().mockResolvedValue(new Response("bad gateway", { status: 502 })),
    );
    await expect(jsonFetch("/web-api/invoke/example")).rejects.toMatchObject({
      status: 502,
      message: "HTTP 502",
    });
    expect(new HttpResponseError("legacy", 409).code).toBeUndefined();
  });
});
