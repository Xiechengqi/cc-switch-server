import { afterEach, describe, expect, it, vi } from "vitest";

const transport = vi.hoisted(() => ({
  invokeCommand: vi.fn(), readToken: vi.fn(), readRouterAccessToken: vi.fn(),
}));
vi.mock("@/lib/runtime", () => ({invokeCommand: transport.invokeCommand, readToken: transport.readToken}));
vi.mock("@/lib/routerAuth", () => ({readRouterAccessToken: transport.readRouterAccessToken}));
import { shareApi } from "./share";

describe("USD usage correction identity", () => {
  afterEach(() => vi.resetAllMocks());
  it("keeps an unchanged local and Router session's result", async () => {
    transport.readToken.mockReturnValue("admin-session");
    transport.readRouterAccessToken.mockReturnValue("router-session");
    transport.invokeCommand.mockResolvedValue({items: []});
    await expect(shareApi.editValueUsage("share", 3, {})).resolves.toEqual({items: []});
    expect(transport.invokeCommand).toHaveBeenCalledWith("edit_share_value_quota_usage", {shareId: "share", expectedConfigRevision: 3, edits: {}});
  });
  it.each(["local", "router"])("does not publish a late result after the %s identity changed", async (which) => {
    transport.readToken.mockReturnValue("admin-session");
    transport.readRouterAccessToken.mockReturnValue("router-session");
    let resolve!: (value: unknown) => void;
    transport.invokeCommand.mockImplementation(() => new Promise((done) => { resolve = done; }));
    const pending = shareApi.editValueUsage("share", 3, {});
    (which === "local" ? transport.readToken : transport.readRouterAccessToken).mockReturnValue("changed-session");
    resolve({items: []});
    await expect(pending).rejects.toMatchObject({name: "AbortError"});
  });
});
