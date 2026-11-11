import { quotaConversionInput, quotaConversionNeeded, calibratedConversionRow, type ValuePreviewRow } from "./share-value-quota";
import { describe, expect, it } from "vitest";
import { parseValueLimit, valueLimitInput, parseValueUsage, valueUsageLabel, valueUsageInput, parseInitialValueUsage, valueStateKey, matchingValueStatus, valueQuotaAvailability, valueCapabilityReady, isValuePreview, type ValueStatus, type ValuePreview,
  parseValueLimitInput, valueLimitLabel, valueRowKind, valuePreviewNeedsConfirmation, valuePreviewBlocker, valueErrorMessage, valueRequestError, ValueQuotaResponseError,
  splitValueRows, mergeValueRows, valueStatusReason, VALUE_STATUS_STALE_MS,
  valueFailedChecks, valueCheckBlocker, valueReadinessRoots, valueReadinessSummary, type ValueCheck } from "./share-value-quota";
import { applyShareUserPolicyBatch, patchedSharePolicy } from "../components/providers/share-user-policy-batch";
import { stageGrantPolicy } from "../utils/shareFormUtils";
import { normalizeShareRecord, normalizeShareRecords } from "../utils/shareRecordNormalize";
import { HttpResponseError } from "./runtime";
import type { ShareUserGrant } from "./api/share";

