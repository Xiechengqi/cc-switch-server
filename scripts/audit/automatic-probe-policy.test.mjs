import assert from "node:assert/strict";
import fs from "node:fs";
import test from "node:test";

const read = (name) => fs.readFileSync(new URL(`../../${name}`, import.meta.url), "utf8");

test("Server and Web automatic probes retain the fixed Codex, Claude, Gemini order", () => {
  assert.match(read("src/domain/providers/probe.rs"), /AUTOMATIC_PROBE_APP_ORDER[^=]*=\s*\[AppKind::Codex,\s*AppKind::Claude,\s*AppKind::Gemini\]/);
  assert.match(read("web-src/src/server/providers/bundles/bundleDraft.ts"), /BUNDLE_TEST_APP_ORDER[^=]*=\s*\[\s*"codex",\s*"claude",\s*"gemini",?\s*\]/);
});

test("the editor shows automatic selection without a manual App control", () => {
  const editor = read("web-src/src/server/providers/bundles/ProviderBundleEditor.tsx");
  assert.match(editor, /automaticBundleProbeApp\(draft\.surfaces\)/);
  assert.match(editor, /providerBundle\.automaticProbeHint/);
  assert.doesNotMatch(editor, /(?:Tabs|Select)[^>]*value=\{draft\.testApp\}/);
  const bundle = read("src/domain/providers/bundle.rs");
  assert.match(bundle, /default = "default_legacy_test_app"/);
  assert.match(bundle, /self\.automatic_test_app\(\)/);
});

test("automatic scheduling and batch acceptance share the Share-scoped selector", () => {
  const scheduler = read("src/api/provider_health_scheduler.rs");
  const control = read("src/api/control/share_router.rs");
  assert.match(scheduler, /automatic_share_probe\(share, providers\)/);
  assert.match(scheduler, /current_health_target\(state, &target\.provider\)/);
  assert.match(control, /automatic_share_probe\(/);
  assert.match(scheduler, /local_result_for_router_cycle/);
  assert.match(scheduler, /log\.data_source\.as_deref\(\) == Some\(SCHEDULED_SOURCE\)/);
  assert.match(scheduler, /in_slot\(log\.created_at_ms\)/);
});
