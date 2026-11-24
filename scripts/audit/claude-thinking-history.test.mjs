import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("../..", import.meta.url));
const fixture = JSON.parse(fs.readFileSync(path.join(root, "assets/contract/claude-thinking-history.json"), "utf8"));
const policy = {
  target: "official_claude_oauth_only",
  remove: "assistant_top_level_unsigned_blank_thinking_only",
  emptyTurn: "reject_without_mutating_history_or_fabricating_content",
  signedThinking: "preserve_opaque_bytes_including_empty_text",
  redactedThinking: "preserve_opaque_data",
  businessPayload: "never_recurse_into_tool_input_or_tool_result",
  thinkingControls: "preserve_adaptive_display_effort_and_context_management",
  countTokens: "same_history_repair_with_independent_generation_and_beta_rules",
  cch: "sign_after_history_repair",
  upstreamRejection: "never_enter_destructive_signature_recovery_for_empty_thinking",
  accountModelFallback: false,
  newReplayStorage: false,
};

function validate(document) {
  assert.equal(document.schemaVersion, 1);
  assert.equal(document.kind, "synthetic_server_contract");
  assert.equal(document.providerType, "claude_oauth");
  assert.equal(document.liveState, "live_pending");
  assert.deepEqual(document.policy, policy);
  assert.equal(document.reference.repository, "CLIProxyAPI");
  assert.equal(document.reference.commit, "85225381d89915595598ed13766cd79890c03a84");
  assert.equal(document.reference.tree, "f73c19829760e4d05be86c7d91997555d2df88e4");
  assert.equal(document.reference.use, "read_only_research_not_a_build_test_or_runtime_input");
  assert.deepEqual(document.reference.files, [
    { path: "internal/runtime/executor/claude_executor.go", sha256: "90c0e8e564ae9d80b9e1077c030e7fd6ffd5b4b0cd7f0405af8875106e30afb9" },
    { path: "internal/signature/provider_compatibility.go", sha256: "b7774611f0fc70f26eb71c615566af8e0f209e85298975abc35f9164a134a0df" },
  ]);
  const request = document.request;
  assert.equal(request.model, "claude-opus-5-5");
  assert.deepEqual(request.thinking, { type: "adaptive", display: "updates" });
  assert.deepEqual(request.output_config, { effort: "high" });
  const content = request.messages[1].content;
  assert.deepEqual(content[0], { type: "thinking", thinking: " ", signature: "" });
  assert.deepEqual(request.messages[1].output_config, { effort: "low" });
  assert.equal(content.length, 5);
  assert(content.slice(1).every((block) => block.type === "tool_use"));
  assert.equal(new Set(content.slice(1).map((block) => block.id)).size, 4);
  assert.deepEqual(request.messages[2].content.map((block) => block.tool_use_id), content.slice(1).map((block) => block.id));
  assert.deepEqual(content[1].input.nested, content[0]);
  assert.deepEqual(JSON.parse(request.messages[2].content[0].content[0].text), content[0]);
  assert.equal(document.preservedBlocks.length, 3);
  assert(document.preservedBlocks.slice(0, 2).every((block) => block.type === "thinking" && block.thinking.trim() === "" && block.signature.length > 0));
  assert.equal(document.preservedBlocks[2].type, "redacted_thinking");
  assert.equal(document.response.stop_reason, "end_turn");
  assert(document.response.usage.input_tokens > 0 && document.response.usage.output_tokens > 0);
  assert.equal(document.tests.length, 4);
}

test("Claude unsigned blank history contract preserves native opaque blocks and four parallel tools", () => {
  validate(fixture);
  for (const evidence of fixture.tests) {
    assert(!path.isAbsolute(evidence.path) && !evidence.path.split("/").includes(".."));
    const source = fs.readFileSync(path.join(root, evidence.path), "utf8");
    assert(source.includes(`fn ${evidence.anchor}(`), `missing local regression ${evidence.anchor}`);
  }
});

test("Claude unsigned blank history evidence cannot widen cleanup or promote live acceptance", () => {
  for (const mutate of [
    (document) => { document.liveState = "live_verified"; },
    (document) => { document.policy.remove = "all_thinking"; },
    (document) => { document.policy.target = "all_anthropic_compatible_providers"; },
    (document) => { document.policy.accountModelFallback = true; },
    (document) => { document.policy.newReplayStorage = true; },
    (document) => { document.request.messages[1].content.pop(); },
    (document) => { document.preservedBlocks[0].signature = ""; },
    (document) => { document.reference.files[0].sha256 = "a".repeat(64); },
  ]) {
    const changed = structuredClone(fixture);
    mutate(changed);
    assert.throws(() => validate(changed));
  }
});