describe("Share USD-equivalent quotas", () => {
  it("initializes only new immediately active USD users and preserves pico precision", () => {
    const row = {email: "new@example.test", effectiveFromMs: 0, reasonCode: "ready", windowId: "Lifetime:0:0", canInitializeUsage: true};
    const preview: ValuePreview = {ready: true, checks: [{code: "value_usage_initialization", ready: true}], rows: [row]};
    const parse = (raw: string, reason = " Reviewed ", existing = false, p = preview) => parseInitialValueUsage(p, row.email, raw, reason, existing);
    expect(parse("")).toEqual({ok: true});
    expect(parse("0", "")).toEqual({ok: true});
    expect(parse("0.000000000001")).toEqual({ok: true, initialization: {targetPicos: "1", windowId: row.windowId, reason: "Reviewed"}});
    for (const raw of ["-1", "1e3", "0.0000000000001", "1000000001"]) expect(parse(raw)).toEqual({ok: false});
    expect(parse("1", "")).toEqual({ok: false});
    expect(parse("1", "x".repeat(1001))).toEqual({ok: false});
    expect(parse("1", "中".repeat(334))).toEqual({ok: false});
    expect(parse("1", "reason", true)).toEqual({ok: false});
    for (const p of [{...preview, checks: []}, {...preview, ready: false}, {...preview, rows: [{...row, canInitializeUsage: false}]},
      {...preview, rows: [{...row, windowId: undefined}]}, {...preview, rows: [{...row, reasonCode: "scheduled_next_window"}]},
      {...preview, rows: [{...row, reasonCode: "blocked_needs_reconcile"}]}]) expect(parse("1", "reason", false, p)).toEqual({ok: false});
    for (const value of ["0", "1", "123456789123", "1000000000000000000000"]) expect(parseValueUsage(valueUsageInput(value))).toBe(value);
  });
  it("separates infrastructure readiness from policy coverage and fails closed on missing checks",()=>{
    const checks=["router_protocol","sqlite_authority","metering_storage","acknowledged_tariff","text_surface"].map(code=>({code,ready:true}));
    const preview:ValuePreview={ready:false,capabilityReady:true,checks,rows:[]};
    expect(valueCapabilityReady(preview)).toBe(true);
    expect(valueCapabilityReady({...preview,ready:true,checks:undefined})).toBe(false);
    for(const check of checks) {
      expect(valueCapabilityReady({...preview,checks:checks.filter(c=>c!==check)})).toBe(false);
      expect(valueCapabilityReady({...preview,checks:checks.map(c=>c===check?{...c,ready:false}:c)})).toBe(false);
    }
    expect(valueCapabilityReady({...preview,checks:[...checks,{code:"text_surface",ready:false}]})).toBe(false);
    expect(isValuePreview({...preview,configRevision:7},7)).toBe(true);
    for(const invalid of [null,{},[],{ready:true,configRevision:6,rows:[]},{ready:true,configRevision:7,rows:[],checks:[null]}])expect(isValuePreview(invalid,7)).toBe(false);
  });
  it("keeps execution-model coverage as an actionable root and preserves binding identity",()=>{
    const base = ["router_protocol", "sqlite_authority", "metering_storage", "acknowledged_tariff", "text_surface"]
      .map(code => ({code, ready: true}));
    const coverage: ValueCheck = {code: "model_price_coverage", ready: false, app: "codex", providerId: "buddy-provider",
      profileId: "codex.workbuddy_oauth", driverId: "special.workbuddy_oauth", model: "auto", reasonCode: "model_unresolved"};
    const checks = [...base, coverage];
    expect(valueReadinessSummary(checks)).toEqual({root: coverage, more: 0});
    expect(valueCapabilityReady({ready: true, capabilityReady: true, rows: [], checks})).toBe(false);
    expect(valueReadinessRoots([...checks, {code: "future_check", ready: false}])).toEqual([coverage, {code: "future_check", ready: false}]);
    const waiting = {...coverage, blockedBy: "acknowledged_tariff"};
    const missingTariff = [...base.map(check => check.code === "acknowledged_tariff" ? {...check, ready: false} : check), waiting];
    expect(valueReadinessRoots(missingTariff).map(check => check.code)).toEqual(["acknowledged_tariff"]);
    expect(valueCheckBlocker(waiting, missingTariff)).toBe("acknowledged_tariff");
    expect(isValuePreview({ready: false, capabilityReady: false, configRevision: 7, checks, rows: []}, 7)).toBe(true);
    expect(coverage.providerId).toBe("buddy-provider");
    expect(coverage.reasonCode).toBe("model_unresolved");
  });
  it("names the root cause of an unready USD quota, not its consequences",()=>{
    const fail=(code:string,blockedBy?:string):ValueCheck=>({code,ready:false,...(blockedBy?{blockedBy}:{})});
    const ok=(code:string):ValueCheck=>({code,ready:true});
    // Storage not committed: the tariff check only waits on it.
    const storage=[ok("router_protocol"),fail("acknowledged_tariff","sqlite_authority"),fail("sqlite_authority"),ok("metering_storage"),ok("text_surface")];
    expect(valueReadinessRoots(storage).map(c=>c.code)).toEqual(["sqlite_authority"]);
    expect(valueReadinessSummary(storage)).toEqual({root:storage[2],more:0});
    expect(valueCheckBlocker(storage[1],storage)).toBe("sqlite_authority");
    expect(valueFailedChecks(storage).map(c=>c.code)).toEqual(["sqlite_authority","acknowledged_tariff"]);
    // A blocker that passes (or is missing) no longer hides the check.
    const cleared=[ok("sqlite_authority"),fail("acknowledged_tariff","sqlite_authority")];
    expect(valueReadinessRoots(cleared).map(c=>c.code)).toEqual(["acknowledged_tariff"]);
    expect(valueCheckBlocker(cleared[1],cleared)).toBeUndefined();
    expect(valueReadinessRoots([fail("acknowledged_tariff","router_protocol")]).map(c=>c.code)).toEqual(["acknowledged_tariff"]);
    expect(valueCheckBlocker(fail("sqlite_authority","sqlite_authority"),[fail("sqlite_authority","sqlite_authority")])).toBeUndefined();
    // Chains resolve to their start; fix-first order, unknown codes last, +N counts other root codes.
    const mixed=[fail("future_check"),fail("text_surface"),fail("acknowledged_tariff","tariff_published"),fail("tariff_published","router_protocol"),
      fail("metering_storage"),fail("router_protocol"),ok("sqlite_authority"),fail("text_surface")];
    expect(valueReadinessRoots(mixed).map(c=>c.code)).toEqual(["router_protocol","metering_storage","text_surface","text_surface","future_check"]);
    expect(valueReadinessSummary(mixed)).toEqual({root:mixed[5],more:3});
    expect(valueFailedChecks(mixed).map(c=>c.code)).toEqual(["router_protocol","metering_storage","tariff_published","acknowledged_tariff","text_surface","text_surface","future_check"]);
    // Several unverified bindings are one cause.
    expect(valueReadinessSummary([fail("text_surface"),fail("text_surface")])?.more).toBe(0);
    // A blocker cycle must not hide every failure.
    expect(valueReadinessRoots([fail("a","b"),fail("b","a")]).map(c=>c.code)).toEqual(["a","b"]);
    expect(valueReadinessSummary([ok("router_protocol")])).toBeUndefined();
    expect(valueReadinessSummary(undefined)).toBeUndefined();
    expect(valueReadinessRoots([null as unknown as ValueCheck,fail("sqlite_authority")]).map(c=>c.code)).toEqual(["sqlite_authority"]);
  });
  it("keeps micro limits and pico usage exact without wallet conversion", () => {
    expect(parseValueLimit("0.000001")).toBe("1");
    expect(parseValueLimit("1000000000")).toBe("1000000000000000");
    expect(valueLimitInput("12345678")).toBe("12.345678");
    for(const [input,micros] of [["$10.00","10000000"],[" 10","10000000"],[".5","500000"],["05","5000000"],["1,000","1000000000"],["US$ 1,250.5","1250500000"],["10.","10000000"]])
      expect(parseValueLimit(input),input).toBe(micros);
    for(const [input,error] of [["","empty"],["$","empty"],["0","too_small"],["1.0000001","too_many_decimals"],["1000000000.000001","too_large"],
      ["1e6","format"],["-1","format"],["1,00","format"],[".","format"],["1 000","format"]] as const) expect(parseValueLimitInput(input),input).toEqual({ok:false,error});
    expect(valueLimitLabel("10000000")).toBe("US$10");
    expect(valueLimitLabel("1000500000")).toBe("US$1,000.50");
    expect(valueUsageLabel("1500000000000")).toBe("US$1.50");
    expect(valueUsageLabel("1234000000000000")).toBe("US$1,234.00");
    expect(parseValueUsage("$1,000.5")).toBe("1000500000000000");
    expect(parseValueUsage("0")).toBe("0");
    expect(parseValueUsage("0.000000000001")).toBe("1");
    expect(parseValueUsage("1.0000000000001")).toBeUndefined();
    expect(valueUsageLabel("1")).toBe("< US$0.000001");
    expect(valueUsageLabel("18000000000")).toBe("US$0.018");
    expect(valueUsageLabel(undefined)).toBe("—");
    expect(valueQuotaAvailability("999999999999","1000000")).toEqual({remainingPicos:"1",exhausted:false,nearLimit:true,percent:99.99});
    expect(valueQuotaAvailability("1000000000001","1000000")).toEqual({remainingPicos:"0",exhausted:true,nearLimit:true,percent:100});
    expect(valueQuotaAvailability("700000000000","1000000")?.nearLimit).toBe(false);
    expect(valueQuotaAvailability(undefined,"1000000")).toBeUndefined();
  });
  const quota={limitUsdMicros:"1000000",tariffPolicyId:"api-equivalent-v1" as const};
  const binding={quotaPolicyEpoch:"epoch",valueMeteredFromMs:1,effectiveFromMs:2,pricingRevision:"revision"};
  const grant:ShareUserGrant={email:"user@example.test",role:"shareto",active:true,
    policy:{tokenLimit:100,tokenPeriod:"day"},valueMetering:binding,
    pendingValueQuota:{policy:{valueQuota:quota,tokenPeriod:"day"},effectiveFromMs:2,retainAccess:true}};
  it("preserves pending policy in unrelated batch edits and replaces quota units", () => {
    const one={[grant.email]:grant};
    const edited=applyShareUserPolicyBatch(one,new Set([grant.email]),{parallelLimit:{value:2}});
    expect(edited[grant.email].policy.valueQuota).toEqual(quota);
    expect(edited[grant.email].policy.parallelLimit).toBe(2);
    const tokens=applyShareUserPolicyBatch(edited,new Set([grant.email]),{tokenLimit:{value:1000,period:"week"}});
    expect(tokens[grant.email].policy.valueQuota).toBeUndefined();
    expect(tokens[grant.email].policy.tokenLimit).toBe(1000);
    const dollars=applyShareUserPolicyBatch(tokens,new Set([grant.email]),{tokenLimit:{valueQuota:quota,period:"week"}});
    expect(dollars[grant.email].policy.tokenLimit).toBeUndefined();
    expect(one[grant.email].policy.tokenLimit).toBe(100);
  });
  it("explicit policy application replaces a legacy schedule", () => {
    const kept=stageGrantPolicy(grant,{...grant.pendingValueQuota!.policy});
    expect(kept.pendingValueQuota).toBeUndefined();
    expect(kept.policy).toEqual(grant.pendingValueQuota!.policy);
    const tokens=stageGrantPolicy(grant,{tokenLimit:300,tokenPeriod:"day"});
    expect(tokens.pendingValueQuota).toBeUndefined();
    expect(tokens.policy).toEqual({tokenLimit:300,tokenPeriod:"day"});
    expect(tokens.valueMetering).toEqual(binding);
    const batch=applyShareUserPolicyBatch({[grant.email]:grant},new Set([grant.email]),{tokenLimit:{value:7,period:"day"}});
    expect(batch[grant.email].pendingValueQuota).toBeUndefined();
    expect(patchedSharePolicy(grant,{expiresAt:{value:5}})).toEqual({...grant.pendingValueQuota!.policy,expiresAt:5});
    const broken={...grant,valueQuotaMalformed:true};
    expect(applyShareUserPolicyBatch({[grant.email]:broken},new Set([grant.email]),{parallelLimit:{value:2}})[grant.email]).toBe(broken);
  });
  it("keeps correction refusals apart from status rows and merges rows per user", () => {
    const row={email:"User@Example.Test",usedPicos:"1",meteringStatus:"observed"} as ValueStatus;
    const {rows,errors}=splitValueRows([row,{email:"b@example.test",error:{code:"quota_target_invalid"}},null]);
    expect(rows.map(item=>item.email)).toEqual(["user@example.test"]);
    expect(errors).toEqual([{email:"b@example.test",code:"quota_target_invalid"}]);
    const other={...row,email:"other@example.test"};
    expect(mergeValueRows([other,{...row,email:"user@example.test",usedPicos:"0"}],rows)).toEqual([other,rows[0]]);
    expect(valueErrorMessage({code:"value_quota_not_active"})).toEqual({key:"share.userLimit.valueQuota.error.notActive"});
    expect(valueStatusReason({...row,reasonCode:"pricing_unavailable"})).toBeUndefined();
    expect(valueStatusReason({...row,meteringStatus:"unavailable",reasonCode:"pricing_unavailable"})).toEqual({key:"share.userLimit.valueQuota.reasonCode.pricing_unavailable"});
    expect(valueStatusReason({...row,meteringStatus:"unavailable",reasonCode:"coverage_gap"})).toEqual({raw:"coverage_gap"});
    const preview:ValuePreview={ready:false,configRevision:3,rows:[],invalidRows:[{email:"a@example.test",reasonCode:"invalid_policy"}]};
    expect(isValuePreview(preview,3)).toBe(true);
    expect(isValuePreview({...preview,invalidRows:[{}]},3)).toBe(false);
    expect(valuePreviewBlocker(preview)).toEqual({email:"a@example.test",message:{key:"share.userLimit.valueQuota.reasonCode.invalid_policy"}});
  });
  it("normalizes monetary evidence without converting or accepting malformed limits", () => {
    const raw={id:"share",userGrants:{[grant.email]:grant}};
    const read=normalizeShareRecord(raw)!;
    expect(read.userGrants?.[grant.email].valueMetering).toEqual(binding);
    expect(read.userGrants?.[grant.email].pendingValueQuota).toEqual(grant.pendingValueQuota);
    for(const limit of [1,"0","01","1000000000000001"]) {
      const tolerant=normalizeShareRecord({id:"share",userGrants:{[grant.email]:{...grant,
        policy:{valueQuota:{...quota,limitUsdMicros:limit},tokenPeriod:"day"}}}})!;
      // Kept exactly as received and flagged, never read as "no USD quota".
      expect(tolerant.userGrants?.[grant.email].policy.valueQuota).toEqual({...quota,limitUsdMicros:limit});
      expect(tolerant.userGrants?.[grant.email].valueQuotaMalformed).toBe(true);
      expect(tolerant.userGrants?.[grant.email].valueMetering).toEqual(binding);
    }
    const snake=normalizeShareRecord({id:"share",userGrants:{[grant.email]:{...grant,pendingValueQuota:undefined,
      pending_value_quota:{policy:{value_quota:{limit_usd_micros:"1000000",tariff_policy_id:"api-equivalent-v1"},token_period:"week"},effective_from_ms:9,retain_access:false}}}})!;
    expect(snake.userGrants?.[grant.email].pendingValueQuota).toEqual({policy:{valueQuota:quota,tokenPeriod:"week"},effectiveFromMs:9,retainAccess:false});
    const list=normalizeShareRecords([{id:"a",userGrants:{[grant.email]:{...grant,valueMetering:{broken:true},pendingValueQuota:{policy:null}}}},{id:"b"}]);
    expect(list.map(item=>item.id)).toEqual(["a","b"]);
    expect(list[0].userGrants?.[grant.email].valueMetering).toEqual({broken:true});
    expect(list[0].userGrants?.[grant.email].pendingValueQuota).toEqual({policy:null});
    expect(list[0].userGrants?.[grant.email].valueQuotaMalformed).toBe(true);
    expect(read.userGrants?.[grant.email].valueQuotaMalformed).toBeUndefined();
    expect(snake.userGrants?.[grant.email].valueQuotaMalformed).toBeUndefined();
  });
  it("labels missing, stale and uncertain evidence without implying zero", () => {
    const status={meteringStatus:"pending",syncedAtMs:1} as ValueStatus;
    expect(valueStateKey(undefined)).toBe("share.userLimit.valueQuota.pending");
    expect(valueStateKey(status,2)).toBe("share.userLimit.valueQuota.state.pending");
    expect(valueStateKey(status,1+VALUE_STATUS_STALE_MS)).toBe("share.userLimit.valueQuota.state.pending");
    expect(valueStateKey(status,2+VALUE_STATUS_STALE_MS)).toBe("share.userLimit.valueQuota.stale");
    expect(valueStateKey({...status,meteringStatus:"unexpected"},2)).toBe("share.userLimit.valueQuota.state.unavailable");
  });
  it("never attaches usage to another policy, user, or pending draft",()=>{
    const active={...grant,policy:{valueQuota:quota,tokenPeriod:"day" as const},pendingValueQuota:undefined};
    const status={email:grant.email,policyEpoch:binding.quotaPolicyEpoch,limitUsdMicros:quota.limitUsdMicros} as ValueStatus;
    expect(matchingValueStatus([status],active)).toBe(status);
    expect(matchingValueStatus([status],{...active,active:false})).toBeUndefined();
    expect(matchingValueStatus([status],{...active,email:"other@example.test"})).toBeUndefined();
    expect(matchingValueStatus([status],{...active,policy:{valueQuota:{...quota,limitUsdMicros:"2000000"},tokenPeriod:"day"}})).toBeUndefined();
  });
  it("classifies previews by reason code and asks to confirm only a later start or carried usage",()=>{
    const row={email:"a@example.test",effectiveFromMs:10000,meteredFromMs:10000,usedPicos:"0"};
    expect(valueRowKind({...row,reasonCode:"scheduled_next_window"},0)).toBe("scheduled");
    expect(valueRowKind({...row,reasonCode:"metered_from_now"},0)).toBe("fromNow");
    expect(valueRowKind({...row,reasonCode:"invalid_policy"},0)).toBe("blocked");
    expect(valueRowKind({...row,reasonCode:"ready",ready:false},0)).toBe("blocked");
    expect(valueRowKind(row,0)).toBe("scheduled");
    const preview:ValuePreview={ready:true,configRevision:1,rows:[{...row,reasonCode:"metered_from_now"}]};
    expect(valuePreviewNeedsConfirmation(preview,0)).toBe(false);
    expect(valuePreviewNeedsConfirmation({...preview,rows:[{...row,reasonCode:"ready",usedPicos:"3"}]},0)).toBe(true);
    expect(valuePreviewBlocker({...preview,ready:false,rows:[{...row,reasonCode:"blocked_needs_reconcile"}]},0))
      .toEqual({email:row.email,message:{key:"share.userLimit.valueQuota.reasonCode.blocked_needs_reconcile"}});
    expect(valuePreviewBlocker({...preview,ready:false,rows:[],reason:"Unknown server text"},0)?.message).toEqual({raw:"Unknown server text"});
  });
  it("maps request failures to the same readiness distinctions as the Router",()=>{
    expect(valueRequestError(new HttpResponseError("x",409,{code:"quota_revision_conflict"}))).toBe("conflict");
    expect(valueRequestError(new HttpResponseError("x",401))).toBe("controlAuth");
    expect(valueRequestError(new HttpResponseError("x",404))).toBe("upgrade");
    expect(valueRequestError(new HttpResponseError("x",504))).toBe("timeout");
    expect(valueRequestError(new TypeError("Failed to fetch"))).toBe("offline");
    expect(valueRequestError(new ValueQuotaResponseError())).toBe("invalidResponse");
    expect(valueRequestError(new Error("boom"))).toBe("failed");
    expect(valueErrorMessage(new HttpResponseError("x",400,{code:"quota_target_invalid"}))).toEqual({key:"share.userLimit.valueQuota.error.targetInvalid"});
    expect(valueStateKey({meteringStatus:"blocked",syncedAtMs:1} as ValueStatus,2)).toBe("share.userLimit.valueQuota.state.blocked");
  });
});


