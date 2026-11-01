import { describe, expect, it } from "vitest";

import {
  clientWebTerminalUrl,
  isEmbeddedServerView,
  preferredServerView,
  requestedServerView,
  requestedSettingsSection,
  resolveInitialServerView,
  SERVER_SETTINGS_SECTIONS,
  SERVER_STORAGE_SETTINGS_SECTION,
  serverSettingsSectionUrl,
  storedServerView,
} from "./serverView";

describe("serverView", () => {
  it("reads the requested view from the query string", () => {
    expect(requestedServerView("?view=terminal")).toBe("terminal");
    expect(requestedServerView("view=shares")).toBe("shares");
    expect(requestedServerView("?view=unknown")).toBeNull();
    expect(requestedServerView("")).toBeNull();
  });

  it("falls back when terminal is disabled", () => {
    expect(preferredServerView(false, "terminal")).toBe("providers");
    expect(preferredServerView(true, "terminal")).toBe("terminal");
    expect(storedServerView("terminal", false)).toBe("providers");
    expect(storedServerView("shares", true)).toBe("shares");
  });

  it("prefers the query view over local storage", () => {
    expect(
      resolveInitialServerView(true, "?view=terminal", "providers"),
    ).toBe("terminal");
    expect(resolveInitialServerView(true, "", "shares")).toBe("shares");
    expect(resolveInitialServerView(false, "?view=terminal", "terminal")).toBe(
      "providers",
    );
  });

  it("detects an embedded visit", () => {
    expect(isEmbeddedServerView("?view=terminal&embed=1")).toBe(true);
    expect(isEmbeddedServerView("embed=true")).toBe(true);
    expect(isEmbeddedServerView("?view=terminal")).toBe(false);
    expect(isEmbeddedServerView("?embed=0")).toBe(false);
    expect(isEmbeddedServerView("")).toBe(false);
  });

  it("reads a settings section deep link only for known sections", () => {
    expect(requestedSettingsSection("?view=settings&section=server-storage")).toBe(
      "server-storage",
    );
    expect(requestedSettingsSection("view=settings&section=server-storage")).toBe(
      "server-storage",
    );
    expect(requestedSettingsSection("?view=shares&section=server-storage")).toBeNull();
    expect(requestedSettingsSection("?view=settings&section=nope")).toBeNull();
    expect(requestedSettingsSection("?view=settings&section=toString")).toBeNull();
    expect(requestedSettingsSection("?view=settings")).toBeNull();
    expect(requestedSettingsSection("")).toBeNull();
    expect(SERVER_SETTINGS_SECTIONS[SERVER_STORAGE_SETTINGS_SECTION]).toBe("general");
  });

  it("links to a settings section on the same page", () => {
    expect(
      serverSettingsSectionUrl(
        "server-storage",
        "https://alpha.example.com/admin/?view=providers&embed=1#top",
      ),
    ).toBe("https://alpha.example.com/admin/?view=settings&section=server-storage");
    expect(
      serverSettingsSectionUrl("server-storage", "https://alpha.example.com/?foo=1"),
    ).toBe("https://alpha.example.com/?foo=1&view=settings&section=server-storage");
    expect(serverSettingsSectionUrl("server-storage", "not a url")).toBe(
      "?view=settings&section=server-storage",
    );
  });

  it("appends the terminal view to a client URL", () => {
    expect(clientWebTerminalUrl("https://alpha.example.com")).toBe(
      "https://alpha.example.com/?view=terminal&embed=1",
    );
    expect(clientWebTerminalUrl("https://alpha.example.com/")).toBe(
      "https://alpha.example.com/?view=terminal&embed=1",
    );
    expect(clientWebTerminalUrl("https://alpha.example.com/?foo=1")).toBe(
      "https://alpha.example.com/?foo=1&view=terminal&embed=1",
    );
  });
});
