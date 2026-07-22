import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";

import { ProviderPresetSelector } from "./ProviderPresetSelector";

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string) => key,
  }),
}));

describe("ProviderPresetSelector", () => {
  it("renders without a react-hook-form provider", () => {
    const html = renderToStaticMarkup(
      <ProviderPresetSelector
        selectedPresetId={null}
        presetEntries={[]}
        presetCategoryLabels={{}}
        onPresetChange={() => undefined}
      />,
    );

    expect(html).toContain("providerPreset.label");
    expect(html).toContain("providerPreset.custom");
  });
});
