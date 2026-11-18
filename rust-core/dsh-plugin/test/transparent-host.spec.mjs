import {test} from 'node:test';
import assert from 'node:assert/strict';
import {createTransparentFetch,digest,validateProjection} from '../transparent/transport.mjs';
import {createProjectionPublisher,createNativeProjectionHandler} from '../transparent/projection.mjs';
import {createFileProjectionStore} from '../transparent/file-store.mjs';
import {createCoreClient} from '../transparent/core-client.mjs';
import {mkdtemp,rm} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import {applyRawTextEdits} from '../transparent/native-json.mjs';

function fixture(body={model:'host-model',messages:[{role:'user',content:'before'}],stream:true,n:2,unknown:{opaque:'sig'}}){
  const units=[{unit_id:'u1',text:'before',anchor:['messages',0,'content']}];
  let request,dispatches=0,admitted=0,published=0;
  const response=new Response(new ReadableStream({start(c){c.enqueue(new TextEncoder().encode('data: first\n\n'));}}),{headers:{'content-type':'text/event-stream'}});
  const core={begin:async p=>({source_id:'native-1',source_digest:digest(p.request),version:0,units}),commit:async p=>{
    const next=structuredClone(body);next.messages[0].content=p.edits[0]?.replacement??'before';
    return{receipt:{source_digest:digest(body),version:1},request:next};
  }};
  const options={publicationRequired:false,core,resolveInvocation:async()=>({actor:'a',session_id:'s',format:'openai-chat',request_id:'r',role:'WORKER'}),
    selectEdits:async()=>[{unit_id:'u1',expected_text:'before',replacement:'after'}],
    admit:async({request:r})=>{assert.equal(r.messages[0].content,'after');admitted++;},
    publishProjection:async p=>{assert.equal(admitted,1);published++;return{recorded:true,adopted_digest:p.adopted_digest};},
    fetchImpl:async r=>{request=r;dispatches++;assert.equal(published,1);return response;}};
  return{body,units,core,options,response,read:()=>({request,dispatches,admitted,published})};
}

test('native wire edit preserves route, auth, n, stream, opaque extensions and returns live response',async()=>{
  const f=fixture();
  const response=await createTransparentFetch(f.options)('https://provider.example/native',{method:'POST',headers:{authorization:'fixture-only','content-type':'application/json'},body:JSON.stringify(f.body)});
  assert.equal(response,f.response);
  assert.equal(response.bodyUsed,false);
  const sent=f.read().request;
  assert.equal(sent.url,'https://provider.example/native');assert.equal(sent.headers.get('authorization'),'fixture-only');
  assert.equal(sent.headers.has('content-length'),false);
  assert.deepEqual(await sent.json(),{...f.body,messages:[{role:'user',content:'after'}]});
  const reader=response.body.getReader();const first=await reader.read();
  assert.match(new TextDecoder().decode(first.value),/first/);await reader.cancel();
});

test('post-dispatch telemetry cannot replace the original live response or failure',async()=>{
  for(const failed of [false,true]){
    const f=fixture(),failure=new Error('original transport failure'),observed=[];
    f.options.observeDispatch=async()=>{throw Error('telemetry unavailable');};
    f.options.onObservationError=async(error,event)=>{observed.push(event.stage);};
    if(failed)f.options.fetchImpl=async()=>{throw failure;};
    const run=createTransparentFetch(f.options)('https://provider.example/native',{method:'POST',body:JSON.stringify(f.body)});
    if(failed)await assert.rejects(run,error=>error===failure);
    else{assert.equal(await run,f.response);assert.equal(f.response.bodyUsed,false);}
    assert.deepEqual(observed,[failed?'dispatch-failed':'response-returned']);
    await f.response.body.cancel();
  }
});

