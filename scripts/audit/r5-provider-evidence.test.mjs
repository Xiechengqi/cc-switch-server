import assert from "node:assert/strict";
import test from "node:test";
import fs from "node:fs";
import { auditR5Evidence, loadR5Evidence, auditStaticProfileProjection, auditSharedCoreProjection } from "./audit-r5-provider-evidence.mjs";

test("Antigravity R5 evidence keeps both OAuth rails and conditional paid/pool boundaries", () => {
  const document = loadR5Evidence("antigravity_agy");
  assert.deepEqual(auditR5Evidence(document), { providerClass: "antigravity_agy", cases: 10, liveState: "live_pending" });
  for (const mutate of [
    (doc) => { doc.scope.apps.push("codex"); },
    (doc) => { doc.localPolicy.transport.poolEnabledByDefault = true; },
    (doc) => { doc.localPolicy.transport.performanceClaim = true; },
    (doc) => { doc.localPolicy.paidCredits = "unknown balance enables paid credits"; },
    (doc) => { doc.localPolicy.terminal = "EOF always completes"; },
    (doc) => { doc.scope.credentialRails = ["interchangeable OAuth pool"]; },
    (doc) => { doc.reference.realReceipt = true; },
  ]) {
    const changed = structuredClone(document);
    mutate(changed);
    assert.throws(() => auditR5Evidence(changed));
  }
});

test("Gemini R5 evidence keeps exact product and onboarding gate", () => {
  const document = loadR5Evidence("gemini_cli");
  assert.deepEqual(auditR5Evidence(document), { providerClass:"gemini_cli", cases:6, liveState:"live_pending" });
  assert.equal(document.localPolicy.onboardingEnabled, false);
});
test("Gemini R5 evidence rejects borrowed client rights, tier selection and false live claims", () => {
  for (const mutate of [
    (doc) => { doc.localPolicy.onboardingEnabled = true; },
    (doc) => { doc.scope.credentialRail = "antigravity interchangeable"; },
    (doc) => { doc.cases[1].project = "guessed"; },
    (doc) => { doc.reference.commit = "0".repeat(40); },
    (doc) => { doc.liveState = "live_verified"; },
    (doc) => { doc.reference.realReceipt = true; },
  ]) {
    const document = loadR5Evidence("gemini_cli");
    mutate(document);
    assert.throws(() => auditR5Evidence(document));
  }
});


test("Kimi R5 evidence freezes owner lifetimes, Chat schema limits and exact model scope", () => {
  const document = loadR5Evidence("kimi_code");
  assert.deepEqual(auditR5Evidence(document), { providerClass:"kimi_code", cases:10, liveState:"live_pending" });
  for (const mutate of [
    (doc) => { doc.localPolicy.replay.owner = "release when index is deleted"; },
    (doc) => { doc.localPolicy.catalogs.refusal = "reuse stale authorization"; },
    (doc) => { doc.localPolicy.schema.scope = "also rewrite native Claude"; },
    (doc) => { doc.localPolicy.schema.refusal = "degrade cycles to weak hints"; },
    (doc) => { doc.localPolicy.schema.maxOutputBytes *= 2; },
    (doc) => { doc.scope.site = "kimi.ai"; },
    (doc) => { doc.reference.realReceipt = true; },
  ]) {
    const changed = structuredClone(document);
    mutate(changed);
    assert.throws(() => auditR5Evidence(changed));
  }
});

test("Ollama R5 evidence keeps explicit effort, exact model and Cloud capability gates", () => {
  const document = loadR5Evidence("ollama_cloud");
  assert.deepEqual(auditR5Evidence(document), { providerClass: "ollama_cloud", cases: 14, liveState: "live_pending" });
  for (const mutate of [
    (doc) => { doc.localPolicy.effort.mapping.ultra = "high"; },
    (doc) => { doc.localPolicy.effort.invalid = "silently omitted"; },
    (doc) => { doc.localPolicy.streamTerminal = "raw EOF is success"; },
    (doc) => { doc.localPolicy.modelCapabilities = "all Cloud models support max"; },
    (doc) => { doc.scope.credentialRail = "pooled OAuth"; },
    (doc) => { doc.cases[7].sent = "high"; },
    (doc) => { doc.reference.realReceipt = true; },
  ]) {
    const changed = structuredClone(document);
    mutate(changed);
    assert.throws(() => auditR5Evidence(changed));
  }
});


