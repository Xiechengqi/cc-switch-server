import { describe, expect, it } from "vitest";

import { getIcon, hasIcon, iconList } from "@/icons/extracted";
import { searchIcons } from "@/icons/extracted/metadata";
import { resolveBundleIcon } from "./bundleIcon";

describe("Custom Provider icon", () => {
  it("registers a neutral inline icon that can also be selected and searched", () => {
    expect(hasIcon("custom")).toBe(true);
    expect(getIcon("custom")).toContain("currentColor");
    expect(iconList).toContain("custom");
    expect(searchIcons("Custom Provider")).toContain("custom");
  });

  it("uses Custom for missing icons without rewriting explicit existing icons", () => {
    for (const icon of [undefined, null, "", " "]) {
      expect(resolveBundleIcon("family.custom_http", icon)).toBe("custom");
    }
    for (const icon of ["anthropic", "claude", "openai", "deepseek", "custom"]) {
      expect(resolveBundleIcon("family.custom_http", icon)).toBe(icon);
    }
  });

  it("uses the shared catalog for all family defaults", () => {
    expect(resolveBundleIcon("family.workbuddy_oauth")).toBe("workbuddy");
    expect(resolveBundleIcon("family.codebuddy_oauth")).toBe("codebuddy");
    expect(resolveBundleIcon("family.openai_oauth")).toBe("openai");
    expect(resolveBundleIcon("family.minimax_cn")).toBe("minimax");
    expect(resolveBundleIcon("family.not_known")).toBeUndefined();
  });
});
