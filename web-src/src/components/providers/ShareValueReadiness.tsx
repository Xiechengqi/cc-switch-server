// Hallmark · pre-emit critique: P4 H4 E4 S5 R5 V4 · component-scope; Server UI tokens.
import { useEffect, useReducer, useState } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";
import { shareApi } from "@/lib/api/share";
import { readToken } from "@/lib/runtime";
import { readRouterAccessToken, SERVER_AUTH_EXPIRED_EVENT } from "@/lib/routerAuth";
import { valueCapabilityReady, isValuePreview, type ValuePreview } from "@/lib/share-value-quota";

const actor = () => `${readToken() ?? ""}:${readRouterAccessToken() ?? ""}`;
export function useShareValueReadiness(shareId?:string, revision?:number) {
  const [session,setSession] = useState(actor);
  const [attempt,retry] = useReducer(n=>n+1,0);
  const key = `${session}:${shareId}:${revision}`;
  const [state,setState] = useState<{key:string;preview?:ValuePreview;error?:string;loading:boolean}>();
  useEffect(()=>{
    const changed=()=>setSession(actor());
    const events=["router-auth-changed","storage",SERVER_AUTH_EXPIRED_EVENT];
    events.forEach(name=>window.addEventListener(name,changed));
    return ()=>events.forEach(name=>window.removeEventListener(name,changed));
  },[]);
  useEffect(()=>{
    if(!shareId || revision==null)return;
    const controller = new AbortController(); let pending=false;
    const refresh=async()=>{
      if(pending || document.visibilityState==="hidden" || controller.signal.aborted || session!==actor())return;
      pending=true;setState(previous=>previous?.key===key?{...previous,error:undefined,loading:true}:{key,loading:true});
      try {
        const preview=await shareApi.previewValueQuota(shareId,revision,{},controller.signal);
        if(!isValuePreview(preview,revision))throw new Error("Invalid readiness response");
        if(!controller.signal.aborted && session===actor())setState({key,loading:false,preview,
          error:!Array.isArray(preview.checks)||preview.configRevision!==revision?"upgrade":undefined});
      }catch {
        if(!controller.signal.aborted && session===actor())setState({key,loading:false,error:"failed"});
      }finally{pending=false;}
    };
    const visible=()=>{void refresh();};
    void refresh();const timer=window.setInterval(visible,30_000);document.addEventListener("visibilitychange",visible);
    return ()=>{controller.abort();window.clearInterval(timer);document.removeEventListener("visibilitychange",visible);};
  },[key,shareId,revision,session,attempt]);
  const current=state?.key===key && session===actor()?state:undefined;
  return {key,preview:current?.preview,error:current?.error,loading:current?.loading??!!shareId,
    ready:!current?.error && valueCapabilityReady(current?.preview),retry};
}
export function ShareValueReadiness({state}:{state:ReturnType<typeof useShareValueReadiness>}) {
  const {t}=useTranslation();const prefix="share.userLimit.valueQuota.readiness.";
  return <div className="space-y-2 rounded-md border bg-background p-3 text-xs" aria-busy={state.loading}>
    <div className="flex flex-wrap items-center justify-between gap-2"><span role="status">{t(prefix+(state.loading?"loading":state.ready?"ready":"blocked"))}</span>
      <Button size="sm" variant="outline" disabled={state.loading} onClick={state.retry}>{t(prefix+"retry")}</Button></div>
    {state.error?<p role="alert" className="text-destructive">{t(prefix+state.error)}</p>:null}
    <ul className="list-disc space-y-1 pl-4">{state.preview?.checks?.filter(check=>!check.ready).map((check,index)=><li key={index}>
      {t(prefix+(["router_protocol","sqlite_authority","metering_storage","acknowledged_tariff","text_surface"].includes(check.code)?check.code:"failed"))}
      {check.code==="sqlite_authority" && check.state?` (${check.state})`:null}
      {check.code==="text_surface"?<span className="break-all font-mono"> {check.app} · {check.profileId??"—"} / {check.driverId??"—"}</span>:null}
    </li>)}</ul>
  </div>;
}