test("Claude R5 evidence preserves native directives, terminal owner and credential boundaries", () => {
  const document = loadR5Evidence("claude_oauth");
  assert.deepEqual(auditR5Evidence(document), { providerClass: "claude_oauth", cases: 10, liveState: "live_pending" });
  for (const mutate of [
    (doc) => { doc.localPolicy.drain.adopted = true; },
    (doc) => { doc.localPolicy.drain.referenceUniversalTimeOrCancellationBound = true; },
    (doc) => { doc.localPolicy.terminal = "message_stop alone is success"; },
    (doc) => { doc.scope.credentialRail = "shared OAuth/setup-token/API-key pool"; },
    (doc) => { doc.localPolicy.tlsFingerprintChange = true; },
    (doc) => { doc.nativeRequest.messages[3].output_config.effort = "high"; },
    (doc) => { doc.nativeResponse.content[0].thinking = ""; },
    (doc) => { doc.reference.realReceipt = true; },
  ]) {
    const changed = structuredClone(document);
    mutate(changed);
    assert.throws(() => auditR5Evidence(changed));
  }
});


test("Codex R5 evidence fences stale controls, partial rotation and post-commit replay", () => {
  const document = loadR5Evidence("codex_oauth");
  assert.deepEqual(auditR5Evidence(document), { providerClass: "codex_oauth", cases: 10, liveState: "live_pending" });
  for (const mutate of [
    (doc) => { doc.localPolicy.ack.terminalIsAck = true; },
    (doc) => { doc.localPolicy.ack.oldConnectionAckCanAcknowledgeNewTurn = true; },
    (doc) => { doc.localPolicy.ack.timeoutMs *= 2; },
    (doc) => { doc.localPolicy.refreshPartial = "retry the consumed refresh token"; },
    (doc) => { doc.localPolicy.postCommit = "reconnect and replay after a partial response"; },
    (doc) => { doc.scope.credentialRail = "shared OAuth/SIWC/key pool"; },
    (doc) => { doc.cases[6].providerTypes = ["claude_oauth"]; },
    (doc) => { doc.cases[7].providerTypes = ["gemini_cli"]; },
    (doc) => { doc.reference.realReceipt = true; },
  ]) {
    const changed = structuredClone(document);
    mutate(changed);
    assert.throws(() => auditR5Evidence(changed));
  }
});


test("Cursor R5 evidence keeps independent rails, structured tools and estimated usage", () => {
  const document = loadR5Evidence("cursor");
  assert.deepEqual(auditR5Evidence(document), { providerClass: "cursor", cases: 10, liveState: "live_pending" });
  for (const mutate of [
    (doc) => { doc.scope.credentialRails = ["interchangeable OAuth/key pool"]; },
    (doc) => { doc.localPolicy.markerOnly.guessExecutableCallFromText = true; },
    (doc) => { doc.localPolicy.constraints.json = "native_guarantee"; },
    (doc) => { doc.localPolicy.usage.usageEstimated = false; },
    (doc) => { doc.localPolicy.usage.cacheUsageObserved = true; },
    (doc) => { doc.localPolicy.replay = "replay any 5xx after business output"; },
    (doc) => { doc.reference.realReceipt = true; },
  ]) {
    const changed = structuredClone(document);
    mutate(changed);
    assert.throws(() => auditR5Evidence(changed));
  }
});


test("Grok R5 evidence separates semantic output, operation scope and committed replay", () => {
  const document = loadR5Evidence("grok_oauth");
  assert.deepEqual(auditR5Evidence(document), {"providerClass": "grok_oauth", "cases": 10, "liveState": "live_pending"});
  for (const mutate of [
    (doc) => { doc.scope.credentialRail = 'interchangeable CLI OAuth/Web SSO pool'; },
    (doc) => { doc.localPolicy.controlFramesAreSemantic = true; },
    (doc) => { doc.localPolicy.afterWriteReplay = true; },
    (doc) => { doc.localPolicy.mediaCoveredByChat = true; },
    (doc) => { doc.reference.realReceipt = true; },
  ]) {
    const changed = structuredClone(document);
    mutate(changed);
    assert.throws(() => auditR5Evidence(changed));
  }
});


