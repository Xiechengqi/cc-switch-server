import { describe, expect, it } from "vitest";

import { queryClient } from "./queryClient";

describe("queryClient defaults", () => {
  it("does not refetch every query on each focus event", () => {
    const defaults = queryClient.getDefaultOptions().queries;
    expect(defaults?.refetchOnWindowFocus).toBe(false);
    expect(defaults?.staleTime).toBe(30_000);
  });
});
