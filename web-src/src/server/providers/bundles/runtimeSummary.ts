import type { ProviderRuntimePlan } from "@/lib/api/providers";

export type RuntimeSummaryRowId =
  | "endpoint"
  | "protocol"
  | "driver"
  | "model"
  | "timeout"
  | "headers"
  | "region"
  | "state";

export interface RuntimeSummaryRow {
  id: RuntimeSummaryRowId;
  /** Already-formatted value; `null` marks "the server decides", not "empty". */
  value: string | null;
}

function seconds(ms: number | undefined): string | null {
  if (ms == null) return null;
  return `${Math.round(ms / 100) / 10}s`.replace(".0s", "s");
}

/**
 * The runtime plan is what the server will actually do with this Surface, and it is the
 * only place several derived values (resolved endpoint, resolved timeouts, header names)
 * can be seen at all. Raw JSON hid that behind a scroll box; these rows are the six or
 * seven lines anyone actually reads, with the JSON still one click away.
 */
export function runtimeSummaryRows(
  plan: ProviderRuntimePlan,
): RuntimeSummaryRow[] {
  const timeouts = [
    seconds(plan.transportPolicy.timeoutMs),
    seconds(plan.transportPolicy.streamFirstByteTimeoutMs),
    seconds(plan.transportPolicy.streamIdleTimeoutMs),
  ].filter((value): value is string => value != null);
  const headers = (plan.extraHeaders ?? []).map((header) => header.name);
  return [
    { id: "endpoint", value: plan.endpoint || null },
    { id: "protocol", value: plan.upstreamProtocol },
    { id: "driver", value: plan.driverId },
    {
      id: "model",
      value:
        plan.modelPolicy.mode === "single"
          ? plan.modelPolicy.upstreamModel
          : null,
    },
    { id: "timeout", value: timeouts.length ? timeouts.join(" / ") : null },
    { id: "headers", value: headers.length ? headers.join(", ") : null },
    { id: "region", value: plan.awsRegion ?? null },
    { id: "state", value: plan.configurationState },
  ];
}
