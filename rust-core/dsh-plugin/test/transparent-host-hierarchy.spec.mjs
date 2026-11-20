import {dshSourcePath} from './test-paths.mjs';
import {test} from 'node:test';
import assert from 'node:assert/strict';
import {readFile,writeFile} from 'node:fs/promises';
import {join} from 'node:path';
import {pathToFileURL} from 'node:url';
import {isolated} from './isolated-core-fixture.mjs';
import {createHostHierarchyRuntime} from '../transparent/host-hierarchy.mjs';
import {createFileProjectionStore} from '../transparent/file-store.mjs';
import {createProjectionPublisher} from '../transparent/projection.mjs';
import {createSessionRecorder} from '../transparent/session-records.mjs';
import {getEditorInvocation} from '../transparent/editor-invocation.mjs';
const dsh=dshSourcePath();
const load=relative=>import(pathToFileURL(join(dsh,relative)).href);
const {Context}=await load('vendor/cordis/lib/index.js');
const api=await load('packages/llm/llm/lib/index.js');
const sessionAPI=await load('packages/core/session/lib/index.js');
const system=await readFile(new URL('../transparent/bundle/fused.md',import.meta.url),'utf8');
const tools=[...JSON.parse(await readFile(new URL('../transparent/bundle/native-tools.json',import.meta.url),'utf8')),
  ...JSON.parse(await readFile(new URL('../transparent/bundle/sic-extra-tools.json',import.meta.url),'utf8'))];

async function setup(){
  const server=await isolated(),ctx=new Context();await ctx.plugin(api.default);
  const session=sessionAPI.Session.create('hierarchy-session'),agent={id:session.id,session,status:'idle'},messages=[],requests=[];
  const binding={chain_id:server.chain_id,scope_id:'hierarchy-scope',role:'WORKER'};let behavior='keep';
  class Model extends api.LlmAdapter{
    async *stream(request){
      assert.ok(getEditorInvocation());assert.equal(request.provider,'original-route');assert.equal(request.model,'original-model');
      assert.equal(request.temperature,0.25);assert.equal(request.maxTokens,256);assert.deepEqual(request.tools,tools);
      assert.deepEqual(request.vendorOpaque,{choices:2,stream:true,signed:'unchanged'});
      const cut=await server.core.operate('cut.get',{}),prefix=request.messages.filter(message=>message.source?.kind==='sic-cb');
      assert.deepEqual(prefix.map(message=>message.content[0].text),cut.view.cbs.map(cb=>cb.body));
      requests.push(JSON.parse(JSON.stringify(request)));
      if(behavior==='error')throw Error('original Harness adapter interrupted');
      if(behavior==='stale')await server.core.operate('source.ingest',{request:{messages:[{role:'user',content:'arrived after the frozen Editor request'}]}});
      const wb=JSON.parse(request.messages.at(-1).content[0].text);
      yield {type:'tool-call-delta',index:0,id:'higher-'+requests.length,name:'sic_edit_blocks',argumentsDelta:JSON.stringify({ops:wb.units.map(unit=>({op:'KEEP',target:unit.unit_id})),pvi:{I:{higher:'actual Harness Editor'}}})};
      yield {type:'finish',reason:{kind:'tool-calls'}};
    }
  }
  ctx.llm.registerAdapter(['original-route'],new Model());
  const facade={llm:ctx.llm,agents:{get:id=>id===agent.id?agent:null},tools:{schemas:()=>tools},sessions:{flush:async()=>true}};
  const store=createFileProjectionStore(server.root),recordSession=await createSessionRecorder(facade,sessionAPI);
  const publisher=createProjectionPublisher({save:store.save,getSession:id=>id===session.id?session:null,
    appendEvent:(current,event)=>recordSession(current,event.type,event.data,{key:event.data.projection_id})});
  const runtime=createHostHierarchyRuntime({ctx:facade,cfg:{actor:'native-http-actor',hierarchyPolicy:{max_level:3,level_batch_sizes:{2:2,3:2}}},
    api,system,store,clientFor:()=>server.core,bindingFor:()=>binding,recordSession,publisher});
  async function seed(index){
    const message=api.createUserMessage({source:{kind:'user'},content:[{type:'text',text:'fact-'+index}]}),cut=await server.core.operate('cut.get',{});
    messages.push(message);
    const recalled=cut.view.cbs.map(cb=>api.createUserMessage({source:{kind:'sic-cb'},content:[{type:'text',text:cb.body}]}));
    const request={provider:'original-route',model:'original-model',messages:[...recalled,message]};
    const originalMessageIndex=recalled.length;
    const begun=await server.core.begin({format:'dsh-messages',request,idempotency_key:'seed-'+index,
      editable_anchors:[['messages',originalMessageIndex,'content',0,'text']],
      protected_cb_bindings:cut.view.cbs.map((cb,i)=>({cb_id:cb.cb_id,native_anchors:[['messages',i,'content',0,'text']]})),
      host_message_origins:request.messages.map((message,i)=>({message_id:message.id,block_index:0,native_anchor:['messages',i,'content',0,'text']}))});
    await server.core.commit({source_id:begun.source_id,expected_source_digest:begun.source_digest,expected_version:0,edits:[],delivery:{ops:[],pvi:{}}});
    return server.core.publish({source_id:begun.source_id,expected_source_digest:begun.source_digest,expected_version:1,expected_cut:cut.sequence,idempotency_key:'seed-pub-'+index});
  }
  function options(){return {provider:'original-route',model:'original-model',temperature:0.25,maxTokens:256,tools,sessionId:session.id,vendorOpaque:{choices:2,stream:true,signed:'unchanged'},
    messages:[api.freezeMessage({id:'frozen-system',role:'system',source:{kind:'system-prompt'},content:[{type:'text',text:system}]}),
      api.freezeMessage({id:'runtime',role:'user',source:{kind:'runtime-context',form:'snapshot',sections:[]},content:[{type:'text',text:'runtime'}]}),...messages]};}
  return {...server,ctx,agent,runtime,store,requests,seed,options,behavior:value=>{behavior=value;},close:async()=>{await ctx.fiber.dispose();await server.close();}};
}

