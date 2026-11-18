import { act, useState } from "react";
import { createRoot } from "react-dom/client";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, expect, it, vi } from "vitest";
import type { ZenModelCatalog } from "@/lib/api/providers";
import i18n from "@/i18n";
import { familyById } from "@/server/providerRegistry";
import { createProviderBundleDraft, changeModelPolicyScope, updateBundleModel, updateSurfaceModel,
  toProviderBundleWriteDraft, validateProviderBundleDraft, validateProviderBundleDraftIssue } from "./bundleDraft";
import { UpstreamModelInput } from "./UpstreamModelInput";
import { useZenModelCatalog } from "./ZenModelSelector";

const mocks = vi.hoisted(() => ({ fetch: vi.fn() }));
vi.mock("@/lib/api/providers", () => ({ providersApi: { fetchZenModels: mocks.fetch } }));
Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });

function fixture(): ZenModelCatalog {
  const model: ZenModelCatalog["models"][number] = { id: "big-pickle", upstreamModel: "big-pickle", displayName: "Big Pickle",
    modelType: "chat", pricing: "free", pricingSource: "upstream_declared", selectable: true,
    unavailableReason: null, protocol: "chat", contextWindow: 200000, maxOutputTokens: 32000 };
  return { source: "opencode_zen_live_model_catalog", stale: false, fetchedAtMs: 1,
    models: [model, { ...model, id: "paid-free", upstreamModel: "paid-free", displayName: "Paid model",
      pricing: "paid", selectable: false, unavailableReason: "paid_model" }] };
}

function Harness({ scope = "global", initial = "", enabled = true, familyId = "family.opencode_zen" }: {
  scope?: "global" | "per_app"; initial?: string; enabled?: boolean; familyId?: string;
}) {
  const catalog = useZenModelCatalog(familyId, enabled);
  const [draft, setDraft] = useState(() => changeModelPolicyScope(updateBundleModel(
    createProviderBundleDraft(familyById("family.opencode_zen")!), "single", initial), scope));
  const props = { zenCatalog: catalog, catalog: undefined };
  return <>
    {scope === "global" ? <UpstreamModelInput {...props} aria-label="global-model" value={draft.upstreamModel}
      onValueChange={(value) => setDraft((old) => updateBundleModel(old, "single", value))} />
      : draft.surfaces.map((surface) => <UpstreamModelInput {...props} key={surface.app} aria-label={`${surface.app}-model`}
        value={surface.upstreamModel} onValueChange={(value) => setDraft((old) => ({ ...old,
          surfaces: old.surfaces.map((s) => s.app === surface.app ? updateSurfaceModel(s, "single", value) : s) }))} />)}
    <output>{JSON.stringify(toProviderBundleWriteDraft(draft))}</output>
  </>;
}

async function mount(props: Parameters<typeof Harness>[0] = {}) {
  const container = document.createElement("div"); document.body.appendChild(container);
  const root = createRoot(container);
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const render = async (next = props) => {
    await act(async () => { root.render(<QueryClientProvider client={client}><Harness {...next} /></QueryClientProvider>); });
    await act(async () => { await new Promise((resolve) => setTimeout(resolve, 0)); });
  };
  await render();
  return { container, render, close: async () => { await act(async () => root.unmount()); client.clear(); container.remove(); } };
}

async function open(container: HTMLElement, index = 0) {
  const button = container.querySelectorAll<HTMLButtonElement>(`button[aria-label="${i18n.t("zenModels.choose")}"]`)[index];
  await act(async () => button.click());
}

beforeEach(() => { mocks.fetch.mockReset(); mocks.fetch.mockResolvedValue(fixture()); });

it.each(["global", "per_app"] as const)("loads once for an unsaved %s draft, marks Free, and saves only an explicit choice", async (scope) => {
  const view = await mount({ scope });
  try {
    expect(mocks.fetch).toHaveBeenCalledTimes(1);
    expect(view.container.querySelector<HTMLInputElement>("input")!.value).toBe("");
    await open(view.container);
    const options = document.querySelectorAll<HTMLButtonElement>('[role="option"]');
    expect(options.length).toBe(1);
    expect(options[0].textContent).toContain(i18n.t("zenModels.free"));
    expect(options[0].textContent).toContain("big-pickle");
    await act(async () => options[0].click());
    const saved = JSON.parse(view.container.querySelector("output")!.textContent!);
    if (scope === "global") expect(saved.upstreamModel).toBe("big-pickle");
    else {
      expect(saved.surfaces[0].upstreamModel).toBe("big-pickle");
      expect(saved.surfaces[1].upstreamModel).toBe("");
    }
    expect(view.container.textContent).not.toContain(i18n.t("buddyModels.autoHint"));
  } finally { await view.close(); }
});

