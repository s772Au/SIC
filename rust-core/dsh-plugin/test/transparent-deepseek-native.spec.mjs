import {dshSourcePath,fixturePath} from './test-paths.mjs';
import {test} from 'node:test';
import assert from 'node:assert/strict';
import {join} from 'node:path';
import {pathToFileURL} from 'node:url';
import {prepareDeepSeekFixture} from '../transparent/prepare-deepseek-fixture.mjs';
import {createTransparentFetch,digest} from '../transparent/transport.mjs';

const dsh=dshSourcePath();
const build=process.env.SIC_HOST_FIXTURE_ROOT??fixturePath('dsh-transparent-fixture-20261117');
const source=await prepareDeepSeekFixture(dsh,build);
const {DeepSeekAdapter,resolveAdapterOptions}=await import(pathToFileURL(source).href);
const {Context}=await import(pathToFileURL(join(dsh,'vendor/cordis/lib/index.js')).href);
const {default:LlmRuntime,createUserMessage}=await import(pathToFileURL(join(dsh,'packages/llm/llm/lib/index.js')).href);

test('actual DSH DeepSeek Messages serializer/adapter uses optional native fetch seam and retains original prepared route',async()=>{
  let original,adopted,projection,sentHeaders;const order=[];
  const wire=createTransparentFetch({
    core:{begin:async({format,request})=>{assert.equal(format,'anthropic-messages');original=request;return{source_id:'src',source_digest:digest(request),version:0,units:[{unit_id:'u1',text:request.messages[0].content[0].text,anchor:['messages',0,'content',0,'text']}]};},
      commit:async p=>{adopted=structuredClone(original);adopted.messages[0].content[0].text=p.edits[0].replacement;return{receipt:{version:1},version:1,request:adopted};},publish:async p=>({version:1,request:adopted,publication:{publication_id:"pub-real-core-shape",cb_id:"cb-1",publication:{wb_id:"wb-1",cut_id:"cut-1"}}})},
    resolveInvocation:async request=>({actor:'fixture',session_id:request.headers.get('x-deepseek-harness-session-id'),request_id:'r',format:'anthropic-messages',role:'WORKER',expected_cut:0,idempotency_key:'fixture-native-publish'}),
    selectEdits:async()=>[{unit_id:'u1',expected_text:'before',replacement:'after'}],
    admit:async({request})=>{order.push('admit');assert.equal(request.model,'deepseek-v4-flash');},
    publishProjection:async p=>{order.push('projection');projection=p;return{recorded:true,adopted_digest:p.adopted_digest};},
    fetchImpl:async request=>{
      order.push('dispatch');assert.equal(request.url,'https://fixture.invalid/anthropic/v1/messages');sentHeaders=request.headers;
      assert.deepEqual(await request.json(),adopted);assert.equal(adopted.messages[0].content[0].text,'after');assert.equal(adopted.opaque_extension.signature,'keep-opaque');
      const events=[{type:'message_start',message:{id:'msg_fixture',model:'deepseek-v4-flash',usage:{input_tokens:10,output_tokens:0}}},
        {type:'content_block_start',index:0,content_block:{type:'text',text:''}},
        {type:'content_block_delta',index:0,delta:{type:'text_delta',text:'native success'}},
        {type:'content_block_stop',index:0},{type:'message_delta',delta:{stop_reason:'end_turn'},usage:{output_tokens:2}},{type:'message_stop'}];
      return new Response(events.map(e=>`event: ${e.type}\ndata: ${JSON.stringify(e)}\n\n`).join(''),{headers:{'content-type':'text/event-stream'}});
    },
  });
  const adapter=new DeepSeekAdapter({fetch:wire,options:()=>resolveAdapterOptions({baseURL:'https://fixture.invalid/anthropic'}),
    resolveAuth:async()=>({headers:{'x-api-key':'fixture-provider-only'}}),resolveUserId:()=> 'fixture-user',
    resolveAttachments:()=>undefined,resolveImageAccess:()=>undefined,
    prepareExtensions:async()=>({fields:{opaque_extension:{signature:'keep-opaque'}},accept:async()=>{}})});
  const ctx=new Context();
  try{
    await ctx.plugin(LlmRuntime);ctx.llm.registerAdapter(['original-deepseek'],adapter);
    const prepared=await ctx.llm.prepareCall({provider:'original-deepseek',model:'deepseek-v4-flash'});
    const chunks=[];for await(const c of prepared.stream({...prepared.config,sessionId:'fixture-session',messages:[createUserMessage({source:{kind:'user'},content:[{type:'text',text:'before'}]})]}))chunks.push(c);
    assert.deepEqual(order,['admit','projection','dispatch']);assert.equal(projection.original.messages[0].content[0].text,'before');
    assert.equal(sentHeaders.get('x-api-key'),'fixture-provider-only');assert.equal(projection.session_id,'fixture-session');
    assert.equal(chunks.find(c=>c.type==='text-delta')?.text,'native success');assert.equal(chunks.at(-1).reason.kind,'stop');
  }finally{await ctx.fiber.dispose();}
});
