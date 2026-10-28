import assert from "node:assert/strict";

const digest = /^[a-f0-9]{64}$/;

function distribution(value, count, label) {
  assert(value && Number.isSafeInteger(value.count) && value.count > 0, `${label}: missing samples`);
  if (count !== undefined) assert.equal(value.count, count, `${label}: sample count`);
  const ordered = ["min", "p50", "p95", "p99", "max"].map((key) => value[key]);
  assert(ordered.every((number) => Number.isFinite(number) && number >= 0), `${label}: invalid duration/bytes`);
  assert(ordered.every((number, index) => index === 0 || number >= ordered[index - 1]), `${label}: unordered percentiles`);
}

// This validates a frozen local measurement, never vendor or deployment acceptance.
export function auditCodexServerProbe(report, codecProbe) {
  assert.equal(report.format, "cc-switch-codex-server-loopback-probe");
  assert.equal(report.schemaVersion, 1);
  assert.equal(report.evidenceState, "offline_loopback_test_assembly_measurement");
  assert.equal(report.targetState, "working_tree");
  assert(/^[a-f0-9]{40}$/.test(report.targetHeadCommit));
  assert(Number.isFinite(Date.parse(report.capturedAt)));
  assert(digest.test(report.binarySha256) && digest.test(report.rustSourceTreeSha256));
  assert.equal(report.frozenRustSourcesUnchanged, true);
  assert.equal(report.probeSourcesUnchanged, true);
  assert.equal(report.environment?.explicitOutboundProxyDisabled, true);
  assert.equal(report.environment?.processMemoryBudgetMb, "0");
  assert(report.applicationByteScope?.includes("precedes the WS response.create envelope"));
  assert(Number.isSafeInteger(report.rustSourceFileCount) && report.rustSourceFileCount > 0);
  assert.equal(report.listenersClosed, true);
  assert.equal(report.inputDirectoryRemoved, true);
  assert.equal(report.workerThreads, 4);
  assert(report.profile.includes("debug unoptimized"));
  assert(report.rssScope.includes("entire Linux test process") && report.rssScope.includes("allocator retention"));
  assert(report.limitations.includes("no baseline implementation speedup claim"));
  assert(report.limitations.includes("not vendor acceptance, deployment or production capacity evidence"));
  assert(report.limitations.includes("no allocation-count instrumentation; logical budget bytes are not allocations or RSS"));
  assert(report.limitations.includes("no new outbound, downstream or WS compression enabled; bytes exclude transport framing, headers and TLS"));

  const cases = ["short", "long_history", "tool_history", "base64_entropy"];
  assert.deepEqual(Object.keys(report.fixtureInputs).sort(), [...cases].sort());
  for (const name of cases) {
    const input = report.fixtureInputs[name];
    const codec = codecProbe.results.find((row) => row.case === name);
    assert(codec, `codec input missing: ${name}`);
    assert.equal(input.sha256, codec.inputSha256, `same input digest: ${name}`);
    assert.equal(input.bytes, codec.plainBodyBytes, `same input bytes: ${name}`);
  }

  const remaining = new Set();
  for (const path of ["native_ws", "fallback_ws", "http_sse", "http_json"]) {
    for (const name of cases) remaining.add(`${path}/${name}`);
    if (path !== "http_json") {
      remaining.add(`${path}/cancel`);
      remaining.add(`${path}/slow_reader`);
    }
  }
  assert.equal(report.results?.length, remaining.size);
  for (const row of report.results) {
    const label = `${row.path}/${row.case}`;
    assert(remaining.delete(label), `duplicate/unknown row: ${label}`);
    assert(Number.isSafeInteger(row.samples) && row.samples >= (row.case === "slow_reader" ? 31 : 101), `${label}: insufficient measured samples`);
    assert.equal(row.warmup, 5, `${label}: warmup excluded`);
    assert(digest.test(row.preparedInputSha256));
    assert.equal(row.upstreamGenerations, row.samples, `${label}: upstream replay`);
    assert(row.preparedInputBytes > 0 && row.upstreamApplicationRequestBytes > 0);
    for (const field of ["clientFirstArrivalSeconds", "clientCompletionOrDropSeconds"]) {
      distribution(row[field], row.samples, `${label}/${field}`);
    }
    if (row.case === "cancel") {
      distribution(row.clientReturnToObservedOwnerReleaseSeconds, row.samples, `${label}/cancel release`);
    } else {
      assert.equal(row.clientReturnToObservedOwnerReleaseSeconds, null);
    }
    for (const field of ["processCpuSeconds", "wallSeconds", "rssBeforeBytes", "rssAfterBytes", "rssSampledPeakBytes",
      "logicalUsedBeforeBytes", "logicalUsedAfterBytes", "logicalSampledPeakBytes", "processLogicalCumulativeHighWaterBytes"]) {
      assert(Number.isFinite(row[field]) && row[field] >= 0, `${label}/${field}`);
    }
    assert(row.rssSampledPeakBytes >= row.rssBeforeBytes && row.rssBeforeBytes > 0);
    assert.equal(row.processLogicalRejections, 0);
    const metrics = Object.entries(row.serverMetrics);
    for (const [key, values] of metrics) distribution(values, undefined, `${label}/${key}`);
    const find = (name, fragment) => metrics.filter(([key]) => key.startsWith(`${name}{`) && key.includes(fragment));
    const required = (name, fragment, count = row.samples) => {
      const selected = find(name, fragment).filter(([key]) =>
        name === "cc_switch_request_memory_high_water_bytes" || key.includes("scope=request,"));
      assert.equal(selected.length, 1, `${label}: missing/ambiguous ${name}/${fragment}`);
      distribution(selected[0][1], count === null ? undefined : count, `${label}/${name}/${fragment}`);
    };
    required("cc_switch_forward_milestone_seconds", "phase=owner_released,");
    required("cc_switch_request_memory_high_water_bytes", "outcome=");
    if (row.path.endsWith("ws")) {
      required("cc_switch_forward_milestone_seconds", "phase=downstream_write_completed,");
      required("cc_switch_forward_wait_seconds", "phase=downstream_write,", null);
    }
    const results = find("cc_switch_forward_duration_seconds", "scope=request,");
    assert.equal(results.length, 1, `${label}: mixed request results`);
    assert(results[0][0].includes(`result=${row.case === "cancel" ? "cancelled" : "complete"},`), `${label}: wrong request result`);
    distribution(results[0][1], row.samples, `${label}/result`);
    // HTTP cancellation may occur at bootstrap bytes, before semantic content.
    if (row.case !== "cancel" || row.path.endsWith("ws")) {
      const semantic = find("cc_switch_forward_milestone_seconds", "phase=first_semantic,")
        .filter(([key]) => key.includes("scope=request,"));
      assert.equal(semantic.length, 1, `${label}: missing semantic content`);
      distribution(semantic[0][1], row.samples, `${label}/first semantic`);
    }
    if (row.case === "cancel") required("cc_switch_forward_wait_seconds", "phase=cancel_drain,");
  }
  assert.equal(remaining.size, 0);
}