describe("signed quota conversions",()=>{
  it("retains exact calibrated amounts for independent batch rows and rejects stale/invalid evidence",()=>{
    const usd:ValuePreviewRow={email:"a@example.test",effectiveFromMs:1,conversionRequired:true,conversionCredential:"signed",credentialExpiresAtMs:Date.now()+60_000,ready:true,calibrationRequired:true,targetUnit:"usdEquivalent",usedPicos:"42000000000000",pendingRiskPicos:"2000000000000",limitUsdMicros:"100000000",coverage:{complete:true,missingFacts:0,evidenceTokens:30_000_000,evidenceRequests:1,status:{meteringStatus:"observed",observedPicos:"42000000000000"}}};
    const calibration={amount:"50.000000000001",reason:"Reviewed"};
    expect(quotaConversionInput(usd,calibration)).toEqual({credential:"signed",targetUsedPicos:"50000000000001",reason:"Reviewed"});
    expect(calibratedConversionRow(usd,calibration).remainingPicos).toBe("47999999999999");
    for(const bad of [{amount:"41",reason:"Reviewed"},{amount:"50",reason:""},{amount:"50",reason:"中".repeat(334)}]) expect(()=>quotaConversionInput(usd,bad)).toThrow();
    expect(()=>quotaConversionInput({...usd,credentialExpiresAtMs:0},calibration)).toThrow();
    const token={...usd,targetUnit:"tokens" as const,observedTokens:30_000_000,tokenLimit:100_000_000,pendingTokens:5_000_000};
    expect(quotaConversionInput(token,{amount:"40000001",reason:"Reviewed"})?.targetUsedTokens).toBe(40_000_001);
    expect(calibratedConversionRow(token,{amount:"40000001",reason:"Reviewed"}).remainingTokens).toBe(54_999_999);
    expect(()=>quotaConversionInput(token,{amount:"29999999",reason:"Reviewed"})).toThrow();
    expect(quotaConversionNeeded({policy:{tokenPeriod:"day",valueQuota:{limitUsdMicros:"100000000",tariffPolicyId:"api-equivalent-v1"}}},{tokenPeriod:"day",tokenLimit:100_000_000})).toBe(true);
    expect(quotaConversionNeeded({policy:{tokenPeriod:"day",tokenLimit:100}},{tokenPeriod:"day",tokenLimit:200})).toBe(false);
  });
});