test('raw native body preserves opaque numeric lexemes, signatures, whitespace and unknown blocks',()=>{
  const raw='{ "model":"m", "messages":[{"role":"user","content":"before"}], "opaque":{"integer":900719925474099312345,"signed":"\\u0061"},"stream":true,"n":2 }';
  const units=[{unit_id:'u',text:'before',anchor:['messages',0,'content']}],edits=[{unit_id:'u',expected_text:'before',replacement:'after\nline'}];
  assert.equal(applyRawTextEdits(raw,units,edits),raw.replace('"before"','"after\\nline"'));
  assert.throws(()=>applyRawTextEdits('{"text":"before","text":"before"}',[{unit_id:'u',text:'before',anchor:['text']}],edits),/ambiguous/);
});

test('published begin receipt reuses adopted request and never reruns Editor or candidate mutation',async()=>{
  const f=fixture();let editor=0;f.options.publicationRequired=true;
  f.options.resolveInvocation=async()=>({actor:'a',session_id:'s',request_id:'same-host-request',format:'openai-chat',expected_cut:0,idempotency_key:'same-key'});
  f.core.begin=async()=>({source_id:'native-1',source_digest:digest(f.body),version:1,source_request:f.body,
    units:[{...f.units[0],text:'after'}],request:{...f.body,messages:[{role:'user',content:'after'}]},receipt:{version:1},publication:{publication_id:'p',cb_id:'cb'}});
  f.core.commit=async()=>{throw Error('must not commit twice');};f.core.publish=async()=>{throw Error('must not publish twice');};
  f.options.selectEdits=async()=>{editor++;return[];};
  await createTransparentFetch(f.options)('https://fixture.invalid/native',{method:'POST',body:JSON.stringify(f.body)});
  assert.equal(editor,0);assert.equal(f.read().dispatches,1);await f.response.body.cancel();
});

test('native Responses and Anthropic anchors edit without protocol normalization',()=>{
  for(const [raw,anchor] of [
    ['{"input":[{"type":"message","content":[{"type":"input_text","text":"before"}]}],"previous_response_id":"prior","stream":false,"opaque":{"signature":"same"}}',['input',0,'content',0,'text']],
    ['{"messages":[{"role":"user","content":[{"type":"text","text":"before"}]}],"max_tokens":8,"thinking":{"type":"enabled","budget_tokens":4},"opaque":{"signature":"same"}}',['messages',0,'content',0,'text']],
  ])assert.equal(applyRawTextEdits(raw,[{unit_id:'u',text:'before',anchor}],[{unit_id:'u',expected_text:'before',replacement:'after'}]),raw.replace('"before"','"after"'));
});

test('native begin receives the authenticated origin of a relocated opaque block',async()=>{
  const f=fixture();
  const media={type:'input_image',image_url:'https://media.example/fixture.png'};
  const request={input:[{role:'user',content:[media]}],stream:true};
  const origins=[{message_id:'original-message',block_index:3,native_anchor:['input',0,'content',0]}];
  f.options.resolveInvocation=async()=>({actor:'a',session_id:'s',request_id:'r',format:'openai-responses',editable_anchors:[],host_opaque_origins:origins});
  f.core.beginRaw=async(format,raw,_signal,scope)=>{
    assert.equal(format,'openai-responses');assert.deepEqual(JSON.parse(raw),request);
    assert.deepEqual(scope.host_opaque_origins,origins);
    return {source_id:'native-media',source_digest:digest(request),version:0,units:[]};
  };
  f.options.selectEdits=async()=>{throw Error('readonly media did not enter an editable scope');};
  f.options.admit=async({request:adopted})=>assert.deepEqual(adopted,request);
  f.options.publishProjection=async p=>({recorded:true,adopted_digest:p.adopted_digest});
  f.options.fetchImpl=async sent=>{assert.deepEqual(await sent.json(),request);return f.response;};
  await createTransparentFetch(f.options)('https://fixture.invalid/responses',{method:'POST',body:JSON.stringify(request)});
  await f.response.body.cancel();
});

