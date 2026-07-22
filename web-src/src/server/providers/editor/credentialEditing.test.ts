import { describe, expect, it } from "vitest";

import {
  credentialInputValue,
  updateCredentialInput,
  type CredentialEdit,
} from "./credentialEditing";

const configured: CredentialEdit = {
  slot: "/settingsConfig/apiKey",
  configured: true,
  action: "keep",
  value: "",
};

describe("direct credential editing", () => {
  it("shows the configured value without converting it into a replacement", () => {
    expect(credentialInputValue(configured, "current-secret")).toBe(
      "current-secret",
    );
    expect(configured.action).toBe("keep");
  });

  it("replaces a changed value and returns to keep when restored", () => {
    const changed = updateCredentialInput(configured, "replacement", {
      optional: false,
      revealedValue: "current-secret",
      revealStatus: "ready",
    });
    expect(changed).toMatchObject({ action: "replace", value: "replacement" });

    const restored = updateCredentialInput(changed, "current-secret", {
      optional: false,
      revealedValue: "current-secret",
      revealStatus: "ready",
    });
    expect(restored).toMatchObject({ action: "keep", value: "" });
  });

  it("clears an optional configured credential when its input is emptied", () => {
    expect(
      updateCredentialInput(configured, "", {
        optional: true,
        revealedValue: "current-secret",
        revealStatus: "ready",
      }),
    ).toMatchObject({ action: "clear", value: "" });
  });
});
