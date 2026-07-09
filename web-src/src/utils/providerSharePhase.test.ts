import { describe, expect, it, beforeEach } from "vitest";
import {
  getProviderCardShareDisplayStatus,
  getProviderSharePhase,
  isShareRunning,
} from "@/utils/shareUtils";
import {
  isProviderShareDeleteConfirmSkipped,
  setProviderShareDeleteConfirmSkipped,
} from "@/utils/providerShareDeleteConfirm";

describe("isShareRunning", () => {
  it("requires active status and tunnel endpoint", () => {
    expect(
      isShareRunning({
        status: "active",
        tunnelUrl: "https://example.cc-switch.com",
      }),
    ).toBe(true);
    expect(
      isShareRunning({
        status: "paused",
        tunnelUrl: "https://example.cc-switch.com",
      }),
    ).toBe(false);
    expect(isShareRunning({ status: "active", tunnelUrl: null })).toBe(false);
  });
});

describe("getProviderCardShareDisplayStatus", () => {
  it("maps share records to compact card statuses", () => {
    expect(
      getProviderCardShareDisplayStatus({
        status: "active",
        tunnelUrl: "https://example.cc-switch.com",
      }),
    ).toBe("sharing");
    expect(
      getProviderCardShareDisplayStatus({
        status: "active",
        tunnelUrl: null,
      }),
    ).toBe("closed");
    expect(
      getProviderCardShareDisplayStatus({
        status: "paused",
        tunnelUrl: "https://example.cc-switch.com",
      }),
    ).toBe("closed");
    expect(getProviderCardShareDisplayStatus({ status: "expired" })).toBe(
      "expired",
    );
    expect(getProviderCardShareDisplayStatus({ status: "exhausted" })).toBe(
      "exhausted",
    );
  });
});

describe("getProviderSharePhase", () => {
  it("maps share records to card phases", () => {
    expect(getProviderSharePhase(null)).toBe("not_created");
    expect(
      getProviderSharePhase({
        status: "active",
        tunnelUrl: "https://example.cc-switch.com",
      } as never),
    ).toBe("sharing");
    expect(
      getProviderSharePhase({
        status: "paused",
        tunnelUrl: "https://example.cc-switch.com",
      } as never),
    ).toBe("stopped");
  });
});

describe("providerShareDeleteConfirm", () => {
  beforeEach(() => {
    window.localStorage.clear();
  });

  it("persists skip-confirm preference", () => {
    expect(isProviderShareDeleteConfirmSkipped()).toBe(false);
    setProviderShareDeleteConfirmSkipped(true);
    expect(isProviderShareDeleteConfirmSkipped()).toBe(true);
    setProviderShareDeleteConfirmSkipped(false);
    expect(isProviderShareDeleteConfirmSkipped()).toBe(false);
  });
});
