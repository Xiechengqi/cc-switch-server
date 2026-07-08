import { describe, expect, it } from "vitest";
import {
  formatShareLimitInput,
  isUnlimitedParallelLimit,
  isUnlimitedTokenLimit,
  normalizeShareLimitValue,
  UNLIMITED_LIMIT_SENTINEL,
  UNLIMITED_TOKEN_LIMIT,
} from "./shareUtils";
import { normalizeShareRecord } from "./shareRecordNormalize";

describe("shareUtils limits", () => {
  it("treats MAX_SAFE_INTEGER as unlimited", () => {
    expect(normalizeShareLimitValue(UNLIMITED_LIMIT_SENTINEL)).toBe(
      UNLIMITED_TOKEN_LIMIT,
    );
    expect(isUnlimitedTokenLimit(UNLIMITED_LIMIT_SENTINEL)).toBe(true);
    expect(isUnlimitedParallelLimit(UNLIMITED_LIMIT_SENTINEL)).toBe(true);
    expect(formatShareLimitInput(UNLIMITED_LIMIT_SENTINEL)).toBe("");
  });

  it("keeps finite limits unchanged", () => {
    expect(normalizeShareLimitValue(100_000)).toBe(100_000);
    expect(formatShareLimitInput(100_000)).toBe("100000");
  });

  it("normalizes share records with legacy unlimited sentinel", () => {
    const share = normalizeShareRecord({
      id: "share-1",
      tokenLimit: UNLIMITED_LIMIT_SENTINEL,
      parallelLimit: UNLIMITED_LIMIT_SENTINEL,
      bindings: { claude: "provider-1" },
    });

    expect(share?.tokenLimit).toBe(UNLIMITED_TOKEN_LIMIT);
    expect(share?.parallelLimit).toBe(UNLIMITED_TOKEN_LIMIT);
  });
});