test("Buddy R5 evidence keeps product/site/platform, strict terminal and existing quota boundaries", () => {
  const document = loadR5Evidence("buddy");
  assert.deepEqual(auditR5Evidence(document), {"providerClass": "buddy", "cases": 10, "liveState": "live_pending"});
  for (const mutate of [
    (doc) => { doc.scope.credentialRail = 'interchangeable products/sites/platforms'; },
    (doc) => { doc.localPolicy.identitySwitchOnFailure = true; },
    (doc) => { doc.localPolicy.tlsProfileChanged = true; },
    (doc) => { doc.localPolicy.nativeMediaGenerationEnabled = true; },
    (doc) => { doc.localPolicy.terminal = 'normal EOF synthesizes stop'; },
    (doc) => { doc.reference.realReceipt = true; },
  ]) {
    const changed = structuredClone(document);
    mutate(changed);
    assert.throws(() => auditR5Evidence(changed));
  }
});


test("Trae R5 evidence keeps three rails, PKCE ownership and native versus simulated tools", () => {
  const document = loadR5Evidence("trae");
  assert.deepEqual(auditR5Evidence(document), {"providerClass": "trae", "cases": 10, "liveState": "live_pending"});
  for (const mutate of [
    (doc) => { doc.localPolicy.rawToolsEnabledByDefault = true; },
    (doc) => { doc.localPolicy.soloImagesSupported = true; },
    (doc) => { doc.localPolicy.crossRailFallback = true; },
    (doc) => { doc.localPolicy.terminal = 'EOF synthesizes stop'; },
    (doc) => { doc.reference.realReceipt = true; },
  ]) {
    const changed = structuredClone(document);
    mutate(changed);
    assert.throws(() => auditR5Evidence(changed));
  }
});


test("DeepSeek R5 evidence preserves fixed bearer scope and actual PoW cancellation/admission owners", () => {
  const document = loadR5Evidence("deepseek_account");
  assert.deepEqual(auditR5Evidence(document), {"providerClass": "deepseek_account", "cases": 10, "liveState": "live_pending"});
  for (const mutate of [
    (doc) => { doc.scope.apps.push("codex"); },
    (doc) => { doc.localPolicy.pow.maxConcurrentWorkers *= 2; },
    (doc) => { doc.localPolicy.pow.maxLocalWorkMs *= 2; },
    (doc) => { doc.localPolicy.pow.permit = 'release when the async future is cancelled'; },
    (doc) => { doc.localPolicy.toolMode = 'native tools without nonce'; },
    (doc) => { doc.reference.realReceipt = true; },
  ]) {
    const changed = structuredClone(document);
    mutate(changed);
    assert.throws(() => auditR5Evidence(changed));
  }
});


test("Static R5 evidence freezes all 26 Registry profiles and strict Bedrock response ownership", () => {
  const document = loadR5Evidence("static_profiles");
  assert.deepEqual(auditR5Evidence(document), {"providerClass": "static_profiles", "cases": 10, "liveState": "live_pending"});
  for (const mutate of [
    (doc) => { doc.profiles[0].app = "gemini"; },
    (doc) => { doc.localPolicy.crossScopeFallback = true; },
    (doc) => { doc.localPolicy.bedrock.streamTerminal = "raw EOF is success"; },
    (doc) => { doc.localPolicy.bedrock.maxFrameBytes *= 2; },
    (doc) => { doc.localPolicy.deepseekApiClaudePath = "/v1/messages"; },
    (doc) => { doc.reference.realReceipt = true; },
  ]) {
    const changed = structuredClone(document);
    mutate(changed);
    assert.throws(() => auditR5Evidence(changed));
  }
});


