import{readFile,mkdir,open,link,unlink}from'node:fs/promises';
import{join}from'node:path';import{randomUUID}from'node:crypto';
import{digest}from'./transport.mjs';

const material=event=>/^(user|developer|assistant|tool|turn|step|request|compaction)\//.test(event.type)||event.type==='session/end-seed'&&event.data.inherited===true;
const same=(a,b)=>a?.chain_id===b?.chain_id&&a?.scope_id===b?.scope_id&&(a?.role??'WORKER')===(b?.role??'WORKER');
const nonempty=value=>typeof value==='string'&&value.trim().length>0;
const migration='existing native history requires an independent explicit SIC migration; binding or compaction does not migrate history';

// This receipt lives in the host's durable projection directory. A record in
// an untrusted/imported native archive cannot authorize its own migration.
export function createSessionAdmission({ctx,actor,coreURL,root,clientFor,recordSession,sessionAPI,resolveBinding}){
 const owners=new Map(),pending=new WeakMap(),directory=join(root,'session-admissions'),endpoint=new URL(coreURL).href;
 function current(agent,binding){const prior=agent&&owners.get(agent.session.id);if(prior&&(prior.agent!==agent||prior.session!==agent.session||!same(prior.binding,binding)))throw Error('SIC session binding changed or was removed; recreate the Agent with its admitted binding');return binding;}
 function bindingFor(id,agent,binding){const prior=owners.get(id);agent??=ctx.agents.get(id);if(prior){if(!agent||agent!==prior.agent)throw Error('SIC session Agent is no longer current');current(agent,binding);}else if(binding&&agent)throw Error('SIC session is not enrolled; explicitly mount a fresh session or restore its trusted boundary');return binding;}
 async function admit(agent,binding){
  current(agent,binding);if(owners.has(agent.session.id))return;
  if(pending.has(agent))return pending.get(agent);
  const run=(async()=>{
   // Identify the receipt independently of its authorization tuple, so a
   // revoked binding cannot make an enrolled session appear unselected.
   const key=digest({session_id:agent.session.id}),path=join(directory,key+'.json');
   let receipt;try{receipt=JSON.parse(await readFile(path,'utf8'));}catch(error){if(error.code!=='ENOENT')throw error;}
   if(!binding){if(receipt)throw Error('SIC session binding was removed; restore its admitted binding');return;}
   const selected={chain_id:binding.chain_id,scope_id:binding.scope_id,role:binding.role??'WORKER'};
   if(!nonempty(selected.chain_id)||!nonempty(selected.scope_id)||!['WORKER','EDITOR'].includes(selected.role))throw Error('fresh SIC session requires an authorized Worker or Editor chain/scope binding');
   const identity={version:1,actor,coreURL:endpoint,session_id:agent.session.id,...selected};
   const events=agent.session.snapshotEvents(),markers=events.map(sessionAPI.pluginRecordOf).filter(record=>record?.type==='plugin:sic/enable-boundary'),[marker]=markers;
   if(receipt){
    if(digest(receipt.identity)!==digest(identity)||!Number.isSafeInteger(receipt.source_event_seq)||receipt.source_event_seq< -1||receipt.receipt_id!==key||digest(events.filter(event=>event.seq<=receipt.source_event_seq))!==receipt.source_digest)throw Error('SIC enable boundary does not match its trusted host receipt and native archive');
    if(marker&&(markers.length!==1||marker.seq<=receipt.source_event_seq||marker.data?.receipt_id!==key||marker.data?.record_key!==key))throw Error('SIC enable marker is outside its trusted host boundary');
    if(events.some(event=>(!marker||event.seq<marker.seq)&&material(event)))throw Error(migration);
   }else{
    if(events.some(material)||marker)throw Error(migration);
    receipt={identity,receipt_id:key,source_event_seq:events.at(-1)?.seq??-1,source_digest:digest(events)};
   }
   // Verify the persisted binding through Core before writing or trusting the
   // native marker. No native model request, source import or rewrite occurs.
   const chain=await clientFor(binding).operate('chain.get',{});if(chain.chain_id!==binding.chain_id)throw Error('SIC enable boundary Core chain mismatch');
   if(resolveBinding&&!same(selected,resolveBinding(agent.session.id,agent)))throw Error('SIC session binding changed during admission');
   if(digest(agent.session.snapshotEvents())!==digest(events))throw Error('native archive changed during SIC admission');
   if(!marker){
    await mkdir(directory,{recursive:true});const temporary=path+'.'+randomUUID()+'.pending',handle=await open(temporary,'wx');
    try{
     try{await handle.writeFile(JSON.stringify(receipt)+'\n');await handle.sync();}finally{await handle.close();}
     try{await link(temporary,path);}catch(error){if(error.code!=='EEXIST')throw error;const existing=JSON.parse(await readFile(path,'utf8'));if(digest(existing)!==digest(receipt))throw Error('concurrent SIC enable boundary differs');}
    }finally{await unlink(temporary);}
   }
   if(digest(agent.session.snapshotEvents())!==digest(events))throw Error('native archive changed before SIC enable marker could be recorded');
   // Retrying an interrupted mount must flush even when the marker is already
   // present in the live Session but its previous flush was not acknowledged.
   await recordSession(agent.session,'sic/enable-boundary',{receipt_id:key},{key});
   if(ctx.agents.get(agent.session.id)!==agent)throw Error('SIC enable boundary Agent is no longer current');
   if(resolveBinding&&!same(selected,resolveBinding(agent.session.id,agent)))throw Error('SIC session binding changed during admission');
   if(agent.session.snapshotEvents().some(event=>event.seq>(events.at(-1)?.seq??-1)&&material(event)))throw Error('native history arrived before SIC admission completed');
   owners.set(agent.session.id,{agent,session:agent.session,binding:selected});
   agent.ctx.effect(()=>()=>{if(owners.get(agent.session.id)?.agent===agent)owners.delete(agent.session.id);});
  })();pending.set(agent,run);try{return await run;}finally{pending.delete(agent);}
 }
 return{admit,bindingFor};
}