test('real Rust host plans and original Harness Editor grow through three levels, retain the tail, and replay without another model call',{timeout:60000},async()=>{
  const f=await setup();try{
    const runs=[];
    for(let index=0;index<13;index++){
      await f.seed(index);
      const run=await f.runtime.drainHigher({agent:f.agent,core:f.core,options:f.options(),requestId:'step-'+index});runs.push(run);
      const cut=await f.core.operate('cut.get',{});assert.equal(run.selected_sequence,cut.sequence);
      assert.ok(run.reports.every(report=>report.model_calls_observed===0&&report.editor_mode==='host'));
    }
    const chain=await f.core.operate('chain.get',{}),cut=await f.core.operate('cut.get',{});
    assert.equal(chain.role_views.worker.level,3);assert.equal(chain.role_views.editor.level,3);
    assert.ok(chain.contexts.main.levels['3'].cbs.length>=3);
    const text=cut.view.cbs.map(cb=>cb.body).join('\n');for(let index=0;index<13;index++)assert.ok(text.includes('fact-'+index));
    const before=f.requests.length,retry=await f.runtime.drainHigher({agent:f.agent,core:f.core,options:f.options(),requestId:'step-12'});
    assert.deepEqual(retry,runs.at(-1));assert.equal(f.requests.length,before);
    assert.ok((await f.store.list(f.agent.session.id,0,100)).rows.some(row=>row.stage==='hierarchy-published-awaiting-dispatch'));
    const reopened=createHostHierarchyRuntime({ctx:{llm:f.ctx.llm,agents:{get:()=>f.agent},tools:{schemas:()=>tools}},cfg:{actor:'native-http-actor'},api,system,
      store:createFileProjectionStore(f.root),clientFor:()=>f.core,bindingFor:()=>({chain_id:f.chain_id,scope_id:'hierarchy-scope'}),recordSession:async()=>{},publisher:async()=>({recorded:true})});
    await reopened.drainHigher({agent:f.agent,core:f.core,options:f.options(),requestId:'step-12'});assert.equal(f.requests.length,before);
    const owner=f.chain_id;
    await f.core.operate('board.create',{acl:{readers:[owner],writers:[owner],claimers:[owner],managers:[owner]}});
    const manual=await f.runtime.scheduleCompaction({agent:f.agent,options:f.options(),request_id:'board-compact',trigger:'manual'});
    assert.equal(manual.recorded,true);assert.equal(manual.progressed,true);
    const manualRow=(await f.store.list(f.agent.session.id,0,100)).rows.find(row=>row.request_id==='board-compact');
    const manualRecord=await f.store.get(f.agent.session.id,manualRow.id);assert.equal(manualRecord.source_kind,'host-hierarchy');
    await f.runtime.authorizeRecord(manualRecord,f.core);
    await f.core.operate('board.unbind',{});
    await assert.rejects(f.runtime.authorizeRecord(manualRecord,f.core),/no longer bound/);
    await writeFile(join(f.root,'hierarchy-native-requests.json'),JSON.stringify({runs,requests:f.requests,selected:cut.view.cbs},null,2));
  }finally{await f.close();}
});

