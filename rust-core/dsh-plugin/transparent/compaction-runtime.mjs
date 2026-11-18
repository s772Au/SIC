import{digest}from'./transport.mjs';

// An admission/receipt port for the existing host scheduler. No model call,
// token policy, summary loop, or replacement surface is implemented here.
export async function installCoreCompactionRuntime(ctx,{bindingFor,schedule,appendRecord}={}){
 if(ctx.compaction?.externalCompactionVersion!==2||ctx.compaction.externalBasicVersion!==1||typeof ctx.compaction.attachExternalCompaction!=='function')throw Error('patched native BasicCompaction external port v2 required');
 if(typeof bindingFor!=='function'||typeof schedule!=='function')throw Error('trusted existing SIC compaction scheduler binding required');
 appendRecord??=(await import('@deepseek-ai/dsh-session')).appendPluginRecord;
 const active=new Map(),disposers=[],mounted=new Set(),owners=new WeakMap();let closing=false;
 const binding=agent=>{const current=bindingFor(agent.session.id,agent),previous=owners.get(agent);if(previous&&(!current||previous.chain_id!==current.chain_id||previous.scope_id!==current.scope_id))throw Error('SIC compaction binding changed or was removed; original history is preserved');if(current&&!previous)owners.set(agent,{chain_id:current.chain_id,scope_id:current.scope_id});return current;},selected=session=>{const agent=ctx.agents.get(session.id);return!!agent&&agent.session===session&&!!binding(agent);};
 function mountPruner(pruner){if(closing||mounted.has(pruner))return;if(pruner.externalPruneVersion!==2||typeof pruner.attachExternalPrune!=='function'||typeof pruner.protectExternalSession!=='function')throw Error('patched native tool-result-pruner external guard v2 required');mounted.add(pruner);disposers.push(pruner.attachExternalPrune({handles:selected}));for(const agent of ctx.agents.list())if(selected(agent.session))pruner.protectExternalSession(agent.session);}
 if(ctx.get('toolResultPruner'))mountPruner(ctx.get('toolResultPruner'));
 const injection=ctx.inject(['toolResultPruner'],child=>mountPruner(child.toolResultPruner));disposers.push(()=>injection.dispose());
 disposers.push(ctx.compaction.attachExternalCompaction({handles:agent=>selected(agent.session),async compact(request){
  const{agent,signal,trigger,sourceCommandId,range}=request,b={...binding(agent)};
  if(closing||ctx.agents.get(agent.session.id)!==agent||!b.chain_id||!b.scope_id||typeof agent.resolveContextEditCall!=='function')throw Error('exact selected native compaction Agent/chain/scope and original request resolver required');
  signal.throwIfAborted();
  if(trigger==='region')throw Error('native surface region has no authorized SIC CB range mapping; no compaction was dispatched and original history is preserved');
  const source_event_seq=agent.session.snapshotEvents().findLast(event=>!event.type.startsWith('plugin:'))?.seq??null;
  const request_id='dsh-compaction:'+digest({session_id:agent.session.id,scope_id:b.scope_id,chain_id:b.chain_id,trigger,source_event_seq,sourceCommandId:sourceCommandId??null,range:range??null});
  if(active.has(request_id))return active.get(request_id);
  const run=(async()=>{
   const result=structuredClone(await schedule({agent,binding:b,request_id,trigger,signal,source_event_seq,resolveCall:()=>agent.resolveContextEditCall(signal),...sourceCommandId===undefined?{}:{sourceCommandId},...range===undefined?{}:{range:{...range}}}));
   signal.throwIfAborted();
   const current=binding(agent);if(ctx.agents.get(agent.session.id)!==agent||!current||current.chain_id!==b.chain_id||current.scope_id!==b.scope_id)throw Error('SIC compaction binding is stale; no native replacement was made');
   if(result?.recorded!==true||typeof result.receipt_id!=='string'||!result.receipt_id||typeof result.progressed!=='boolean'||(result.publication_ids!==undefined&&(!Array.isArray(result.publication_ids)||result.publication_ids.some(id=>typeof id!=='string'||!id))))throw Error('SIC compaction scheduler did not return a durable host receipt');
   const external={recorded:true,request_id,receipt_id:result.receipt_id,progressed:result.progressed,publication_ids:result.publication_ids??[]};
   appendRecord(agent.session,'plugin:sic/compaction/receipt',{...external,trigger,source_event_seq,...sourceCommandId===undefined?{}:{sourceCommandId}});
   if(await ctx.sessions.flush(agent.session)!==true)throw Error('native compaction receipt was not durably flushed');signal.throwIfAborted();
   return{external,text:typeof result.text==='string'&&result.text?result.text:result.progressed?'SIC context compaction committed.':trigger==='region'?'Native region has no authorized SIC CB mapping; original history is preserved.':'No SIC context compaction was needed.'};
  })();active.set(request_id,run);try{return await run;}finally{if(active.get(request_id)===run)active.delete(request_id);}
 }}));
 const protect=agent=>{if(selected(agent.session)){ctx.compaction.externalCompactionFor(agent);for(const pruner of mounted)pruner.protectExternalSession(agent.session);}};
 for(const agent of ctx.agents.list())protect(agent);disposers.push(ctx.on('agent/created',({agent})=>protect(agent)));
 let disposal;const dispose=()=>disposal??=(async()=>{closing=true;try{await Promise.allSettled([...active.values()]);}finally{for(const release of disposers.reverse())await release();}})();ctx.effect(()=>dispose);
 return{dispose,drain:async()=>{await Promise.all([...active.values()]);}};
}
