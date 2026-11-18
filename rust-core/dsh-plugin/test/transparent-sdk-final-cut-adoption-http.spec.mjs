import {dshSourcePath,fixturePath} from './test-paths.mjs';
import {test} from 'node:test';
import assert from 'node:assert/strict';
import {readFile,writeFile,open} from 'node:fs/promises';
import {join} from 'node:path';
import {pathToFileURL} from 'node:url';
import {apply} from '../transparent/plugin.mjs';
import {isolated} from './isolated-core-fixture.mjs';
import {collectedFixture} from './collection-fixture.mjs';
import {prepareSdkRuntime} from '../transparent/prepare-sdk-runtime.mjs';
import {getEditorInvocation} from '../transparent/editor-invocation.mjs';

const dsh=dshSourcePath();
const load=path=>import(pathToFileURL(join(dsh,path)).href);
const sessionAPI=await load('packages/core/session/lib/index.js');
const api=await load('packages/llm/llm/lib/index.js');
const {Context}=await load('vendor/cordis/lib/index.js');
const {default:SystemPrompt,renderPrompt}=await load('packages/core/system-prompt/lib/index.js');
const {default:ToolRuntime}=await load('packages/core/tools/lib/index.js');
const {createScope}=await load('packages/core/scope/lib/index.js');
const runtimePath=await prepareSdkRuntime(dsh,fixturePath('dsh-sdk-final-cut-fixture'));
const {default:PatchedRuntime}=await import(pathToFileURL(runtimePath).href);

