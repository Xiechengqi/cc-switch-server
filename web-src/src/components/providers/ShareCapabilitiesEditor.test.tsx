import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, useState } from "react";
import { createRoot, type Root } from "react-dom/client";
import i18n from "@/i18n";
import {
  emptyCapabilityPolicy,
  capabilityQuotaContext,
  SHARE_OPERATIONS,
  type CapabilityPreview,
  type ShareCapabilityPolicy,
  type CapabilityConsent,
} from "@/lib/share-capabilities";
import type { ShareUserGrantMap } from "@/lib/api/share";
const api = vi.hoisted(() => ({ previewCapabilities: vi.fn() }));
vi.mock("@/lib/api/share", () => ({ shareApi: api }));
import { ShareCapabilitiesEditor } from "./ShareCapabilitiesEditor";
Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
const operation = "tool.web_search" as const;
function preview(
  input: {
    policy?: ShareCapabilityPolicy;
    policies: Record<
      string,
      { valueQuota?: unknown; allowedOperations?: string[] }
    >;
  },
  shareId = "share",
): CapabilityPreview {
  const policy = input.policy ?? emptyCapabilityPolicy();
  const selected = policy.allowedOperations.includes(operation);
  const scopes = Object.entries(input.policies)
    .filter(
      ([, p]) =>
        !p.allowedOperations || p.allowedOperations.includes(operation),
    )
    .map(([scope, p]) => ({
      scope,
      quotaUnit: p.valueQuota ? "usdEquivalent" : "tokens",
      coverage: p.valueQuota ? "unknown" : "partial",
      reason: p.valueQuota
        ? "priced_requests_only"
        : "resource_not_in_token_ledger",
      condition: p.valueQuota ? undefined : `gap:${scope}`,
    }));
  return {
    shareId,
    configRevision: 1,
    ready: true,
    legacy: false,
    policy,
    expiresAt: Date.now() + 900000,
    previewCredential: "signed-ticket",
    requiredAcknowledgements: selected
      ? scopes
          .filter((s) => s.condition)
          .map((s) => ({
            condition: s.condition!,
            scope: s.scope,
            operation,
            quotaUnit: s.quotaUnit,
            bindingFingerprint: "a".repeat(64),
            actor: "",
            acceptedAtMs: 0,
          }))
      : [],
    rows: SHARE_OPERATIONS.map((op) => ({
      app: "codex",
      operation: op,
      bindingFingerprint: "a".repeat(64),
      controllable: op === operation,
      selected: op === operation && selected,
      execution: "unknown",
      measurement: "estimated",
      nativeUnit: op,
      pricing: "missing",
      admission: "estimated_reservation",
      scopes: op === operation ? scopes : [],
    })),
  };
}
let root: Root;
let container: HTMLDivElement;
beforeEach(async () => {
  await i18n.changeLanguage("en");
  vi.useFakeTimers();
  api.previewCapabilities.mockReset();
  api.previewCapabilities.mockImplementation((shareId, input) =>
    Promise.resolve(preview(input, shareId)),
  );
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
});
afterEach(async () => {
  await act(async () => root.unmount());
  container.remove();
  vi.useRealTimers();
});
const button = (label: string) =>
  Array.from(container.querySelectorAll<HTMLButtonElement>("button")).find(
    (b) => b.textContent === label,
  )!;
const input = (label: string) =>
  Array.from(container.querySelectorAll<HTMLLabelElement>("label"))
    .find((l) => l.textContent === label)!
    .querySelector<HTMLInputElement>("input")!;
