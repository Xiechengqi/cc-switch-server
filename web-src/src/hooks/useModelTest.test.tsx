import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "@/i18n";
import type { StreamCheckResult } from "@/lib/api/model-test";
import { useModelTest } from "./useModelTest";

const mocks = vi.hoisted(() => ({
  testProvider: vi.fn(),
  warning: vi.fn(),
  error: vi.fn(),
  success: vi.fn(),
}));
vi.mock("@/lib/api/model-test", () => ({
  modelTestProvider: mocks.testProvider,
}));
vi.mock("sonner", () => ({
  toast: { warning: mocks.warning, error: mocks.error, success: mocks.success },
}));

let container: HTMLDivElement;
let root: Root;
let modelTest: ReturnType<typeof useModelTest>;
function Harness() {
  modelTest = useModelTest("codex");
  return null;
}

beforeEach(async () => {
  vi.resetAllMocks();
  Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
  await i18n.changeLanguage("zh");
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  await act(async () => root.render(<Harness />));
});
afterEach(async () => {
  await act(async () => root.unmount());
  container.remove();
});

const failed: StreamCheckResult = {
  status: "failed",
  success: false,
  message: "OpenCode Zen upstream rate_limit: HTTP 429; request_id=safe-123",
  httpStatus: 429,
  testedAt: 0,
  retryCount: 0,
  inputTokens: 0,
  outputTokens: 0,
  cacheReadTokens: 0,
  cacheCreationTokens: 0,
};

async function run(result: StreamCheckResult, name = "OpenCode Zen/Codex") {
  mocks.testProvider.mockResolvedValue(result);
  let returned: StreamCheckResult | null = null;
  await act(async () => {
    returned = await modelTest.testProvider("zen-codex", name);
  });
  expect(returned).toEqual(result);
  expect(mocks.testProvider).toHaveBeenCalledExactlyOnceWith("codex", "zen-codex");
  expect(modelTest.isTesting("zen-codex")).toBe(false);
}

describe("model test quota and rate limit feedback", () => {
  it.each(["zh", "zh-TW", "en", "ja"])("distinguishes Zen free access and cooldown with a retry time in %s", async (language) => {
    await act(async () => { await i18n.changeLanguage(language); });
    await run({ ...failed, errorCategory: "zenFreeUsageLimit", retryAfterSeconds: 43200 });
    expect(mocks.warning.mock.calls[0][0]).toBe(i18n.t("streamCheck.zenFreeAccess", { providerName: "OpenCode Zen/Codex" }));
    expect(mocks.warning.mock.calls[0][1].description).toContain(i18n.t("streamCheck.zenFreeAccessHint"));
    expect(mocks.warning.mock.calls[0][1].description).toContain(failed.message);
    expect(mocks.warning.mock.calls[0][1].description).not.toContain("streamCheck.");
    mocks.warning.mockClear();
    mocks.testProvider.mockClear();
    await run({ ...failed, errorCategory: "zenLocalCooldown", retryAfterSeconds: 60 });
    expect(mocks.warning.mock.calls[0][1].description).toContain(i18n.t("streamCheck.zenLocalCooldownHint"));
    expect(mocks.warning.mock.calls[0][1].description.split("\n")).toHaveLength(3);
  });

  it.each(["zh", "zh-TW", "en", "ja"])(
    "shows rate limiting with the returned detail in %s",
    async (language) => {
      await act(async () => {
        await i18n.changeLanguage(language);
      });
      await run({ ...failed, errorCategory: "rateLimit" });
      expect(mocks.warning).toHaveBeenCalledExactlyOnceWith(
        i18n.t("streamCheck.rateLimit", { providerName: "OpenCode Zen/Codex" }),
        expect.objectContaining({
          description: `${i18n.t("streamCheck.rateLimitHint")}\n${failed.message}`,
          closeButton: true,
        }),
      );
      expect(mocks.warning.mock.calls[0][0]).toContain("OpenCode Zen/Codex");
      expect(mocks.warning.mock.calls[0][1].description).not.toContain("streamCheck.");
      expect(mocks.error).not.toHaveBeenCalled();
    },
  );

  it.each(["zh", "zh-TW", "en", "ja"])(
    "does not attribute another provider's quota to Qianfan in %s",
    async (language) => {
      await act(async () => {
        await i18n.changeLanguage(language);
      });
      const result = {
        ...failed,
        httpStatus: 402,
        errorCategory: "quotaExceeded",
        message: "upstream quota exhausted",
      };
      await run(result);
      expect(mocks.warning).toHaveBeenCalledExactlyOnceWith(
        i18n.t("streamCheck.quotaExceeded", { providerName: "OpenCode Zen/Codex" }),
        expect.objectContaining({
          description: `${i18n.t("streamCheck.quotaExceededHint")}\n${result.message}`,
        }),
      );
      const text = JSON.stringify(mocks.warning.mock.calls);
      expect(mocks.warning.mock.calls[0][0]).toContain("OpenCode Zen/Codex");
      expect(text).not.toContain("streamCheck.");
      expect(text).not.toMatch(
        /Coding Plan|千帆|Qianfan|5\s*(小时|小時|時間|hours?|[- ]hour)/i,
      );
      expect(mocks.error).not.toHaveBeenCalled();
    },
  );

  it("shows a local cooldown as rate limiting and keeps the diagnostic detail", async () => {
    const result = {
      ...failed,
      errorCategory: "rateLimit",
      message: "OpenCode Zen public lane is cooling down",
    };
    await run(result);
    expect(mocks.warning.mock.calls[0][1].description).toContain(result.message);
    expect(mocks.warning.mock.calls[0][0]).not.toContain("额度");
  });

  it("keeps quota guidance provider-neutral for a Coding Plan provider too", async () => {
    await run(
      { ...failed, httpStatus: 402, errorCategory: "quotaExceeded", message: "upstream quota exhausted" },
      "百度千帆",
    );
    expect(mocks.warning.mock.calls[0][0]).toContain("百度千帆");
    expect(mocks.warning.mock.calls[0][1].description).toContain(
      i18n.t("streamCheck.quotaExceededHint"),
    );
    expect(mocks.warning.mock.calls[0][1].description).not.toContain("5 小时");
  });

  it("keeps other failures on the normal error path", async () => {
    const result = {
      ...failed,
      httpStatus: 502,
      errorCategory: "upstream",
      message: "OpenCode Zen upstream: HTTP 502",
    };
    await run(result);
    expect(mocks.error).toHaveBeenCalledExactlyOnceWith(
      i18n.t("streamCheck.failed", {
        providerName: "OpenCode Zen/Codex",
        message: result.message,
      }),
      expect.objectContaining({ closeButton: true }),
    );
    expect(mocks.warning).not.toHaveBeenCalled();
  });
});
