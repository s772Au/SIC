import {fixturePath,evidencePath} from './test-paths.mjs';
import assert from 'node:assert/strict';
import {test} from 'node:test';
import {createRequire} from 'node:module';
import {readFile,writeFile} from 'node:fs/promises';
import {join} from 'node:path';
import {pathToFileURL} from 'node:url';
import {dsh,load,redImage,installNativeApi} from './native-api-fixture.mjs';
import {nativeApiServer,strings} from './native-api-server.mjs';
import {isolated} from './isolated-core-fixture.mjs';

const evidenceRoot=process.env.SIC_IMAGE_EVIDENCE_ROOT??evidencePath('image-compression-20261118');
const build=fixturePath('image-compression-fixtures-20261118');
const packageRoot=process.env.SIC_TRANSPARENT_TEST_PACKAGE;
const nativeModule=name=>import(packageRoot?pathToFileURL(join(packageRoot,name)).href:new URL('../transparent/'+name,import.meta.url));
const [{apply},{IMAGE_VIEW},{prepareSdkRuntime},{prepareDeepSeekFixture}]=await Promise.all(
  ['plugin.mjs','image-compression.mjs','prepare-sdk-runtime.mjs','prepare-deepseek-fixture.mjs'].map(nativeModule),
);
const {default:Runtime}=await import(pathToFileURL(await prepareSdkRuntime(dsh,join(build,'sdk'))).href);
const api=await load('packages/llm/llm/lib/index.js');
const sessionAPI=await load('packages/core/session/lib/index.js');
const {Context}=await load('vendor/cordis/lib/index.js');
const {createScope}=await load('packages/core/scope/lib/index.js');
const {default:SystemPrompt,renderPrompt}=await load('packages/core/system-prompt/lib/index.js');
const {default:ToolRuntime}=await load('packages/core/tools/lib/index.js');
const {default:LocalAttachmentStore}=await load('packages/attachment/attachment-local/lib/index.js');
const sharp=createRequire(join(dsh,'packages/attachment/attachment-local/package.json'))('sharp');
const bundle=name=>readFile(new URL('../transparent/bundle/'+name,import.meta.url),'utf8');