async function flush() {
  await act(async () => {
    await vi.advanceTimersByTimeAsync(250);
  });
}
describe("Server-native capability configuration", () => {
  it("requires scoped Token acceptance, invalidates it after changing unit and protects managed grants", async () => {
    let latest: {
      policy: ShareCapabilityPolicy;
      consent?: CapabilityConsent;
      valid: boolean;
    };
    function Harness() {
      const [policy, setPolicy] = useState(emptyCapabilityPolicy);
      const [grants, setGrants] = useState<ShareUserGrantMap>({
        "a@example.com": {
          email: "a@example.com",
          role: "shareto",
          active: true,
          policy: { tokenPeriod: "day", tokenLimit: 100000000 },
        },
        "market@example.com": {
          email: "market@example.com",
          role: "shareto",
          active: true,
          manager: "routerShareMarket",
          policy: { tokenPeriod: "day", allowedOperations: [] },
        },
      });
      const [consent, setConsent] = useState<CapabilityConsent>();
      const [scope, setScope] = useState<string>();
      const valid =
        !!consent && scope === capabilityQuotaContext(policy, grants, -1);
      latest = { policy, consent, valid };
      return (
        <>
          <ShareCapabilitiesEditor
            shareId="share"
            revision={1}
            policy={policy}
            savedPolicy={emptyCapabilityPolicy()}
            grants={grants}
            tokenLimit={-1}
            onChange={(p, c) => {
              setPolicy(p);
              setConsent(c);
              setScope(c ? capabilityQuotaContext(p, grants, -1) : undefined);
            }}
            onGrantOperationsChange={(email, allowedOperations) => {
              setGrants((current) => ({
                ...current,
                [email]: {
                  ...current[email],
                  policy: { ...current[email].policy, allowedOperations },
                },
              }));
              setConsent(undefined);
            }}
          />
          <button
            onClick={() =>
              setGrants((current) => ({
                ...current,
                "a@example.com": {
                  ...current["a@example.com"],
                  policy: {
                    tokenPeriod: "day",
                    valueQuota: {
                      limitUsdMicros: "100000000",
                      tariffPolicyId: "api-equivalent-v2",
                      unpricedUsage: "reject",
                    },
                  },
                },
              }))
            }
          >
            Switch unit
          </button>
        </>
      );
    }
    await act(async () => root.render(<Harness />));
    await flush();
    expect(latest!.policy.allowedOperations).toEqual([]);
    await act(async () => input("Web search").click());
    await flush();
    expect(button("Confirm capabilities").disabled).toBe(true);
    const accept = Array.from(
      container.querySelectorAll<HTMLLabelElement>("label"),
    )
      .find((l) => l.textContent?.startsWith("a@example.com · Web search"))!
      .querySelector<HTMLInputElement>("input")!;
    await act(async () => accept.click());
    await act(async () => button("Confirm capabilities").click());
    expect(latest!.valid).toBe(true);
    expect(latest!.consent?.acceptUncovered).toEqual(["gap:a@example.com"]);
    await act(async () => button("Switch unit").click());
    await flush();
    expect(latest!.valid).toBe(false);
    await act(async () => button("Confirm capabilities").click());
    expect(latest!.consent?.acceptUncovered).toEqual([]);
    expect(latest!.valid).toBe(true);
    expect(
      container.querySelector(
        'select[aria-label="market@example.com capabilities"]',
      ),
    ).toBeNull();
  });
  it("discards late previews from a previous Share even when the transport ignores abort", async () => {
    const waiting = new Map<string, (value: CapabilityPreview) => void>();
    api.previewCapabilities.mockImplementation(
      (id) => new Promise((resolve) => waiting.set(id, resolve)),
    );
    const props = {
      revision: 1,
      policy: emptyCapabilityPolicy(),
      grants: {},
      tokenLimit: -1,
      onChange: vi.fn(),
      onGrantOperationsChange: vi.fn(),
    };
    await act(async () =>
      root.render(<ShareCapabilitiesEditor {...props} shareId="old" />),
    );
    await flush();
    await act(async () =>
      root.render(<ShareCapabilitiesEditor {...props} shareId="new" />),
    );
    await flush();
    const next = preview(
      { policy: emptyCapabilityPolicy(), policies: {} },
      "new",
    );
    next.rows[0].app = "current";
    await act(async () => waiting.get("new")!(next));
    const old = preview(
      { policy: emptyCapabilityPolicy(), policies: {} },
      "old",
    );
    old.rows[0].app = "late-old";
    await act(async () => waiting.get("old")!(old));
    expect(container.textContent).toContain("current");
    expect(container.textContent).not.toContain("late-old");
  });
});