for(const highOperation of ['COMPRESS','DELETE']){
 test(`first actual SDK consumption follows final higher ${highOperation} lineage before Worker dispatch and reuses that decision on retry`,{timeout:30000},async()=>{
  // isolated() copies the latest Core binary before starting its own DB/server.
  const server=await isolated(),ctx=new Context();let scope,installed;
  const seed=api.createUserMessage({source:{kind:'user'},content:[{type:'text',text:'independent seed material'}]});
  const calls={low:0,high:0,worker:0,adopt:0,publish:0},sent=[],editorAssignments=[];
  try{
   const initial=await server.core.operate('cut.get',{}),anchor=['messages',0,'content',0,'text'];
   const seeded=await server.core.begin({format:'dsh-messages',request:{messages:[seed]},editable_anchors:[anchor],
    protected_cb_bindings:[],host_message_origins:[{message_id:seed.id,block_index:0,native_anchor:anchor}],idempotency_key:'seed-before-sdk'});
   const seedDelivery={ops:[{op:'KEEP',target:seeded.binding.editable_unit_ids[0]}],pvi:{}};
   const committed=await server.core.commit({source_id:seeded.source_id,expected_source_digest:seeded.source_digest,expected_version:0,edits:[],delivery:seedDelivery});
   const seedPublication=(await server.core.publish({source_id:seeded.source_id,expected_source_digest:seeded.source_digest,
    expected_version:committed.version,expected_cut:initial.sequence,idempotency_key:'seed-low-publish',...seedDelivery})).publication;
   assert.equal((await server.core.operate('cut.get',{})).view.cb_origin_bindings[0].origins[0].message_id,seed.id);
   const collected=await collectedFixture(server.core,server.chain_id);
   assert.equal((await server.core.operate('harness.get',{scope_id:'scope',object_id:collected.id})).adopted,false);

   await ctx.plugin(SystemPrompt,{includeHarnessIdentity:false});await ctx.plugin(ToolRuntime);await ctx.plugin(PatchedRuntime);
   const native=await readFile(new URL('../transparent/bundle/native.md',import.meta.url),'utf8');
   ctx.systemPrompt.section({name:'reviewed-native-fixture',order:0,text:native,interpolate:false});
   for(const definition of JSON.parse(await readFile(new URL('../transparent/bundle/native-tools.json',import.meta.url),'utf8'))){
    ctx.tools.register({...definition,output:{schema:{},render:(_args,value)=>[{type:'text',text:JSON.stringify(value)}]},
     execute:async()=>{throw Error('the legacy executor must not execute');}});
   }
   const session=sessionAPI.Session.create('session'),agent={id:'session',status:'idle',session};
   scope=createScope(ctx,agent);agent.ctx=scope.ctx;
   class Model extends api.LlmAdapter{
    async *stream(request){
     assert.equal(request.provider,'private-final-cut');assert.equal(request.model,'native-model');
     assert.equal(request.temperature,0.3);assert.equal(request.maxTokens,128);
     if(getEditorInvocation()){
      const activation=request.messages.findLast(message=>message.source?.kind==='runtime-context').source.sections.find(section=>section.name==='sic:activation');
      const auth=JSON.parse(activation.text),wb=JSON.parse(request.messages.at(-1).content[0].text),higher=auth.level===2;
      // The collection has never been adopted through a preceding Worker call.
      assert.equal((await server.core.operate('harness.get',{scope_id:'scope',object_id:collected.id})).adopted,false);
      let ops;
      if(higher){
       calls.high++;assert.equal(wb.units.length,2);
       const seedUnit=wb.units.find(unit=>unit.text===seed.content[0].text),collectionUnit=wb.units.find(unit=>unit.text===collected.result.value.text);
       assert.ok(seedUnit);assert.ok(collectionUnit);
       ops=[{op:'KEEP',target:seedUnit.unit_id},{op:highOperation,target:collectionUnit.unit_id,
        ...highOperation==='COMPRESS'?{content:'higher collection summary'}:{}}];
      }else{
       calls.low++;assert.equal(wb.units.length,1,'only the new collected result belongs to the low WB');
       assert.equal(wb.units[0].text,collected.result.value.text);ops=[{op:'KEEP',target:wb.units[0].unit_id}];
      }
      editorAssignments.push({level:auth.level??1,source_id:auth.source_id,units:wb.units,ops});
      yield{type:'tool-call-delta',index:0,id:'delivery-'+editorAssignments.length,name:'sic_edit_blocks',argumentsDelta:JSON.stringify({ops,pvi:{}})};
      yield{type:'finish',reason:{kind:'tool-calls'}};return;
     }
     calls.worker++;
     const cut=await server.core.operate('cut.get',{}),active=request.messages.filter(message=>message.source?.kind==='sic-cb');
     assert.equal(cut.view.cbs.length,1);const high=cut.view.cbs[0];assert.equal(high.level,2);
     assert.deepEqual(active.map(message=>message.source.cb_id),[high.cb_id]);
     assert.equal(active[0].content[0].text,highOperation==='COMPRESS'?'independent seed material\nhigher collection summary':'independent seed material');
     assert.deepEqual(active.map(message=>message.content[0].text),cut.view.cbs.map(cb=>cb.body));
     const rows=await installed.store.list(session.id);assert.equal(rows.rows.length,1);
     const projection=await installed.store.get(session.id,rows.rows[0].id),decision=projection.outcomes.findLast(event=>event.stage==='collection-adoption');
     assert.ok(decision,'the persisted adoption decision precedes the original adapter');
     assert.equal(projection.layout_receipt.selected_cut_id,cut.cut_id);assert.equal(decision.adopted_cut_id,cut.cut_id);
     assert.equal(projection.publication.publication.collection_adoptions[0].eligible_current_p,true,'low publication alone is eligible in both cases');
     const state=await server.core.operate('harness.get',{scope_id:'scope',object_id:collected.id});
     assert.equal(state.adopted,highOperation==='COMPRESS');assert.equal((state.adoptions??[]).length,highOperation==='COMPRESS'?1:0);
     if(highOperation==='COMPRESS'){
      assert.equal(decision.skipped.length,0);assert.equal(decision.adoptions.length,1);
      assert.deepEqual(decision.adoptions[0].p_lineage.p_paths,[[high.cb_id,projection.publication.cb_id]]);
      assert.equal(state.adoptions[0].adopted_cut_id,cut.cut_id);assert.equal(calls.adopt,1);
     }else{
      assert.equal(decision.adoptions.length,0);assert.equal(decision.skipped.length,1);
      assert.equal(decision.skipped[0].skipped,'not_represented_in_adopted_p');
      assert.equal(decision.skipped[0].p_lineage.publication_eligible_current_p,true);
      assert.equal(decision.skipped[0].p_lineage.eligible_current_p,false);assert.equal(calls.adopt,0);
     }
     sent.push({cut,projection,state,decision,messages:request.messages});
     yield{type:'text-delta',text:'original Worker dispatched'};yield{type:'finish',reason:{kind:'stop'}};
    }
   }
   ctx.llm.registerAdapter(['private-final-cut'],new Model());
   const binding={chain_id:server.chain_id,scope_id:'scope',role:'WORKER'};
   const facade={llm:ctx.llm,tools:ctx.tools,agents:{list:()=>[agent],get:id=>id===session.id?agent:null,roots:()=>[agent],currentInitiator:()=>agent},
    jobs:{externalControlVersion:1,externalExecutorJoinVersion:1,joinExecutor:async()=>{},attachExternalLifecycle:()=>()=>{},externalLaunchKind:()=>null},get:()=>undefined,
    sessions:{get:id=>id===session.id?session:null,flush:async()=>{const handle=await open(join(server.root,'native-session.jsonl'),'w');try{await handle.writeFile(session.snapshotEvents().map(event=>JSON.stringify(event)).join('\n'));await handle.sync();}finally{await handle.close();}return true;}},
    goals:{externalDriverVersion:1,defaultMaxGoalRounds:37,attachExternalState:()=>()=>{}},effect:()=>{},emit:()=>{},logger:{warn:()=>{}},
    sessionProjections:{stateOf:()=>({openTurnStartSeq:0,lastStepStartSeq:1})},on:(name,handler)=>ctx.on(name,handler),
    inject:(dependencies,run)=>dependencies.some(name=>['subagents','workflowEngine','compaction'].includes(name))?{dispose:()=>{}}:run({effect:body=>body(),connection:{fetch:{register:()=>()=>{}}}})};
   installed=await apply(facade,{enabled:true,mode:'edit',actor:'native-http-actor',coreURL:server.baseURL,credential:server.credential,
    projectionRoot:join(server.root,'projections'),bindings:{session:binding},hierarchyPolicy:{max_level:2,level_batch_sizes:{2:2},max_host_editor_calls:8},
    renderPrompt,llmAPI:api,sessionAPI,agentEvents:()=>({emit:()=>{}}),coreFetch:async(url,init)=>{
     const envelope=JSON.parse(init.body);if(envelope.operation)assert.equal(envelope.context.allow_model,false);
     if(envelope.operation==='harness.observe'&&envelope.payload.observation.kind==='result.adopt')calls.adopt++;
     if(envelope.operation==='native.publish')calls.publish++;
     return fetch(url,init);
    }});
   const system=await readFile(new URL('../transparent/bundle/fused.md',import.meta.url),'utf8');
   const runtime=api.createUserMessage({source:{kind:'runtime-context',sections:[{name:'sic:activation',text:JSON.stringify({role:'WORKER',scope_id:'scope'})}]},content:[{type:'text',text:'trusted runtime'}]});
   const assistant=api.freezeMessage({id:'collection-caller',role:'assistant',source:{kind:'model',provider:'private-final-cut',model:'native-model'},content:[{type:'tool-call',id:'collect-call',name:'job_output',arguments:'{}'}]});
   const result=api.createToolResultMessage({callId:'collect-call',isError:false,content:[{type:'text',text:collected.result.value.text}]});
   sessionAPI.appendPluginRecord(session,'plugin:sic/jobs/collection',{...collected.proof,message_id:result.id,call_id:'collect-call',material:{block_index:0,utf8_start:0,utf8_end:Buffer.byteLength(collected.result.value.text)}});
   const prepared=await ctx.llm.prepareCall({provider:'private-final-cut',model:'native-model',temperature:0.3,maxTokens:128});
   const options={...prepared.config,sessionId:session.id,messages:[api.createSystemMessage(system),runtime,seed,assistant,result],tools:ctx.tools.schemas(agent)};
   const send=async()=>{const call=await ctx.llm.prepareCall(prepared.config);for await(const chunk of call.stream(options))if(chunk.type==='finish')assert.equal(chunk.reason.kind,'stop',JSON.stringify(chunk));};
   await send();assert.deepEqual(calls,{low:1,high:1,worker:1,adopt:highOperation==='COMPRESS'?1:0,publish:1});
   const revision=sent[0].state.revision,cutId=sent[0].cut.cut_id;
   await send();assert.deepEqual(calls,{low:1,high:1,worker:2,adopt:highOperation==='COMPRESS'?1:0,publish:1});
   assert.equal(sent[1].state.revision,revision);assert.equal(sent[1].cut.cut_id,cutId);
   assert.deepEqual(sent[1].messages,sent[0].messages);
   sessionAPI.Session.create(session.id,(await readFile(join(server.root,'native-session.jsonl'),'utf8')).split('\n').map(JSON.parse));
   await writeFile(join(server.root,'final-cut-'+highOperation.toLowerCase()+'.json'),JSON.stringify({calls,seedPublication,editorAssignments,sent},null,2));
  }finally{if(scope)await scope.dispose();await ctx.fiber.dispose();await server.close();}
 });
}
