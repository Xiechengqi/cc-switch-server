import * as React from "react";
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";

import { EmailTagsInput } from "./tags-input";

describe("EmailTagsInput", () => {
  const roots: Array<ReturnType<typeof createRoot>> = [];

  afterEach(() => {
    for (const root of roots.splice(0)) {
      act(() => root.unmount());
    }
    document.body.replaceChildren();
  });

  it("hides an example placeholder while focused and restores it on blur", () => {
    const container = document.createElement("div");
    document.body.append(container);
    const root = createRoot(container);
    roots.push(root);

    act(() => {
      root.render(
        <EmailTagsInput
          value={[]}
          onChange={() => undefined}
          placeholder="a@example.com, b@example.com"
          hidePlaceholderOnFocus
        />,
      );
    });

    const input = container.querySelector("input");
    expect(input?.placeholder).toBe("a@example.com, b@example.com");

    act(() => input?.focus());
    expect(input?.placeholder).toBe("");

    act(() => input?.blur());
    expect(input?.placeholder).toBe("a@example.com, b@example.com");
  });

  it("splits and deduplicates multiple pasted email addresses", () => {
    const container = document.createElement("div");
    document.body.append(container);
    const root = createRoot(container);
    roots.push(root);
    const onChange = vi.fn();

    act(() => {
      root.render(
        <EmailTagsInput value={["existing@example.com"]} onChange={onChange} />,
      );
    });

    const input = container.querySelector("input");
    const paste = new Event("paste", { bubbles: true, cancelable: true });
    Object.defineProperty(paste, "clipboardData", {
      value: {
        getData: () =>
          "Alice@example.com, bob@example.com; existing@example.com\ncarol@example.com",
      },
    });

    act(() => input?.dispatchEvent(paste));

    expect(paste.defaultPrevented).toBe(true);
    expect(onChange).toHaveBeenCalledWith([
      "existing@example.com",
      "alice@example.com",
      "bob@example.com",
      "carol@example.com",
    ]);
  });
});
