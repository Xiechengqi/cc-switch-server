import { describe, expect, it } from "vitest";
import { parseValueLimit, valueLimitInput, parseValueUsage, valueUsageLabel, valueStateKey, matchingValueStatus, valueQuotaAvailability, type ValueStatus } from "./share-value-quota";
import { applyShareUserPolicyBatch } from "../components/providers/share-user-policy-batch";
import { normalizeShareRecord } from "../utils/shareRecordNormalize";
import type { ShareUserGrant } from "./api/share";

describe("Share USD-equivalent quotas", () => {
  it("keeps micro limits and pico usage exact without wallet conversion", () => {
    expect(parseValueLimit("0.000001")).toBe("1");
    expect(parseValueLimit("1000000000")).toBe("1000000000000000");
    expect(valueLimitInput("12345678")).toBe("12.345678");
    for(const bad of ["0","01","1.",".1","1e6","-1"," 1","1.0000001","1000000000.000001"]) expect(parseValueLimit(bad)).toBeUndefined();
    expect(parseValueUsage("0")).toBe("0");
    expect(parseValueUsage("0.000000000001")).toBe("1");
    expect(parseValueUsage("1.0000000000001")).toBeUndefined();
    expect(valueUsageLabel("1")).toBe("< US$0.000001");
    expect(valueUsageLabel("18000000000")).toBe("US$0.018");
    expect(valueUsageLabel(undefined)).toBe("—");
    expect(valueQuotaAvailability("999999999999","1000000")).toEqual({remainingPicos:"1",exhausted:false});
    expect(valueQuotaAvailability("1000000000001","1000000")).toEqual({remainingPicos:"0",exhausted:true});
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
  it("normalizes monetary evidence without converting or accepting malformed limits", () => {
    const raw={id:"share",userGrants:{[grant.email]:grant}};
    const read=normalizeShareRecord(raw)!;
    expect(read.userGrants?.[grant.email].valueMetering).toEqual(binding);
    expect(read.userGrants?.[grant.email].pendingValueQuota).toEqual(grant.pendingValueQuota);
    for(const limit of [1,"0","01","1000000000000001"]) {
      expect(()=>normalizeShareRecord({id:"share",userGrants:{[grant.email]:{...grant,
        policy:{valueQuota:{...quota,limitUsdMicros:limit},tokenPeriod:"day"}}}})).toThrow();
    }
  });
  it("labels missing, stale and uncertain evidence without implying zero", () => {
    const status={meteringStatus:"pending",syncedAtMs:1} as ValueStatus;
    expect(valueStateKey(undefined)).toBe("share.userLimit.valueQuota.pending");
    expect(valueStateKey(status,2)).toBe("share.userLimit.valueQuota.state.pending");
    expect(valueStateKey(status,100000)).toBe("share.userLimit.valueQuota.stale");
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
});