test('interrupted or stale actual Harness higher Editor is preserved as unknown and never automatically dispatched again',{timeout:30000},async()=>{
  for(const behavior of ['error','stale']){
    const f=await setup();try{
      await f.seed(0);await f.seed(1);const cut=await f.core.operate('cut.get',{});f.behavior(behavior);
      await assert.rejects(f.runtime.drainHigher({agent:f.agent,core:f.core,options:f.options(),requestId:'failed'}));
      assert.equal(f.requests.length,1);assert.equal((await f.core.operate('cut.get',{})).sequence,cut.sequence);
      const state=await f.core.operate('chain.get',{}),job=Object.values(state.jobs).find(job=>job.host_editor?.runner==='host');
      assert.equal(job.status,'failed');assert.equal(job.host_editor.attempts['editor-1'].status,'unknown');
      await assert.rejects(f.runtime.drainHigher({agent:f.agent,core:f.core,options:f.options(),requestId:'explicit-next-request'}));assert.equal(f.requests.length,1);
    }finally{await f.close();}
  }
});

test('an existing higher V interval is retained while Worker may obtain the next fresh lower interval',{timeout:15000},async()=>{
  const f=await setup();try{
    await f.seed(0);await f.seed(1);
    const cut=await f.core.operate('cut.get',{}),formed=await f.core.operate('hierarchy.form',{level:2,cb_ids:cut.view.cbs.map(cb=>cb.cb_id)});
    const job=await f.core.operate('edit.host.prepare',{source_id:formed.source_id});
    await f.core.operate('edit.host.claim',{job_id:job.job_id,editor:'editor-1',claim_id:'explicit-higher-v'});
    await f.core.operate('edit.host.submit',{job_id:job.job_id,editor:'editor-1',claim_id:'explicit-higher-v',ops:[],pvi:{V:['preserve this higher-level volatile source']}});
    const receipt=await f.runtime.drainHigher({agent:f.agent,core:f.core,options:f.options(),requestId:'retain-higher-v'});
    assert.equal(receipt.recorded,true);assert.ok(receipt.reports.some(report=>report.wait?.code==='higher_volatile_input'));
    const chain=await f.core.operate('chain.get',{});assert.equal(chain.contexts.main.levels['2'].pending.length,1);assert.equal(f.requests.length,0);
  }finally{await f.close();}
});
