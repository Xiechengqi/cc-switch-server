import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import en from "@/i18n/server-locales/en.json";
import { LeaseWorkspace } from "./LeaseWorkspace";

const { request } = vi.hoisted(() => ({ request: vi.fn() }));
vi.mock("@/lib/runtime", () => ({ jsonFetch: request, apiFetch: vi.fn() }));
vi.mock("react-i18next", () => ({
  useTranslation: () => ({ i18n: { language: "en" }, t:(key:string)=>en.lease[key.split(".")[1] as keyof typeof en.lease] || key }),
}));
Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });

let host: HTMLDivElement;
let root: Root;
beforeEach(() => {
  request.mockReset();
  host = document.createElement("div");
  document.body.append(host);
  root = createRoot(host);
});
afterEach(() => {
  act(() => root.unmount());
  host.remove();
});

it("keeps tenant tools without an owner login", async () => {
  request.mockResolvedValue({ backups: ["snapshot-1"], restorable: true });
  await act(async () =>
    root.render(
      <LeaseWorkspace leaseId="current">
        <p>Tenant controls</p>
      </LeaseWorkspace>,
    ),
  );
  expect(request.mock.calls.map((c) => c[0])).toEqual([
    "/web-api/market/backups",
  ]);
  expect(host.textContent).toContain("snapshot-1");
  expect(host.textContent).not.toContain("root access");
  expect(host.textContent).toContain("Tenant controls");
  expect(host.querySelector('input[type="password"]')).toBeNull();
});
