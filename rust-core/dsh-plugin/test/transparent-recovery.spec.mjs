import {dshSourcePath} from './test-paths.mjs';
import{test}from'node:test';import assert from'node:assert/strict';import{mkdtemp,rm,readdir,readFile,writeFile}from'node:fs/promises';import{tmpdir}from'node:os';import{join}from'node:path';import{pathToFileURL}from'node:url';
import{createTransparentFetch,digest}from'../transparent/transport.mjs';import{createFileProjectionStore}from'../transparent/file-store.mjs';import{createProjectionPublisher}from'../transparent/projection.mjs';import{createSessionRecorder}from'../transparent/session-records.mjs';import{nativeExchange}from'../transparent/native-boundary.mjs';
const dsh=dshSourcePath();
const sessionAPI=await import(pathToFileURL(join(dsh,'packages/core/session/lib/index.js')).href);
test('commit response loss and publication acknowledgement loss recover the original delivery once, including PVI',async()=>{
 const root=await mkdtemp(join(tmpdir(),'sic-crash-'));try{
  const original={model:'m',messages:[{role:'user',content:'before'}],stream:true},adopted={...original,messages:[{role:'user',content:'after'}]};
  const unit={unit_id:'u1',text:'before',anchor:['messages',0,'content']},delivery={ops:[{op:'COMPRESS',target:'u1',content:'after'}],pvi:{I:{decision:'keep this decision'}}};
  let state={source_id:'src',source_digest:digest(original),version:0,source_request:original,request:original,units:[unit]},editors=0,commits=0,publications=0,dispatches=0,flushes=0,flushFailure=true;
  const session=sessionAPI.Session.create('s'),store=createFileProjectionStore(root),record=await createSessionRecorder({sessions:{flush:async()=>{flushes++;if(flushFailure){flushFailure=false;throw Error('session durability failure');}return true;}}},sessionAPI);
  const publisher=createProjectionPublisher({save:store.save,getSession:()=>session,appendEvent:(s,e)=>record(s,e.type,e.data,{key:e.data.projection_id})});
  const core={begin:async()=>structuredClone(state),commit:async p=>{commits++;assert.deepEqual(p.delivery,delivery);state={...state,version:1,request:adopted,units:[{...unit,text:'after'}],receipt:{version:1},current_delivery:p.delivery,delivery_version:1};throw Error('commit response lost');},
   publish:async p=>{publications++;assert.deepEqual({ops:p.ops,pvi:p.pvi},delivery);state={...state,publication:{publication_id:'p',cb_id:'cb'}};return state;}};
  const wire=createTransparentFetch({core,resolveInvocation:async()=>({actor:'host',session_id:'s',request_id:'same-real-turn',format:'openai-chat',expected_cut:0,idempotency_key:'same-real-turn',editable_anchors:[unit.anchor]}),
   selectEdits:async()=>{editors++;return{...delivery,edits:[{unit_id:'u1',expected_text:'before',replacement:'after'}]};},admit:async()=>{},publishProjection:publisher,
   fetchImpl:async r=>{dispatches++;assert.deepEqual(await r.json(),adopted);return new Response('original-response');}});
  const send=()=>wire('https://fixture.invalid/native',{method:'POST',body:JSON.stringify(original)});
  await assert.rejects(send(),/commit response lost/);await assert.rejects(send(),/session durability failure/);assert.equal(dispatches,0);
  await send();assert.deepEqual({editors,commits,publications,dispatches,flushes},{editors:1,commits:1,publications:1,dispatches:1,flushes:2});
  assert.equal(session.snapshotEvents().filter(e=>sessionAPI.pluginRecordOf(e)?.type==='plugin:sic/native-projection').length,1);
  assert.equal((await store.list('s')).rows.length,1);
  // The actual native Session validator must accept these persisted records.
  const file=join(root,'session-roundtrip.jsonl');await writeFile(file,session.snapshotEvents().map(e=>JSON.stringify(e)).join('\n'));
  const reopened=sessionAPI.Session.create('s',(await readFile(file,'utf8')).split('\n').map(JSON.parse));assert.equal(reopened.snapshotEvents().filter(e=>sessionAPI.pluginRecordOf(e)?.type==='plugin:sic/native-projection').length,1);
 }finally{await rm(root,{recursive:true,force:true});}
});
test('a legacy committed candidate without durable delivery cannot silently rerun Editor or lose PVI',async()=>{
 let edited=false;const wire=createTransparentFetch({core:{begin:async()=>({source_id:'s',source_digest:'d',version:1,units:[]}),commit:async()=>{}},resolveInvocation:async()=>({actor:'a',session_id:'s',request_id:'r',format:'openai-chat'}),publicationRequired:false,selectEdits:async()=>{edited=true;return[];},publishProjection:async()=>{},admit:async()=>{},fetchImpl:async()=>{throw Error('must not dispatch');}});
 await assert.rejects(wire('https://fixture.invalid',{method:'POST',body:'{}'}),/explicit recovery/);assert.equal(edited,false);
});
test('final native guard cannot be removed and catches a later body replacement',async()=>{
 const bound=nativeExchange(new Request('https://fixture.invalid',{method:'POST',body:'approved'}),{});bound.exchange.addFinalGuard(async r=>{assert.equal(await r.text(),'approved');});
 assert.throws(()=>{bound.exchange.addFinalGuard=()=>{};},TypeError);bound.exchange.request=new Request('https://fixture.invalid',{method:'POST',body:'tampered'});
 await assert.rejects(bound.dispatch(()=>{throw Error('must not dispatch');}),/tampered/);
});
test('Editor chunks append to one JSONL source with ordered pages and JSON-native boundary records',async()=>{
 const root=await mkdtemp(join(tmpdir(),'sic-trace-'));try{const store=createFileProjectionStore(root);for(let i=0;i<200;i++)await store.recordTrace('s','src',{stage:'editor-chunk',text:String(i)});await store.recordTrace('s','src',{stage:'editor-response'});
 const dirs=await readdir(join(root,digest('s'),'traces'));assert.equal(dirs.length,1);assert.match(dirs[0],/\.jsonl$/);const page=await store.traces('s','src',195,5);assert.deepEqual(page.rows.map(r=>r.trace_sequence),[196,197,198,199,200]);assert.equal(page.next_offset,200);assert.equal((await createFileProjectionStore(root).traces('s','src',200,5)).rows[0].stage,'editor-response');
 }finally{await rm(root,{recursive:true,force:true});}
});


