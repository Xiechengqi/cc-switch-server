import { beforeEach, describe, expect, it, vi } from "vitest";
import { act, useState } from "react";
import { createRoot } from "react-dom/client";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { BuddyAccountModels } from "@/lib/api/providers";
import i18n from "@/i18n";
import { UpstreamModelInput, useBuddyModelCatalog } from "./BuddyModelSelector";

const mocks = vi.hoisted(() => ({ fetch: vi.fn() }));
vi.mock("@/lib/api/providers", () => ({ providersApi: { fetchBuddyAccountModels: mocks.fetch } }));
Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });

function fixture(accountId: string, id = "hy3"): BuddyAccountModels {
  return { providerType: "workbuddy_oauth", accountId, authIdentityGeneration: 1,
    models: [{ id, upstreamModel: id, displayName: `Model ${id}` }], source: "fixture", stale: false, fetchedAtMs: 1 };
}

function Harness({ account = "a", initial = "auto", product = "workbuddy_oauth", generation = 1 }: {
  account?: string; initial?: string; product?: string; generation?: number;
}) {
  const catalog = useBuddyModelCatalog(product, account, generation, true);
  const [value, setValue] = useState(initial);
  return <><UpstreamModelInput catalog={catalog} value={value} onValueChange={setValue} aria-label="fixed-model" />
    <output>{JSON.stringify(catalog.data?.models ?? [])}</output></>;
}

async function mount() {
  const container = document.createElement("div"); document.body.appendChild(container);
  const root = createRoot(container);
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return {
    container,
    render: async (props: Parameters<typeof Harness>[0] = {}) => {
      await act(async () => { root.render(<QueryClientProvider client={client}><Harness {...props} /></QueryClientProvider>); });
      await act(async () => { await new Promise((resolve) => setTimeout(resolve, 0)); });
    },
    close: async () => { await act(async () => root.unmount()); client.clear(); container.remove(); },
  };
}

beforeEach(() => {
  mocks.fetch.mockReset();
  mocks.fetch.mockImplementation(async ({ accountId }) => fixture(accountId));
});

