import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import en from "@/i18n/server-locales/en.json";
import { LeaseWorkspace } from "./LeaseWorkspace";

const { request, proof } = vi.hoisted(() => ({
  request: vi.fn(),
  proof: vi.fn(),
}));
vi.mock("@/lib/runtime", () => ({ jsonFetch: request, apiFetch: vi.fn() }));
vi.mock("@/lib/routerAuth", () => ({ setWorkspaceOwnerProof: proof, setMaintenanceVisit:vi.fn() }));
vi.mock("react-i18next", () => ({
  useTranslation: () => ({ i18n: { language: "en" }, t:(key:string)=>en.lease[key.split(".")[1] as keyof typeof en.lease] || key }),
}));
Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });

let host: HTMLDivElement;
let root: Root;
beforeEach(() => {
  request.mockReset();
  proof.mockReset();
  host = document.createElement("div");
  document.body.append(host);
  root = createRoot(host);
});
afterEach(() => {
  act(() => root.unmount());
  host.remove();
});

it("shows workspace backups and the owner access notice to a tenant", async () => {
  request.mockResolvedValue({ backups: ["snapshot-1"], restorable: true });
  await act(async () =>
    root.render(
      <LeaseWorkspace role="tenant">
        <p>Tenant controls</p>
      </LeaseWorkspace>,
    ),
  );
  expect(request.mock.calls.map((c) => c[0])).toEqual([
    "/web-api/market/backups",
  ]);
  expect(host.textContent).toContain("snapshot-1");
  expect(host.textContent).toContain("root access");
  expect(host.textContent).toContain("Tenant controls");
  expect(host.querySelector('input[type="password"]')).toBeNull();
});

it("requires owner login before changing installation maintenance settings", async () => {
  const maintenance = {
    terminalEnabled: true,
    terminalAvailable: true,
    upgradePolicy: {
      delegateUpgradeToRouterOwner: false,
      autoUpgradeEnabled: false,
      autoUpgradeCheckIntervalMinutes: 60,
    },
  };
  request.mockResolvedValueOnce({
    providers: [],
    audit: [],
    version: "v1",
    shareCount: 2,
    connectivity: { registered: true, lastHeartbeatMs: 1000 },
    hardware: { arch: "x86_64", cpu: 2, memoryBytes: 2 ** 30, diskBytes: null },
    maintenance,
  });
  await act(async () => root.render(<LeaseWorkspace role="assistance" />));
  expect(host.textContent).toContain("Last heartbeat");
  expect(host.textContent).toContain("1 GiB");
  expect(host.querySelector('[name="interval"]')).toBeNull();
  const password = host.querySelector<HTMLInputElement>(
    'input[type="password"]',
  )!;
  await act(async () => {
    Object.getOwnPropertyDescriptor(
      HTMLInputElement.prototype,
      "value",
    )!.set!.call(password, "sample-owner-password");
    password.dispatchEvent(new Event("input", { bubbles: true }));
  });
  const credential = {accessToken:"sample-owner-proof",refreshToken:"refresh",expiresAt:new Date(Date.now()+600000).toISOString()};
  request.mockResolvedValueOnce(credential).mockRejectedValueOnce(new Error("owner_proof_required"));
  await act(async () => {password.form!.requestSubmit();});
  expect(host.querySelector('[name="interval"]')).toBeNull();
  expect(proof).toHaveBeenLastCalledWith(null);
  request.mockResolvedValueOnce(credential).mockResolvedValueOnce({authorized:true,instanceId:"first"});
  vi.spyOn(window,"confirm").mockReturnValue(true);
  await act(async () => {
    password.form!.requestSubmit();
  });
  expect(proof).toHaveBeenCalledWith("sample-owner-proof");
  const interval = host.querySelector<HTMLInputElement>('[name="interval"]')!;
  interval.value = "45";
  request.mockResolvedValueOnce(maintenance);
  await act(async () => {
    interval.form!.requestSubmit();
  });
  const [path, options] = request.mock.calls.at(-1)!;
  expect(path).toBe("/web-api/market/maintenance");
  expect(JSON.parse(options.body)).toEqual({
    terminalEnabled: true,
    upgradePolicy: {
      ...maintenance.upgradePolicy,
      autoUpgradeCheckIntervalMinutes: 45,
    },
  });
});