for(const protocol of ['messages','chat','responses'])test('actual '+protocol+' Adapter sends I mosaic and retains original image lineage',{timeout:60000},async()=>{
  const host=await isolated({evidenceRoot});
  let editorCount=0,step=1,recipe,editNext=true,ctx,scope,agent,installed,route,attachmentStore,handler,viewerAllowed=true;
  let lookup=null,negativeOffload=false;
  const server=await nativeApiServer({respond({wb,texts}){
    if(!wb){
      if(lookup){
        const args=lookup;lookup=null;
        return{tool:{id:'archive-'+step,name:'sic_image_source',args}};
      }
      return{text:'fixture Worker response'};
    }
    editorCount++;
    let pvi={};
    if(editNext){
      editNext=false;
      const instructions=texts.find(text=>text.includes('Optional visual compression.'));
      assert.ok(instructions?.includes(IMAGE_VIEW)&&instructions.includes(recipe.crops.at(-1).message_id));
      if(negativeOffload)assert.ok(!instructions.includes(recipe.crops[0].message_id),'offloaded source was not offered to Editor');
      pvi={I:{[IMAGE_VIEW]:recipe}};
    }
    return{tool:{id:'editor-'+editorCount,name:'sic_edit_blocks',args:{ops:[],pvi}}};
  }});
  const sessionFile=join(host.root,'native-session.jsonl');
  const system=await bundle('fused.md');
  const binding={chain_id:host.chain_id,scope_id:'vision-scope',role:'WORKER'};
  async function install(events,imageCompression=true){
    ctx=new Context();
    await ctx.plugin(Runtime);
    await ctx.plugin(SystemPrompt,{includeHarnessIdentity:false});
    await ctx.plugin(ToolRuntime);
    await ctx.plugin(LocalAttachmentStore,{dshHome:join(host.root,'dsh')});
    attachmentStore=ctx.attachments;
    ctx.systemPrompt.section({name:'native-review',order:0,text:await bundle('native.md'),interpolate:false});
    for(const definition of JSON.parse(await bundle('native-tools.json'))){
      ctx.tools.register({...definition,output:{schema:{},render:()=>[]},execute:async()=>{throw Error('fixture does not run native tasks');}});
    }
    const session=sessionAPI.Session.create('vision-session',events);
    agent={id:session.id,status:'idle',session};
    scope=createScope(ctx,agent);agent.ctx=scope.ctx;
    const facade={llm:ctx.llm,tools:ctx.tools,agents:{list:()=>[agent],get:id=>id===agent.id?agent:null,roots:()=>[agent],currentInitiator:()=>agent},
      get:name=>name==='attachments'?attachmentStore:undefined,
      jobs:{externalControlVersion:1,externalExecutorJoinVersion:1,joinExecutor:async()=>{},attachExternalLifecycle:()=>()=>{},externalLaunchKind:()=>null},
      sessions:{get:id=>id===session.id?session:null,flush:async()=>{
        await writeFile(sessionFile,session.snapshotEvents().map(event=>JSON.stringify(event)).join('\n')+'\n');
        return true;
      }},
      goals:{externalDriverVersion:1,defaultMaxGoalRounds:8,attachExternalState:()=>()=>{}},
      effect:()=>{},emit:()=>{},logger:{warn:()=>{}},sessionProjections:{stateOf:()=>({openTurnStartSeq:0,lastStepStartSeq:step})},on:(name,fn)=>ctx.on(name,fn),
      inject:(dependencies,fn)=>dependencies.some(name=>['subagents','workflowEngine','compaction'].includes(name))?{dispose:()=>{}}:fn({
        effect:body=>body(),connection:{fetch:{register:port=>{handler=port.fetch;return()=>{};}}},
      })};
    installed=await apply(facade,{enabled:true,imageCompression,imageSharp:sharp,actor:'native-http-actor',coreURL:host.baseURL,
      credential:host.credential,projectionRoot:join(host.root,'projections'),bindings:{[session.id]:binding},
      authorizeSession:()=>viewerAllowed,renderPrompt,llmAPI:api,sessionAPI,agentEvents:()=>({emit:()=>{}}),
      hierarchyPolicy:{max_level:2,level_batch_sizes:{2:2}}});
    route=await installNativeApi(ctx,{protocol,model:'vision-fixture',baseURL:server.baseURL,fetchNative:fetch,auth:()=>({'x-api-key':'owned-loopback'}),
      buildRoot:join(build,protocol),image:{store:attachmentStore},prepareDeepSeekFixture});
  }
  async function send(history){
    const call=await ctx.llm.prepareCall({provider:route.provider,model:route.model,maxTokens:1024});
    const assembler=new api.BlockAssembler();
    for await(const chunk of call.stream({...call.config,sessionId:agent.id,messages:history,tools:ctx.tools.schemas(agent)})){
      assembler.push(chunk);
      if(chunk.type==='finish')assert.notEqual(chunk.reason.kind,'error',JSON.stringify(chunk));
    }
    return assembler.message({provider:route.provider,model:route.model});
  }
  async function records(){
    const page=await installed.store.list(agent.id);
    return Promise.all(page.rows.map(row=>installed.store.get(agent.id,row.id)));
  }
  const imageBytes=body=>strings(body).filter(text=>text.startsWith('iVBOR')||text.startsWith('data:image/png;base64,'))
    .map(text=>Buffer.from(text.replace(/^data:image\/png;base64,/u,''),'base64'));
  try{
    await install();
    const coordinateRaster=Buffer.alloc(64*64*3);
    for(let y=0;y<64;y++)for(let x=0;x<64;x++)coordinateRaster.set([x*3,y*3,17],(y*64+x)*3);
    const coordinateImage=await sharp(coordinateRaster,{raw:{width:64,height:64,channels:3}}).png().toBuffer();
    const red=await attachmentStore.saveImage({data:coordinateImage,mediaType:'image/png'});
    const blue=await attachmentStore.saveImage({data:redImage([0,0,255]).data,mediaType:'image/png'});
    const historical=api.createUserMessage({source:{kind:'user'},content:[{type:'text',text:'Review the two screenshots.'},{type:'image',attachment:red}]});
    const toolUse=api.createAssistantMessage({source:{provider:route.provider,model:route.model},content:[{type:'tool-call',id:'call_vision_fixture',name:'read',arguments:'{}'}]});
    const toolImage=api.createToolResultMessage({callId:'call_vision_fixture',content:[{type:'image',attachment:blue}],isError:false});
    const runtime=api.createUserMessage({source:{kind:'runtime-context',sections:[{name:'sic:activation',text:'{"role":"WORKER","scope_id":"vision-scope"}'}]},content:[{type:'text',text:'Runtime.'}]});
    const history=[api.createSystemMessage(system),historical,toolUse,toolImage,runtime];
    recipe={crops:[{message_id:historical.id,block_index:1,x:7,y:8,width:24,height:18},{message_id:toolImage.id,block_index:0,x:5,y:4,width:20,height:12}],description:[{text:'Red and blue reference regions.',tiles:[1,2]}]};
    await send(history);
    const [first]=await records(), view=first.layout_receipt.image_view;
    assert.ok(view.active);
    assert.equal(view.lineage.originals.length,2);
    assert.equal(view.lineage.originals[1].role,'tool');
    assert.equal(view.lineage.originals[1].tool_call_id,'call_vision_fixture');
    assert.deepEqual(view.lineage.crops[0].region,{x:7,y:8,width:24,height:18});
    assert.equal(view.lineage.description.parent_id,view.lineage.mosaic.id);
    assert.deepEqual(view.lineage.description.parts[0].tiles,[1,2]);
    assert.equal(view.lineage.mosaic.tiles[0].crop_id,view.lineage.crops[0].id);
    assert.equal(view.lineage.crops[0].parent_id,view.lineage.originals[0].id);
    assert.equal(first.adopted.messages.find(message=>message.source.kind==='sic-instant-view').content.at(-1).attachment.attachmentId,view.lineage.mosaic.attachment.attachmentId);
    assert.equal(imageBytes(server.requests[0].body).length,2,'Editor received actual original images');
    const [wireMosaic]=imageBytes(server.requests.at(-1).body);
    assert.equal(imageBytes(server.requests.at(-1).body).length,1,'Worker received mosaic only');
    assert.ok(strings(server.requests.at(-1).body).includes('call_vision_fixture'),'native call/result remains linked');
    assert.deepEqual(wireMosaic,Buffer.from((await attachmentStore.readImage(view.lineage.mosaic.attachment)).data));
    const {data,info}=await sharp(wireMosaic).ensureAlpha().raw().toBuffer({resolveWithObject:true});
    for(const [tile,color] of view.lineage.mosaic.tiles.map((tile,index)=>[tile,index===0?[24,27,17,255]:[0,0,255,255]])){
      const {x,y}=tile.placement,offset=((y+1)*info.width+x+1)*4;
      assert.deepEqual([...data.subarray(offset,offset+4)],color);
    }
    const query=(assetId)=>handler(new Request('http://localhost/api/sic-core/native-projections',{method:'POST',body:JSON.stringify({sessionId:agent.id,projectionId:first.id,action:'image',assetId})}));
    for(const item of [...view.lineage.originals,...view.lineage.crops,view.lineage.mosaic]){
      const result=await query(item.id);assert.equal(result.status,200);
      assert.deepEqual(Buffer.from(await result.arrayBuffer()),Buffer.from((await attachmentStore.readImage(item.attachment)).data));
    }
    assert.equal((await query('a'.repeat(64))).status,404);
    viewerAllowed=false;assert.equal((await query(view.lineage.originals[0].id)).status,403);viewerAllowed=true;

    const next=[...history.slice(0,-1),api.createUserMessage({source:{kind:'user'},content:[{type:'text',text:'Refine the selected areas.'}]}),runtime];
    recipe={...recipe,crops:recipe.crops.map(crop=>({...crop,width:crop.width+3}))};
    step=2;editNext=true;await send(next);
    const second=(await records()).find(record=>record.id!==first.id);
    assert.notEqual(second.layout_receipt.image_view.pointer.manifest_id,view.pointer.manifest_id);
    assert.equal(imageBytes(server.requests.at(-1).body).length,1);
    assert.ok((await host.core.operate('cut.get',{})).view.cbs.some(cb=>cb.level===2));
    assert.ok(server.requests.slice(2).some(({body})=>strings(body).some(text=>text.includes('authenticated-wb-origins-v1'))&&imageBytes(body).length===1),'subsequent Editor saw actual I image');
    const durable=(await readFile(sessionFile,'utf8')).trim().split('\n').map(JSON.parse);
    await scope.dispose();scope=null;await ctx.fiber.dispose();ctx=null;
    await install(durable);
    const priorCount=editorCount;
    await send(next);
    assert.equal(editorCount,priorCount,'reopened host reused existing publication and image snapshot');
    assert.deepEqual((await records()).find(record=>record.id===first.id).layout_receipt.image_view,view);
    assert.ok(imageBytes(server.requests.at(-1).body).length===1);
    let traceHistory=[...next,api.createUserMessage({source:{kind:'user'},content:[{type:'text',text:'Inspect the evidence in the archived images.'}]})];
    const manifest_id=second.layout_receipt.image_view.pointer.manifest_id;
    async function trace(from,tile){
      step++;
      lookup={manifest_id,from,tile};
      const assistant=await send(traceHistory),call=assistant.content.find(block=>block.type==='tool-call');
      assert.equal(call?.name,'sic_image_source');
      const result=await ctx.tools.execute({agent,name:call.name,callId:call.id,arguments:JSON.parse(call.arguments),signal:new AbortController().signal});
      traceHistory=[...traceHistory,assistant,api.createToolResultMessage({callId:call.id,content:result.content,isError:result.isError})];
      return result;
    }
    assert.equal((await trace('crop',1)).isError,true,'an original cannot be read before its crop');
    const cropResult=await trace('mosaic',1);
    assert.equal(cropResult.isError,false);
    assert.equal(cropResult.content.find(block=>block.type==='image').attachment.attachmentId,second.layout_receipt.image_view.lineage.crops[0].attachment.attachmentId);
    const originalResult=await trace('crop',1);
    assert.equal(originalResult.isError,false);
    assert.equal(originalResult.content.find(block=>block.type==='image').attachment.attachmentId,red.attachmentId);
    step++;await send(traceHistory);
    const sourceImages=imageBytes(server.requests.at(-1).body);
    const archivedOriginal=Buffer.from((await attachmentStore.readImage(red)).data);
    assert.ok(sourceImages.some(bytes=>bytes.equals(archivedOriginal)),'restored original pixels reached the next actual Worker');

    step++;editNext=true;recipe={...recipe,display:'description'};
    const described=[...next,api.createUserMessage({source:{kind:'user'},content:[{type:'text',text:'Keep only the cited visual observations.'}]})];
    await send(described);
    assert.equal(imageBytes(server.requests.at(-1).body).length,0);
    assert.ok(strings(server.requests.at(-1).body).some(text=>text.includes(recipe.description[0].text)));
    const descriptionRecord=(await records()).find(record=>record.layout_receipt?.image_view?.pointer.display==='description');
    assert.equal(descriptionRecord.layout_receipt.image_view.pointer.manifest_id,second.layout_receipt.image_view.pointer.manifest_id);
    assert.ok(descriptionRecord.layout_receipt.image_view.active);
    assert.equal((await query(view.lineage.originals[0].id)).status,200,'previous projection still retrieves its original after I refresh');

    step++;editNext=true;recipe={crops:recipe.crops,description:recipe.description,display:'crops'};
    const crops=[...described,api.createUserMessage({source:{kind:'user'},content:[{type:'text',text:'Review the individual selected regions.'}]})];
    await send(crops);
    assert.equal(imageBytes(server.requests.at(-1).body).length,2);
    const cropRecord=(await records()).find(record=>record.layout_receipt?.image_view?.pointer.display==='crops');
    assert.equal(cropRecord.layout_receipt.image_view.pointer.manifest_id,second.layout_receipt.image_view.pointer.manifest_id);
    assert.ok(cropRecord.layout_receipt.image_view.active);

    step++;
    const offloaded=api.freezeMessage({...historical,content:historical.content.map((block,index)=>index===1?{...block,offloaded:true}:block)});
    const fallback=[...crops.map(message=>message.id===historical.id?offloaded:message),...traceHistory.slice(next.length+1)];
    fallback.push(api.createUserMessage({source:{kind:'user'},content:[{type:'text',text:'Continue after host image offload.'}]}));
    await send(fallback);
    const actual=server.requests.at(-1).body;
    assert.equal(imageBytes(actual).length,1,'old composite and crops do not bypass original offload');
    assert.ok(!imageBytes(actual).some(bytes=>bytes.equals(archivedOriginal)),'prior native recall does not reintroduce offloaded original bytes');
    assert.ok(strings(actual).some(text=>text.includes(api.offloadedImageText(red))));
    assert.equal((await records()).filter(record=>record.layout_receipt?.image_view?.active===false).length,1);
    step++;editNext=true;negativeOffload=true;
    const priorCut=(await host.core.operate('cut.get',{})).sequence,wireCount=server.requests.length;
    await assert.rejects(send([...fallback,api.createUserMessage({source:{kind:'user'},content:[{type:'text',text:'Try the offloaded source.'}]})]),/image crop source is outside this native assignment or offloaded/);
    assert.equal((await host.core.operate('cut.get',{})).sequence,priorCut);
    assert.equal(server.requests.length,wireCount+1,'invalid delivery never reaches Worker');

    const rollbackEvents=(await readFile(sessionFile,'utf8')).trim().split('\n').map(JSON.parse);
    await scope.dispose();scope=null;await ctx.fiber.dispose();ctx=null;
    await install(rollbackEvents,false);
    step++;
    await send(history);
    assert.equal(imageBytes(server.requests.at(-1).body).length,2,'disabled optional feature leaves native images available');

    await writeFile(join(host.root,'image-lineage.json'),JSON.stringify({protocol,packageRoot:packageRoot??'source',first:first.id,second:second.id,originals:view.lineage.originals.map(row=>row.id),first_mosaic:view.lineage.mosaic.id,second_mosaic:second.layout_receipt.image_view.lineage.mosaic.id,wire_requests:server.requests.length},null,2));
  }finally{
    if(scope)await scope.dispose();
    if(ctx)await ctx.fiber.dispose();
    await server.close();await host.close();
  }
});
