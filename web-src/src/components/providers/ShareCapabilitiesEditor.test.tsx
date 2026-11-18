import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, createRef, useState } from "react";
import { createRoot, type Root } from "react-dom/client";
import i18n from "@/i18n";
import {
  emptyCapabilityPolicy,
  SHARE_OPERATIONS,
  type CapabilityApproval,
  type CapabilityPreview,
  type CapabilityReviewHandle,
  type ShareCapabilityPolicy,
} from "@/lib/share-capabilities";
import type { ShareUserGrantMap } from "@/lib/api/share";
const api = vi.hoisted(() => ({ previewCapabilities: vi.fn() }));
vi.mock("@/lib/api/share", () => ({ shareApi: api }));
import { ShareCapabilitiesEditor } from "./ShareCapabilitiesEditor";
import { ShareCapabilityPermissionFields } from "./ShareCapabilityPermissionFields";
Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
const operation = "tool.web_search" as const;
const fp = "a".repeat(64);
let liveFingerprint = fp;
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
  const requested = input.policy ?? emptyCapabilityPolicy();
  const enabled = requested.allowedOperations.includes(operation);
  const selected =
    enabled &&
    (!requested.operationBindings[operation] ||
      requested.operationBindings[operation]?.some(
        (b) => b.fingerprint === liveFingerprint,
      ));
  const policy = {
    ...requested,
    operationBindings: enabled
      ? {
          [operation]: selected
            ? [{ app: "codex", fingerprint: liveFingerprint }]
            : [],
        }
      : {},
  };
  const scopes = Object.entries(input.policies)
    .filter(
      ([, p]) =>
        !p.allowedOperations || p.allowedOperations.includes(operation),
    )
    .map(([scope, p]) => ({
      scope,
      quotaUnit: p.valueQuota ? "usdEquivalent" : "tokens",
      coverage: "partial",
      reason: p.valueQuota
        ? "priced_requests_only"
        : "resource_not_in_token_ledger",
      condition: p.valueQuota ? undefined : `gap:${scope}:${liveFingerprint}`,
    }));
  return {
    shareId,
    configRevision: 1,
    displayVersion: 2,
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
            bindingFingerprint: liveFingerprint,
            actor: "",
            acceptedAtMs: 0,
          }))
      : [],
    rows: SHARE_OPERATIONS.map((op) => ({
      app: "codex",
      operation: op,
      bindingFingerprint: liveFingerprint,
      controllable: op === operation,
      implemented: op === operation,
      providerEnabled: true,
      accountSupport: "unknown",
      unavailableReason: op === operation ? null : "driver_not_implemented",
      modelScope: "request",
      pricingCoverage: "request_dependent",
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
  liveFingerprint = fp;
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
  Array.from(document.querySelectorAll<HTMLButtonElement>("button")).find(
    (b) => b.textContent === label,
  )!;
const checkbox = (label: string) =>
  Array.from(document.querySelectorAll<HTMLLabelElement>("label"))
    .find((l) => l.textContent?.startsWith(label))!
    .querySelector<HTMLInputElement>("input")!;
async function flush() {
  await act(async () => {
    await vi.advanceTimersByTimeAsync(250);
  });
}
const grants: ShareUserGrantMap = Object.fromEntries(
  ["a@example.com", "b@example.com"].map((email) => [
    email,
    {
      email,
      role: "shareto",
      active: true,
      policy: { tokenPeriod: "day", tokenLimit: 100000000 },
    },
  ]),
);

describe("Server extension configuration", () => {
  it("filters unsupported operations and reviews grouped risks only when saving", async () => {
    const ref = createRef<CapabilityReviewHandle>();
    let latest: CapabilityApproval | null = null;
    function Harness() {
      const [policy, setPolicy] = useState(emptyCapabilityPolicy);
      return (
        <>
          <ShareCapabilitiesEditor
            shareId="share"
            revision={1}
            policy={policy}
            savedPolicy={emptyCapabilityPolicy()}
            grants={grants}
            tokenLimit={-1}
            reviewRef={ref}
            onChange={setPolicy}
          />
          <button
            onClick={async () => {
              latest = await ref.current!.review();
            }}
          >
            Save settings
          </button>
        </>
      );
    }
    await act(async () => root.render(<Harness />));
    await flush();
    expect(container.querySelectorAll('input[type="checkbox"]')).toHaveLength(
      1,
    );
    expect(container.textContent).not.toContain("Hosted computer");
    await act(async () => checkbox("Web search").click());
    await flush();
    expect(document.querySelector('[role="dialog"]')).toBeNull();
    const calls = api.previewCapabilities.mock.calls.length;
    await act(async () => button("Save settings").click());
    expect(api.previewCapabilities.mock.calls.length).toBe(calls + 1);
    expect(document.querySelector('[role="dialog"]')).not.toBeNull();
    expect(button("Confirm and save").disabled).toBe(true);
    expect(
      document.querySelectorAll('[role="dialog"] input[type="checkbox"]'),
    ).toHaveLength(1);
    await act(async () =>
      checkbox("I understand and allow this uncovered usage").click(),
    );
    await act(async () => button("Confirm and save").click());
    expect(latest!.consent.acceptUncovered).toEqual([
      `gap:a@example.com:${fp}`,
      `gap:b@example.com:${fp}`,
    ]);
    expect(latest!.policy.operationBindings[operation]).toEqual([
      { app: "codex", fingerprint: fp },
    ]);
  });

  it("discards late previews from a previous Share even when transport ignores abort", async () => {
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
    next.rows.find((r) => r.operation === operation)!.providerLabel = "current";
    await act(async () => waiting.get("new")!(next));
    const old = preview(
      { policy: emptyCapabilityPolicy(), policies: {} },
      "old",
    );
    old.rows.find((r) => r.operation === operation)!.providerLabel = "late-old";
    await act(async () => waiting.get("old")!(old));
    expect(container.textContent).toContain("current");
    expect(container.textContent).not.toContain("late-old");
  });

  it("requires deliberate reauthorization of changed bindings and keeps newly added Apps disabled", async () => {
    liveFingerprint = "b".repeat(64);
    const ref = createRef<CapabilityReviewHandle>();
    const policy: ShareCapabilityPolicy = {
      ...emptyCapabilityPolicy(),
      allowedOperations: [operation],
      operationBindings: { [operation]: [{ app: "codex", fingerprint: fp }] },
    };
    const request = api.previewCapabilities.getMockImplementation()!;
    api.previewCapabilities.mockImplementation(async (id, input) => {
      const result = await request(id, input);
      result.rows.push({
        ...result.rows.find(
          (r: CapabilityPreview["rows"][number]) => r.operation === operation,
        )!,
        app: "claude",
        selected: false,
      });
      return result;
    });
    await act(async () =>
      root.render(
        <ShareCapabilitiesEditor
          shareId="share"
          revision={1}
          policy={policy}
          savedPolicy={policy}
          grants={{}}
          tokenLimit={-1}
          reviewRef={ref}
          onChange={vi.fn()}
        />,
      ),
    );
    await flush();
    let pending: Promise<CapabilityApproval | null>;
    await act(async () => {
      pending = ref.current!.review();
    });
    expect(document.body.textContent).toContain(
      "The account or execution binding changed",
    );
    expect(button("Confirm and save").disabled).toBe(true);
    await act(async () => checkbox("Authorize the current binding").click());
    await act(async () => button("Confirm and save").click());
    const approval = await pending!;
    expect(approval!.policy.operationBindings[operation]).toEqual([
      { app: "codex", fingerprint: liveFingerprint },
    ]);
  });

  it("cancels a pending save on scope changes and reports rebinding transport failures", async () => {
    const ref = createRef<CapabilityReviewHandle>();
    const policy: ShareCapabilityPolicy = {
      ...emptyCapabilityPolicy(),
      allowedOperations: [operation],
    };
    const props = {
      shareId: "share",
      revision: 1,
      policy,
      savedPolicy: emptyCapabilityPolicy(),
      grants,
      tokenLimit: -1,
      reviewRef: ref,
      onChange: vi.fn(),
    };
    await act(async () => root.render(<ShareCapabilitiesEditor {...props} />));
    await flush();
    let pending: Promise<CapabilityApproval | null>;
    await act(async () => {
      pending = ref.current!.review();
    });
    await act(async () =>
      root.render(<ShareCapabilitiesEditor {...props} tokenLimit={100} />),
    );
    expect(await pending!).toBeNull();
    expect(document.querySelector('[role="dialog"]')).toBeNull();
    liveFingerprint = "b".repeat(64);
    const oldBinding = {
      ...policy,
      operationBindings: { [operation]: [{ app: "codex", fingerprint: fp }] },
    };
    await act(async () =>
      root.render(<ShareCapabilitiesEditor {...props} policy={oldBinding} />),
    );
    await flush();
    api.previewCapabilities
      .mockImplementationOnce((id, input) =>
        Promise.resolve(preview(input, id)),
      )
      .mockRejectedValueOnce(new Error("rebind unavailable"));
    await act(async () => {
      await expect(ref.current!.review()).rejects.toThrow("rebind unavailable");
    });
    expect(container.textContent).toContain("rebind unavailable");
  });

  it("choosing a custom subset starts with the existing Share choices", async () => {
    let value: string[] | undefined;
    function Harness() {
      const [operations, setOperations] = useState<(typeof operation)[]>();
      value = operations;
      return (
        <ShareCapabilityPermissionFields
          value={operations}
          available={[operation]}
          onChange={setOperations}
        />
      );
    }
    await act(async () => root.render(<Harness />));
    const select = container.querySelector("select")!;
    await act(async () => {
      select.value = "custom";
      select.dispatchEvent(new Event("change", { bubbles: true }));
    });
    expect(value).toEqual([operation]);
    expect(checkbox("Web search").checked).toBe(true);
    await act(async () => checkbox("Web search").click());
    expect(value).toEqual([]);
    expect(select.value).toBe("disabled");
  });
});