test('file projection persists across readers and is isolated by session',async()=>{
  const root=await mkdtemp(join(tmpdir(),'sic-native-projection-'));
  try{
    const adopted={input:[{type:'message',content:'edited'}]},record={id:'p1',session_id:'session-a',request_id:'r',source_id:'src',adopted,adopted_digest:digest(adopted)};
    await createFileProjectionStore(root).save(record);
    const restarted=createFileProjectionStore(root);
    assert.deepEqual(await restarted.get('session-a','p1'),record);
    assert.equal(await restarted.get('session-b','p1'),null);
    assert.equal((await restarted.list('session-a')).rows[0].id,'p1');
    await assert.rejects(restarted.save({...record,adopted:{changed:true},adopted_digest:digest({changed:true})}),/EEXIST/);
    await assert.rejects(restarted.get('session-a','../escape'),/invalid projection/);
  }finally{await rm(root,{recursive:true,force:true});}
});

test('Core client uses actual operation envelope and unwraps result',async()=>{
  const client=createCoreClient({baseURL:'http://127.0.0.1:18890',credential:()=> 'fixture-core-only',context:()=>({actor:'host',chain_id:null,allow_write:true,allow_model:false,allow_revoke:false}),
    fetchImpl:async(url,init)=>{
      assert.equal(String(url),'http://127.0.0.1:18890/api/v2/operations');assert.equal(init.headers.authorization,'Bearer fixture-core-only');
      const b=JSON.parse(init.body);assert.equal(b.operation,'native.begin');assert.equal(b.context.actor,'host');assert.equal(b.payload.format,'openai-responses');
      assert.equal(JSON.stringify(b).includes('fixture-core-only'),false);return Response.json({result:{source_id:'native-src',version:0}});
    }});
  assert.equal((await client.begin({format:'openai-responses',request:{input:'x'}})).source_id,'native-src');
});

test('final admission and projection acknowledgement fail closed before Provider dispatch',async()=>{
  for(const key of ['admit','publishProjection']){
    const f=fixture();f.options[key]=async()=>{throw Error('fixture rejection');};
    await assert.rejects(createTransparentFetch(f.options)('https://provider.example/native',{method:'POST',body:JSON.stringify(f.body)}),/fixture rejection/);
    assert.equal(f.read().dispatches,0);await f.response.body.cancel();
  }
});

test('out-of-scope mutations, duplicate edit and stale text cannot be dispatched',()=>{
  const f=fixture(),after=structuredClone(f.body);after.messages[0].content='after';after.model='other';
  assert.throws(()=>validateProjection(f.body,after,f.units,[{unit_id:'u1',expected_text:'before',replacement:'after'}]),/outside authorized/);
  assert.throws(()=>validateProjection(f.body,f.body,f.units,[{unit_id:'u1',expected_text:'wrong',replacement:'after'}]),/text conflict/);
  const e={unit_id:'u1',expected_text:'before',replacement:'before'};
  assert.throws(()=>validateProjection(f.body,f.body,f.units,[e,e]),/duplicate/);
  void f.response.body.cancel();
});

test('projection is durably saved and correlated to the native session before ack; reads enforce session ACL',async()=>{
  const records=new Map(),events=[];
  const publish=createProjectionPublisher({save:async r=>records.set(r.id,r),getSession:id=>({id}),appendEvent:async(s,e)=>{assert.equal(records.size,1);events.push(e);}});
  const adopted={model:'m',messages:[]};await publish({id:'p',session_id:'s',request_id:'r',source_id:'src',format:'openai-chat',adopted,adopted_digest:digest(adopted),stage:'admitted'});
  assert.equal(events[0].data.projection_id,'p');
  const handler=createNativeProjectionHandler({authorize:async(req,s)=>s==='s',list:async()=>[...records.values()],get:async(s,id)=>records.get(id)?.session_id===s?records.get(id):null});
  const call=b=>handler(new Request('http://host/api/sic-core/native-projections',{method:'POST',body:JSON.stringify(b)}));
  assert.equal((await call({sessionId:'other',action:'get',projectionId:'p'})).status,403);
  assert.deepEqual((await(await call({sessionId:'s',action:'get',projectionId:'p'})).json()).value.adopted,adopted);
});