test("Static R5 matrix detects actual Registry driver, key and regional-route drift", () => {
  const document=loadR5Evidence("static_profiles");
  const registry=JSON.parse(fs.readFileSync(new URL("../../assets/contract/provider-registry.json",import.meta.url),"utf8"));
  const runtime=fs.readFileSync(new URL("../../src/domain/providers/runtime.rs",import.meta.url),"utf8");
  assert.deepEqual(auditStaticProfileProjection(document,registry,runtime),{profiles:26,codingPlans:20});
  for (const mutate of [
    (data)=>{data.profiles.find(p=>p.profileId==="claude.nvidia").driverBinding.driverId="http.anthropic_messages";},
    (data)=>{data.profiles.find(p=>p.profileId==="codex.deepseek_api").credentialPolicy.slots=["account.api_key"];},
    (data)=>{data.profiles.find(p=>p.codingPlan).codingPlan.inference.fixedOrigin="https://other-region.invalid";},
    (data)=>{data.profiles.find(p=>p.codingPlan).codingPlan.routes[0].path="/wrong/messages";},
    (data)=>{data.families.find(p=>p.familyId==="family.nvidia").credentialSourceScope="surface";},
  ]) {
    const changed=structuredClone(registry);mutate(changed);
    assert.throws(()=>auditStaticProfileProjection(document,changed,runtime));
  }
  assert.throws(()=>auditStaticProfileProjection(document,registry,runtime.replace('Some("https://integrate.api.nvidia.com")','Some("https://other-origin.invalid")')));
});


test("Web Session R5 evidence keeps encrypted Cookie scope and buffered strict terminals", () => {
  const document = loadR5Evidence("web_session");
  assert.deepEqual(auditR5Evidence(document), {"providerClass": "web_session", "cases": 9, "liveState": "live_pending"});
  for (const mutate of [
    (doc) => { doc.localPolicy.tokenByTokenStreaming = true; },
    (doc) => { doc.localPolicy.automaticCookieRotation = true; },
    (doc) => { doc.localPolicy.tools = true; },
    (doc) => { doc.localPolicy.authRecovery = "refresh and retry"; },
    (doc) => { doc.reference.realReceipt = true; },
  ]) {
    const changed = structuredClone(document);
    mutate(changed);
    assert.throws(() => auditR5Evidence(changed));
  }
});


test("R5 shared timing and resource evidence rejects scope, ownership and policy drift", () => {
  const document = loadR5Evidence("shared_core");
  assert.deepEqual(auditR5Evidence(document), {"providerClass": "shared_core", "cases": 8, "liveState": "live_pending"});
  for (const mutate of [
    (doc) => { doc.liveState = "live_passed"; },
    (doc) => { doc.localPolicy.httpHandoffIsSocketWrite = true; },
    (doc) => { doc.localPolicy.capacityIsAccountInvalidation = true; },
    (doc) => { doc.scope.registry.labels.push("caller-controlled-label"); },
    (doc) => { doc.scope.resources.trae.MAX_RUNTIME_BYTES *= 2; },
  ]) {
    const changed = structuredClone(document);
    mutate(changed);
    assert.throws(() => auditR5Evidence(changed));
  }
});


test("R5 shared core projection rejects live Registry and resource-bound drift", () => {
  const document = loadR5Evidence("shared_core");
  assert.deepEqual(auditSharedCoreProjection(document), { profiles:85, providerTypes:28, timingLabels:30 });
  const registry = JSON.parse(fs.readFileSync(new URL("../../assets/contract/provider-registry.json", import.meta.url),"utf8"));
  const changed = structuredClone(registry);
  changed.profiles.find((profile) => profile.compatibilityProviderType).driverBinding.driverId = "unreviewed.driver";
  assert.throws(() => auditSharedCoreProjection(document, { registry:changed }));
  const source = fs.readFileSync(new URL("../../src/proxy/deepseek_runtime.rs", import.meta.url),"utf8");
  const raised = source.replace("MAX_ACTIVE_SESSION_FLIGHTS: usize = 64;", "MAX_ACTIVE_SESSION_FLIGHTS: usize = 65;");
  assert.notEqual(raised,source);
  assert.throws(() => auditSharedCoreProjection(document, { deepseek:raised }));
});
