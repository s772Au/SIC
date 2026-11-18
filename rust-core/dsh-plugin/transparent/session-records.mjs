// Snapshot only JSON data. Native controls such as AbortSignal never enter the
// Session log; the original request object remains owned by the Harness.
export function jsonSnapshot(value){
  return JSON.parse(JSON.stringify(value,(_key,v)=>{
    if(typeof v==='function'||typeof v==='symbol'||v instanceof AbortSignal)return undefined;
    return v;
  }));
}

export async function createSessionRecorder(ctx,provided){
  const api=provided??await import('@deepseek-ai/dsh-session');
  if(typeof api.appendPluginRecord!=='function'||typeof api.pluginRecordOf!=='function')throw Error('native Session plugin record API required');
  return async(session,type,data,{key,flush=true}={})=>{
    const nativeType='plugin:'+type;
    if(key&&!session.snapshotEvents().some(event=>{
      const r=api.pluginRecordOf(event);return r?.type===nativeType&&r.data.record_key===key;
    }))api.appendPluginRecord(session,nativeType,{...jsonSnapshot(data),record_key:key});
    else if(!key)api.appendPluginRecord(session,nativeType,jsonSnapshot(data));
    if(flush){if(typeof ctx.sessions.flush!=='function'||await ctx.sessions.flush(session)!==true)throw Error('durable native Session flush listener required');}
  };
}
