import { act, useState } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import i18n from "@/i18n";
import { parseGrantPreview, type GrantPreview, type GrantPreviewInput } from "@/lib/grant-capability-preview";
import contract from "../../../../tests/fixtures/share-grant-capabilities-v14.json";
import type { ShareOperation } from "@/lib/share-capabilities";
import { GrantCapabilityFields } from "./GrantCapabilityFields";

const mock = vi.hoisted(() => ({ preview: vi.fn() }));
vi.mock("@/lib/api/share", () => ({ shareApi: { previewGrantCapabilities: mock.preview } }));
Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });

const search = "tool.web_search";
let container: HTMLDivElement;
let root: Root;
beforeEach(async () => {
  mock.preview.mockReset();
  await i18n.changeLanguage("en");
  vi.useFakeTimers();
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
});
afterEach(async () => {
  await act(async () => root.unmount());
  container.remove();
  vi.useRealTimers();
});

function reply(rows: GrantPreview["rows"], unavailable = false): GrantPreview {
  return { displayVersion: 3, shareId: "share", configRevision: 7, subject: "alice@example.com", ready: true, unavailable, rows };
}

it("reads the shared v14 grant preview and rejects a different subject", () => {
  const candidate = parseGrantPreview(contract.candidate, "share-v14", contract.candidateRequest);
  expect(candidate.rows[0].needsReconfirm).toBe(true);
  expect(candidate.rows[1].available).toBe(false);
  expect(() => parseGrantPreview(contract.candidate, "share-v14", { ...contract.candidateRequest, subject: "bob@example.com" })).toThrow();
  const review = parseGrantPreview(contract.review, "share-v14", contract.reviewRequest as GrantPreviewInput);
  expect(review.requiredAcknowledgements?.map(row => row.scope)).toEqual(["alice@example.com"]);
  expect(contract.reviewRequest.grantCapabilityEdits["bob@example.com"].operations).toEqual([]);
});

function Fixture({ initial = [] }: { initial?: ShareOperation[] }) {
  const [operations, setOperations] = useState(initial);
  const [reconfirm, setReconfirm] = useState<ShareOperation[]>([]);
  return <GrantCapabilityFields shareId="share" revision={7} email="alice@example.com" operations={operations} reconfirm={reconfirm}
    quota="tokens" onChange={setOperations} onReconfirm={operation => setReconfirm([operation])} />;
}

async function render(initial?: ShareOperation[]) {
  await act(async () => root.render(<Fixture initial={initial} />));
  await act(async () => vi.advanceTimersByTimeAsync(200));
}

it("edits only current supported operations and shows the Token cost warning", async () => {
  mock.preview.mockResolvedValue(reply([{ operation: search, available: true, selected: false, needsReconfirm: false }]));
  await render();
  const button = container.querySelector('button[role="switch"]') as HTMLButtonElement;
  expect(button.getAttribute("aria-checked")).toBe("false");
  await act(async () => button.click());
  expect(button.getAttribute("aria-checked")).toBe("true");
  expect(container.textContent).toContain("Token limits do not cover media quantities or tool calls.");
  expect(mock.preview.mock.calls[0][1]).toMatchObject({ subject: "alice@example.com", expectedConfigRevision: 7 });
});

it("distinguishes a failed read from a verified empty account and can retry", async () => {
  mock.preview.mockRejectedValueOnce(new Error("offline")).mockResolvedValueOnce(reply([]));
  await render();
  const retry = [...container.querySelectorAll("button")].find(button => /retry/i.test(button.textContent ?? ""))!;
  expect(retry).toBeTruthy();
  await act(async () => retry.click());
  await act(async () => vi.advanceTimersByTimeAsync(200));
  expect(container.querySelector("fieldset")).toBeNull();
});

it("retains a disabled binding for removal without allowing it back on", async () => {
  mock.preview.mockResolvedValue(reply([{ operation: search, available: false, selected: true, needsReconfirm: false }]));
  await render([search]);
  const button = container.querySelector('button[role="switch"]') as HTMLButtonElement;
  expect(container.textContent).toContain("You can remove permission");
  expect(button.getAttribute("aria-checked")).toBe("true");
  await act(async () => button.click());
  expect(button.getAttribute("aria-checked")).toBe("false");
  expect(button.disabled).toBe(true);
});

it("allows removing saved permissions when the candidate lookup is unreachable", async () => {
  mock.preview.mockRejectedValue(new Error("offline"));
  await render([search]);
  const button = container.querySelector<HTMLButtonElement>('button[role="switch"]')!;
  expect(button.disabled).toBe(false);
  await act(async () => button.click());
  expect(container.querySelector('button[role="switch"]')).toBeNull();
  expect(container.textContent).toContain("Available operations could not be confirmed.");
});
