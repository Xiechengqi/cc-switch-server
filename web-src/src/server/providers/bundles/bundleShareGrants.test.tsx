import { describe, expect, it } from "vitest";
import { act, useState } from "react";
import { createRoot } from "react-dom/client";
import type { ShareUserGrantMap } from "@/lib/api/share";
import { ShareUserGrantsEditor } from "@/components/providers/ShareUserGrantsEditor";
import {
  bundleShareGrantHandlers,
  createBundleShareDraft,
  type ProviderBundleShareDraft,
} from "./bundleShare";
import i18n from "@/i18n";

const GRANT: ShareUserGrantMap = {
  "friend@example.com": {
    email: "friend@example.com",
    role: "shareto",
    active: true,
    policy: { tokenPeriod: "lifetime" },
  },
};

describe("bundleShareGrantHandlers", () => {
  it("keeps the grant change when the usage-edit change lands in the same tick", () => {
    let draft = createBundleShareDraft();
    const handlers = bundleShareGrantHandlers((apply) => {
      draft = apply(draft);
    });
    // ShareUserGrantsEditor fires both callbacks from one click; the second one
    // must not restore the grant map captured before the first.
    handlers.onChange(GRANT);
    handlers.onUsageEditsChange({});
    expect(Object.keys(draft.userGrants)).toEqual(["friend@example.com"]);
  });
});

function findButton(label: string): HTMLButtonElement {
  const button = Array.from(
    document.querySelectorAll<HTMLButtonElement>("button"),
  ).find((candidate) => candidate.textContent?.includes(label));
  if (!button) throw new Error(`button not found: ${label}`);
  return button;
}

function typeInto(input: HTMLInputElement, value: string) {
  const setter = Object.getOwnPropertyDescriptor(
    window.HTMLInputElement.prototype,
    "value",
  )?.set;
  setter?.call(input, value);
  input.dispatchEvent(new Event("input", { bubbles: true }));
}

describe("Provider Bundle share user grants", () => {
  it("adds an authorized user to a brand-new Bundle Share draft", async () => {
    const container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);
    let latest: ProviderBundleShareDraft = createBundleShareDraft();

    function Harness() {
      const [draft, setDraft] = useState<ProviderBundleShareDraft>(() =>
        createBundleShareDraft(),
      );
      latest = draft;
      const handlers = bundleShareGrantHandlers(setDraft);
      return (
        <ShareUserGrantsEditor
          value={draft.userGrants}
          ownerEmail="owner@example.com"
          defaultPolicy={{ tokenPeriod: "lifetime" }}
          usageEdits={draft.userUsageEdits}
          onUsageEditsChange={handlers.onUsageEditsChange}
          onChange={handlers.onChange}
        />
      );
    }

    await act(async () => {
      root.render(<Harness />);
    });
    await act(async () => {
      findButton(i18n.t("share.userLimit.add")).click();
    });
    const email = document.querySelector<HTMLInputElement>("#share-user-email");
    expect(email).not.toBeNull();
    await act(async () => {
      typeInto(email!, "friend@example.com");
    });
    await act(async () => {
      findButton(i18n.t("common.save")).click();
    });

    expect(Object.keys(latest.userGrants).sort()).toEqual([
      "friend@example.com",
      "owner@example.com",
    ]);
    expect(latest.userGrants["friend@example.com"].role).toBe("shareto");

    await act(async () => {
      root.unmount();
    });
    container.remove();
  });
});