describe("Buddy model selection", () => {
  it.each(["codebuddy_oauth", "workbuddy_oauth"])("shows catalog image capabilities for %s without changing the model", async (product) => {
    mocks.fetch.mockResolvedValue({ ...fixture("a"), models: [
      { id: "vision", upstreamModel: "vision", raw: { capabilities: { images: true } } },
      { id: "text", upstreamModel: "text", raw: { capabilities: { images: false } } },
      { id: "legacy", upstreamModel: "legacy" },
    ] });
    const view = await mount();
    try {
      await view.render({ product });
      await act(async () => view.container.querySelector<HTMLButtonElement>(`button[aria-label="${i18n.t("buddyModels.choose")}"]`)!.click());
      const options = Array.from(document.querySelectorAll<HTMLButtonElement>('[role="option"]'));
      for (const [id, label] of [["vision", "imageInput"], ["text", "textInput"], ["legacy", "unknownImageInput"]]) {
        const option = options.find((option) => option.querySelector(".font-mono")?.textContent === id)!;
        expect(option.textContent).toContain(i18n.t(`buddyModels.${label}`));
      }
      expect(view.container.querySelector<HTMLInputElement>("input")!.value).toBe("auto");
      expect(view.container.textContent).toContain(i18n.t("buddyModels.autoHint"));
    } finally { await view.close(); }
  });

  it("loads automatically without changing auto and saves a real ID only when selected", async () => {
    const view = await mount();
    try {
      await view.render();
      const input = view.container.querySelector<HTMLInputElement>("input")!;
      expect(input.value).toBe("auto");
      expect(mocks.fetch).toHaveBeenCalledTimes(1);
      await act(async () => view.container.querySelector<HTMLButtonElement>(`button[aria-label="${i18n.t("buddyModels.choose")}"]`)!.click());
      const option = document.querySelector<HTMLButtonElement>('[role="option"][aria-selected="false"]')!;
      expect(option.textContent).toContain("hy3");
      await act(async () => option.click());
      expect(input.value).toBe("hy3");
    } finally { await view.close(); }
  });

  it("never displays a previous account's late response after an unsaved account switch", async () => {
    let finishA!: (value: BuddyAccountModels) => void;
    let signalA: AbortSignal | undefined;
    mocks.fetch.mockImplementation(({ accountId }, _force, signal) => {
      if (accountId === "a") { signalA = signal; return new Promise((resolve) => { finishA = resolve; }); }
      return Promise.resolve(fixture(accountId, "new-account-model"));
    });
    const view = await mount();
    try {
      await view.render({ account: "a" });
      await view.render({ account: "b" });
      expect(signalA?.aborted).toBe(true);
      await act(async () => { finishA(fixture("a", "old-account-model")); });
      expect(view.container.textContent).toContain("new-account-model");
      expect(view.container.textContent).not.toContain("old-account-model");
      expect(view.container.querySelector<HTMLInputElement>("input")!.value).toBe("auto");
    } finally { await view.close(); }
  });

  it("preserves a saved model on failure and makes manual refresh bypass fresh cache", async () => {
    mocks.fetch.mockRejectedValueOnce(new Error("offline"));
    const view = await mount();
    try {
      await view.render({ initial: "old-model" });
      expect(view.container.textContent).toContain("offline");
      expect(view.container.querySelector<HTMLInputElement>("input")!.value).toBe("old-model");
      await act(async () => view.container.querySelector<HTMLButtonElement>(`button[aria-label="${i18n.t("buddyModels.refresh")}"]`)!.click());
      await act(async () => { await new Promise((resolve) => setTimeout(resolve, 0)); });
      expect(mocks.fetch.mock.lastCall?.[1]).toBe(true);
      expect(view.container.textContent).toContain(i18n.t("buddyModels.missing"));
      expect(view.container.querySelector<HTMLInputElement>("input")!.value).toBe("old-model");
    } finally { await view.close(); }
  });

  it("does not carry a pending manual refresh flag to another account", async () => {
    let finish!: (value: BuddyAccountModels) => void;
    mocks.fetch.mockImplementation(({ accountId }, force) => force && accountId === "a"
      ? new Promise((resolve) => { finish = resolve; }) : Promise.resolve(fixture(accountId)));
    const view = await mount();
    try {
      await view.render();
      await act(async () => view.container.querySelector<HTMLButtonElement>(`button[aria-label="${i18n.t("buddyModels.refresh")}"]`)!.click());
      await view.render({ account: "b" });
      expect(mocks.fetch.mock.lastCall?.[0].accountId).toBe("b");
      expect(mocks.fetch.mock.lastCall?.[1]).toBe(false);
      await act(async () => { finish(fixture("a", "old-refreshed-model")); });
      expect(view.container.textContent).not.toContain("old-refreshed-model");
    } finally { await view.close(); }
  });

  it("isolates product/generation changes, explains empty and stale catalogs, and waits for an account", async () => {
    const view = await mount();
    try {
      await view.render({ account: "" });
      expect(mocks.fetch).not.toHaveBeenCalled();
      expect(view.container.textContent).toContain(i18n.t("buddyModels.selectAccount"));
      mocks.fetch.mockResolvedValueOnce({ ...fixture("a"), models: [] });
      await view.render();
      expect(view.container.textContent).toContain(i18n.t("buddyModels.empty"));
      mocks.fetch.mockResolvedValueOnce({ ...fixture("a"), authIdentityGeneration: 2, stale: true });
      await view.render({ generation: 2 });
      expect(mocks.fetch).toHaveBeenCalledTimes(2);
      expect(view.container.textContent).toContain(i18n.t("buddyModels.stale", { time: new Date(1).toLocaleString() }));
      await view.render({ product: "codebuddy_oauth", generation: 2 });
      expect(mocks.fetch).toHaveBeenCalledTimes(3);
    } finally { await view.close(); }
  });
});