it("searches by name and ID, and disables paid options when all models are shown", async () => {
  const view = await mount();
  try {
    await open(view.container);
    await act(async () => document.querySelector<HTMLInputElement>('input[type="checkbox"]')!.click());
    const options = document.querySelectorAll<HTMLButtonElement>('[role="option"]');
    expect(options.length).toBe(2);
    expect(options[1].disabled).toBe(true);
    expect(options[1].textContent).toContain(i18n.t("zenModels.reason.paid_model"));
    for (const key of ["ArrowDown", "ArrowUp", "Home", "End"]) {
      await act(async () => {
        options[0].focus();
        options[0].dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true }));
      });
      expect(document.activeElement).toBe(options[0]);
    }
    const search = document.querySelector<HTMLInputElement>(`input[aria-label="${i18n.t("zenModels.search")}"]`)!;
    for (const needle of ["Big Pickle", "big-pickle"]) {
      await act(async () => {
        Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(search, needle);
        search.dispatchEvent(new Event("input", { bubbles: true }));
      });
      expect(document.querySelectorAll('[role="option"]').length).toBe(1);
    }
  } finally { await view.close(); }
});

it("preserves hand-entered IDs through failure and forced refresh without auto-selecting", async () => {
  mocks.fetch.mockRejectedValueOnce(new Error("offline"));
  const view = await mount({ initial: "my-custom-free" });
  try {
    expect(view.container.textContent).toContain(i18n.t("zenModels.failed"));
    const input = view.container.querySelector<HTMLInputElement>("input")!;
    await act(async () => {
      Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(input, "my-next-model");
      input.dispatchEvent(new Event("input", { bubbles: true }));
    });
    await act(async () => view.container.querySelector<HTMLButtonElement>(`button[aria-label="${i18n.t("zenModels.refresh")}"]`)!.click());
    await act(async () => { await new Promise((resolve) => setTimeout(resolve, 0)); });
    expect(mocks.fetch.mock.lastCall?.[0]).toBe(true);
    expect(input.value).toBe("my-next-model");
    expect(JSON.parse(view.container.querySelector("output")!.textContent!).upstreamModel).toBe("my-next-model");
    expect(view.container.textContent).toContain(i18n.t("zenModels.missing"));
  } finally { await view.close(); }
});

it("distinguishes stale and empty catalogs and never fetches for another family or passthrough", async () => {
  const view = await mount({ enabled: false, initial: "old-model" });
  try {
    expect(mocks.fetch).not.toHaveBeenCalled();
    await view.render({ familyId: "family.codebuddy", enabled: true });
    expect(mocks.fetch).not.toHaveBeenCalled();
    mocks.fetch.mockResolvedValueOnce({ ...fixture(), models: [], stale: true });
    await view.render({ enabled: true });
    expect(view.container.textContent).toContain(i18n.t("zenModels.empty"));
    expect(view.container.textContent).toContain(i18n.t("zenModels.stale", { time: new Date(1).toLocaleString() }));
    expect(view.container.textContent).not.toContain(i18n.t("zenModels.missing"));
    expect(view.container.querySelector<HTMLInputElement>("input")!.value).toBe("old-model");
  } finally { await view.close(); }
});

it("requires an explicit fixed model without a synthetic default", () => {
  const initial = createProviderBundleDraft(familyById("family.opencode_zen")!);
  expect(initial.upstreamModel).toBe("");
  const fixed = updateBundleModel(initial, "single", "");
  expect(validateProviderBundleDraftIssue(fixed)?.code).toBe("upstreamModelRequired");
  expect(validateProviderBundleDraft(updateBundleModel(fixed, "single", "custom-model"))).toBeNull();
});
