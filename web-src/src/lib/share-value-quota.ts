// Wire amounts remain integers. Never convert quota limits through Number.
import type { ShareUserGrant } from "./api/share";
export function parseValueLimit(value:string):string|undefined {
  if (!/^(0|[1-9]\d*)(\.\d{1,6})?$/.test(value)) return undefined;
  const [units,part=""]=value.split(".");
  const result=BigInt(units)*1000000n+BigInt(part.padEnd(6,"0"));
  if (result<=0n || result>1000000000000000n) return undefined;
  return result.toString();
}
export function valueLimitInput(micros:string):string {
  if(!/^(0|[1-9]\d*)$/.test(micros))return "";
  const value=BigInt(micros),fraction=(value%1000000n).toString().padStart(6,"0").replace(/0+$/,"");
  return (value/1000000n).toString()+(fraction?"."+fraction:"");
}
export function valueUsageLabel(picos:string|undefined):string {
  if(!picos || !/^(0|[1-9]\d*)$/.test(picos))return "—";
  const value=BigInt(picos);
  if(value>0n && value<1000000n)return "< US$0.000001";
  const whole=value/1000000000000n,decimal=(value%1000000000000n/1000000n).toString().padStart(6,"0").replace(/0+$/,"").padEnd(2,"0");
  return "US$"+whole.toString()+"."+decimal;
}
export type ValuePreview={ready:boolean;configRevision?:number;capabilityReady?:boolean;
  checks?:Array<{code:string;ready:boolean;state?:string;app?:string;profileId?:string|null;driverId?:string|null}>;
  reason?:string|null;activePricingRevision?:string|null;rows:Array<{email:string;effectiveFromMs:number;usedPicos?:string|null}>};
export function isValuePreview(value:unknown,revision:number):value is ValuePreview {
  if(!value || typeof value!=="object")return false;
  const p=value as ValuePreview;
  return typeof p.ready==="boolean" && p.configRevision===revision && Array.isArray(p.rows)
    && (p.checks==null || (Array.isArray(p.checks) && p.checks.every(c=>c && typeof c.code==="string" && typeof c.ready==="boolean")));
}
export function valueCapabilityReady(value:ValuePreview|undefined) {
  return value?.capabilityReady === true && Array.isArray(value.checks)
    && ["router_protocol","sqlite_authority","metering_storage","acknowledged_tariff","text_surface"].every(code=>value.checks!.some(c=>c?.code===code))
    && value.checks.every(check=>check?.ready===true);
}
export type ValueStatus={email:string;usedPicos:string;observedPicos:string;manualOffsetPicos:string;windowId:string;policyEpoch:string;usageWatermark:number;meteringStatus:string;syncedAtMs:number;limitUsdMicros?:string|null};
export function matchingValueStatus(items:ValueStatus[],grant:ShareUserGrant):ValueStatus|undefined {
  if(!grant.active || !grant.valueMetering)return undefined;
  return items.find(status=>status.email===grant.email.trim().toLowerCase() && status.policyEpoch===grant.valueMetering!.quotaPolicyEpoch
    && (status.limitUsdMicros??null)===(grant.policy.valueQuota?.limitUsdMicros??null));
}
export function valueQuotaAvailability(usedPicos?:string,limitMicros?:string) {
  if(!usedPicos || !limitMicros || !/^(0|[1-9]\d{0,38})$/.test(usedPicos) || !/^[1-9]\d{0,15}$/.test(limitMicros))return undefined;
  const used=BigInt(usedPicos),limit=BigInt(limitMicros)*1000000n;
  return {remainingPicos:(used>=limit?0n:limit-used).toString(),exhausted:used>=limit};
}
export type ValueDiagnostics={attempts:number;pendingAttempts:number;unavailableAttempts:number;coverageGaps:number;dailyCheckpoints:number;databaseBytes:number;capacityWarning:boolean};
export function valueStateKey(status:ValueStatus|undefined,now=Date.now()):string {
  if(!status)return "share.userLimit.valueQuota.pending";
  if(now-status.syncedAtMs>90000)return "share.userLimit.valueQuota.stale";
  const name=["observed","pending","unavailable","conservative","scheduled"].includes(status.meteringStatus)?status.meteringStatus:"unavailable";
  return "share.userLimit.valueQuota.state."+name;
}
export type ValueUsageEdit={action:"set"|"clear";targetPicos?:string;expectedGrantRevision:number;windowId:string;expectedWatermark:number;operationId:string;reason:string};
export function parseValueUsage(value:string):string|undefined {
  if(!/^(0|[1-9]\d*)(\.\d{1,12})?$/.test(value))return undefined;
  const [whole,fraction=""]=value.split(".");
  const result=BigInt(whole)*1000000000000n+BigInt(fraction.padEnd(12,"0"));
  return result<=1000000000000000000000n ? result.toString():undefined;
}
