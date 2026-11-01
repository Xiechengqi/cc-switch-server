import { describe, expect, it } from "vitest";

import {
  serverStoreAuthorityKey,
  serverStorePhase,
  serverStoreShadowStale,
} from "./serverStore";
import type { ServerStoreActivationRequest } from "./server-legacy-api";

const pending: ServerStoreActivationRequest = {
  status: "pending",
  requestedAtMs: 1,
  requestedBy: "owner@example.test",
  failedAtMs: null,
  error: null,
};
const failed: ServerStoreActivationRequest = {
  ...pending,
  status: "failed",
  failedAtMs: 2,
  error: "shadow mismatch",
};

describe("server store authority", () => {
  it("offers the migration only while nothing is committed or requested", () => {
    expect(serverStorePhase({ authority: "legacy", activationRequest: null })).toBe("available");
    expect(serverStorePhase({ authority: "shadow_verified", activationRequest: null })).toBe("available");
    expect(serverStorePhase({ authority: "shadow_verified", activationRequest: pending })).toBe("pending");
    expect(serverStorePhase({ authority: "prepared", activationRequest: pending })).toBe("migrating");
    expect(serverStorePhase({ authority: "prepared", activationRequest: null })).toBe("migrating");
  });

  it("surfaces a failed request until it is retried or cancelled", () => {
    expect(serverStorePhase({ authority: "shadow_verified", activationRequest: failed })).toBe("failed");
    expect(serverStorePhase({ authority: "prepared", activationRequest: failed })).toBe("failed");
  });

  it("treats committed as final whatever request is left over", () => {
    expect(serverStorePhase({ authority: "committed", activationRequest: null })).toBe("committed");
    expect(serverStorePhase({ authority: "committed", activationRequest: failed })).toBe("committed");
    expect(serverStorePhase({ authority: "committed", activationRequest: pending })).toBe("committed");
  });

  it("labels every authority in plain words and unknown ones generically", () => {
    expect(serverStoreAuthorityKey("committed")).toBe("settings.serverStorage.authority.committed");
    expect(serverStoreAuthorityKey("shadow_verified")).toBe("settings.serverStorage.authority.shadow_verified");
    expect(serverStoreAuthorityKey("legacy")).toBe("settings.serverStorage.authority.legacy");
    expect(serverStoreAuthorityKey("prepared")).toBe("settings.serverStorage.authority.prepared");
    expect(serverStoreAuthorityKey("future")).toBe("settings.serverStorage.authority.unknown");
    expect(serverStoreAuthorityKey(undefined)).toBe("settings.serverStorage.authority.unknown");
  });

  it("notes a stale shadow only before the switch and only when known stale", () => {
    expect(serverStoreShadowStale({ authority: "shadow_verified", shadowCurrent: false })).toBe(true);
    expect(serverStoreShadowStale({ authority: "shadow_verified", shadowCurrent: true })).toBe(false);
    expect(serverStoreShadowStale({ authority: "legacy", shadowCurrent: null })).toBe(false);
    expect(serverStoreShadowStale({ authority: "committed", shadowCurrent: false })).toBe(false);
  });
});
