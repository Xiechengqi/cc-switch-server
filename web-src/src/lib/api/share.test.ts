import { beforeEach, describe, expect, it, vi } from "vitest";

const runtime = vi.hoisted(() => ({ invokeCommand: vi.fn() }));
vi.mock("@/lib/runtime", () => runtime);
import { shareApi } from "./share";

beforeEach(() => runtime.invokeCommand.mockReset());

describe("Bundle share preflight API", () => {
  it("uses the authenticated tunnel-compatible command without caching", async () => {
    const preflight = {
      bundleId: "custom", bundleRevision: 4,
      enabledApps: ["claude", "codex"], canShare: true,
    };
    runtime.invokeCommand.mockResolvedValue(preflight);
    await expect(shareApi.preflightProviderBundleShare("custom")).resolves.toBe(preflight);
    expect(runtime.invokeCommand).toHaveBeenCalledWith(
      "get_provider_bundle_share_preflight", { bundleId: "custom" }, { cache: "no-store" },
    );
  });
});
