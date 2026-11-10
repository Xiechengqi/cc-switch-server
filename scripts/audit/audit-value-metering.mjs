#!/usr/bin/env node
// Static Registry/lifecycle coverage. This does not call any Provider.
import fs from "node:fs";
import assert from "node:assert/strict";
const read = path => fs.readFileSync(path, "utf8");
const registry = JSON.parse(read("assets/contract/provider-registry.json"));
const source = read("src/domain/usage/value_pricing.rs");
const strategySource = source.slice(source.indexOf("pub fn value_metering_strategy("), source.indexOf("pub fn supports_value_text_runtime("));
const strategy = new Map();
for (const match of strategySource.matchAll(/((?:\s*"[^"]+"\s*\|?)+)=>\s*\{?\s*Some\("([^"]+)"\)/g)) {
  for (const id of match[1].matchAll(/"([^"]+)"/g)) strategy.set(id[1], match[2]);
}
assert.equal(strategy.size, registry.drivers.length, "Every Driver requires a reviewed lifecycle strategy");
for (const driver of registry.drivers) assert(strategy.has(driver.driverId), driver.driverId);
const forwarder = read("src/proxy/forwarder.rs");
const lifetime = read("src/proxy/value_metering.rs");
const hooks = {
  http: ["admit_share_value_request(", "value_attempt_guard", "value_metering::response(response, value_attempt_guard)"],
  qoder: ["forward_qoder_metered", "value_metering::admit("],
  buddy: ["send_codebuddy_generation", "resource_request_plan(&prepared.payload"],
  trae: ["forward_trae_metered", "value_metering::admit("],
  kiro: ["forward_claude_kiro_inner_metered", "value_metering::admit("],
  web_session: ["forward_web_session_metered", "value_metering::admit("],
  deepseek: ["forward_claude_deepseek_metered", "start_deepseek_chat_completion_metered("],
  cursor: [],
  resolved_runtime: ["supports_value_text_runtime("],
};
for (const [name, needles] of Object.entries(hooks)) {
  for (const needle of needles) assert(forwarder.replace(/\s+/g, "").replace(/,\)/g, ")").includes(needle.replace(/\s+/g, "")), name + " missing " + needle);
}
for (const needle of ["begin_cursor_value_generation(", "value_request_body", ".observe(observed_usage", "series_response"]) {
  assert(read("src/proxy/cursor/agent_driver.rs").includes(needle), needle);
}
for (const needle of ["pub(crate) fn response(", "pub(crate) fn series_response(", "admit_prepared"]) assert(lifetime.includes(needle), needle);
const state = read("src/state.rs");
const deepseek = state.slice(state.indexOf("pub(crate) async fn start_deepseek_chat_completion_metered("), state.indexOf("async fn deepseek_runtime_generation_matches(")).replace(/\s+/g, "");
assert(deepseek.includes("meter.begin(self)"), "DeepSeek session rebuild must admit each generation");
assert(deepseek.indexOf("meter.begin(self)") < deepseek.indexOf(".completion("), "DeepSeek admission must precede generation IO");
for (const helper of ["send_qoder_generation", "send_trae_generation"]) {
  const start = forwarder.indexOf("async fn " + helper + "(");
  const end = forwarder.indexOf("\nasync fn ", start + 1);
  const source = forwarder.slice(start, end < 0 ? undefined : end);
  assert(source.includes("attempt.mark_sent()"), helper + " must own its actual send marker");
}
assert(read("src/proxy/web_session.rs").includes("attempt.mark_sent()"), "Web Session preparation must not mark generation sent");
for (const needle of ["admit_generation(", "value_async_tasks", "observe_task_in_tx"]) {
  assert((forwarder + read("src/repository/server_sqlite/value_quota.rs")).includes(needle), needle);
}
const pairs = registry.profiles.flatMap(profile => {
  const binding = profile.driverBinding;
  const drivers = binding.kind === "fixed" ? [binding.driverId] :
    registry.customPolicies.find(policy => policy.customPolicyId === binding.customPolicyId).allowedDriverIds;
  return drivers.map(driverId => ({
    profileId: profile.profileId, driverId, strategy: strategy.get(driverId),
    transports: driverId === "oauth.openai_codex" || driverId === "oauth.grok_responses" ? ["http", "sse", "websocket"] :
      driverId === "special.cursor" ? ["h2", "sse"] : ["http", "sse"],
    acceptance: "live_pending",
  }));
});
assert(pairs.every(pair => pair.strategy), "Uncovered Profile/Driver binding");
const document = JSON.stringify({ schemaVersion: 1, authority: "assets/contract/provider-registry.json",
  counts: { families: registry.families.length, profiles: registry.profiles.length, drivers: registry.drivers.length, pairs: pairs.length },
  pairs }, null, 2) + "\n";
const target = "assets/contract/value-metering-coverage.json";
if (process.argv.includes("--check")) assert.equal(read(target), document, "Regenerate value metering coverage after lifecycle review");
else fs.writeFileSync(target, document);
console.log("USD metering Registry coverage: " + pairs.length + " Profile/Driver pairs; live_pending");
