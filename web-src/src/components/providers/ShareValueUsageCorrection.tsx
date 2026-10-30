import {useRef,useState} from "react";
import {useTranslation} from "react-i18next";
import {Button} from "@/components/ui/button";
import {Input} from "@/components/ui/input";
import {shareApi,type ShareUserGrant} from "@/lib/api/share";
import {parseValueUsage,valueUsageLabel,type ValueStatus,type ValueUsageEdit} from "@/lib/share-value-quota";

export function ShareValueUsageCorrection({shareId,revision,grant,status,onSaved}:{shareId:string;revision:number;grant:ShareUserGrant;status:ValueStatus;onSaved:(rows:ValueStatus[])=>void}) {
  const {t}=useTranslation();const [amount,setAmount]=useState("");const [explanation,setExplanation]=useState("");
  const [working,setWorking]=useState(false);const [failure,setFailure]=useState("");
  const operation=useRef<{key:string;body:ValueUsageEdit}|undefined>(undefined);
  const apply=async(action:"set"|"clear")=>{
    const target=action==="set"?parseValueUsage(amount):undefined;
    if(!explanation.trim() || (action==="set" && (target==null || BigInt(target)<BigInt(status.observedPicos)))) {
      setFailure(t("share.userLimit.valueQuota.correctionInvalid"));return;
    }
    const key=JSON.stringify([action,target,explanation,status.windowId,status.usageWatermark,grant.revision]);
    const body=operation.current?.key===key?operation.current.body:{action,targetPicos:target,reason:explanation.trim(),
      expectedGrantRevision:grant.revision??0,windowId:status.windowId,expectedWatermark:status.usageWatermark,operationId:crypto.randomUUID()};
    operation.current={key,body};setWorking(true);setFailure("");
    try{const response=await shareApi.editValueUsage(shareId,revision,{[grant.email]:body});operation.current=undefined;onSaved(response.items);}
    catch(error){setFailure(String(error));}finally{setWorking(false);}
  };
  return <details className="mt-2 max-w-sm text-xs"><summary>{t("share.userLimit.valueQuota.correction")}</summary>
    <p className="my-2">{t("share.userLimit.valueQuota.observed")}: {valueUsageLabel(status.observedPicos)} · {t("share.userLimit.valueQuota.offset")}: {valueUsageLabel(status.manualOffsetPicos)}</p>
    <div className="space-y-2"><Input value={amount} onChange={event=>setAmount(event.target.value)} disabled={working} inputMode="decimal" aria-label={t("share.userLimit.valueQuota.target")} placeholder={t("share.userLimit.valueQuota.target")}/>
      <Input value={explanation} onChange={event=>setExplanation(event.target.value)} disabled={working} maxLength={1000} aria-label={t("share.userLimit.valueQuota.reason")} placeholder={t("share.userLimit.valueQuota.reason")}/>
      <div className="flex flex-wrap gap-2"><Button size="sm" disabled={working} onClick={()=>void apply("set")}>{t("common.save")}</Button><Button size="sm" variant="outline" disabled={working} onClick={()=>void apply("clear")}>{t("share.userLimit.valueQuota.clearOffset")}</Button></div>
    </div>{failure?<p className="text-destructive" role="alert">{failure}</p>:null}
  </details>;
}
