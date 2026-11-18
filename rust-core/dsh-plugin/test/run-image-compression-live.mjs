import {fixturePath,evidencePath} from './test-paths.mjs';
import assert from 'node:assert/strict';
import {createHash} from 'node:crypto';
import {createRequire} from 'node:module';
import {readFile,writeFile} from 'node:fs/promises';
import {join} from 'node:path';
import {pathToFileURL} from 'node:url';
import {autoDL,paidRowUpperCost,reservePaidRequest} from './autodl-live-support.mjs';
import {dsh,load,installNativeApi} from './native-api-fixture.mjs';
import {strings} from './native-api-server.mjs';
import {isolated} from './isolated-core-fixture.mjs';
import {imageCases,renderCase,score,coverage,assertNoAnswerInText} from './image-live-cases.mjs';

if(!process.argv.includes('--live'))throw Error('explicit --live required');
const packageRoot=process.env.SIC_TRANSPARENT_LIVE_PACKAGE,core=process.env.SIC_CORE_TEST_BIN;
if(!packageRoot||!core)throw Error('frozen plugin and Core paths required');
const baselineEvidence=evidencePath('image-production-acceptance-20261118/live');
const finalMode=process.argv.includes('--final'),maxRequests=finalMode?40:60;
const evidenceRoot=finalMode?join(baselineEvidence,'final'):baselineEvidence;
const build=fixturePath('image-paid-fixtures-20261118');
const lineageOnly=process.argv.includes('--lineage-only');
const ids=process.argv.find(arg=>arg.startsWith('--cases='))?.slice(8).split(',')??(lineageOnly?['relay']:imageCases.map(row=>row.key));
assert.ok(ids.every(id=>imageCases.some(row=>row.key===id)),'unknown case');
const config=await autoDL({maxRequests,evidenceDirectory:evidenceRoot});
const baselineCost=finalMode?JSON.parse(await readFile(join(baselineEvidence,'paid-request-ledger.json'),'utf8')).requests
 .reduce((sum,row)=>sum+paidRowUpperCost(row),0):0;
const started=new Date().toISOString().replaceAll(':','-');
const native=name=>import(pathToFileURL(join(packageRoot,name)).href);
const bundle=name=>readFile(join(packageRoot,'bundle',name),'utf8');
const [{apply},{getEditorInvocation},{prepareSdkRuntime},{prepareDeepSeekFixture},api,sessionAPI,
 {Context},{createScope},{default:SystemPrompt,renderPrompt},{default:ToolRuntime},{default:LocalAttachmentStore}]=await Promise.all([
 ...['plugin.mjs','editor-invocation.mjs','prepare-sdk-runtime.mjs','prepare-deepseek-fixture.mjs'].map(native),
 ...['packages/llm/llm/lib/index.js','packages/core/session/lib/index.js','vendor/cordis/lib/index.js',
 'packages/core/scope/lib/index.js','packages/core/system-prompt/lib/index.js','packages/core/tools/lib/index.js',
 'packages/attachment/attachment-local/lib/index.js'].map(load),
]);
const {default:Runtime}=await import(pathToFileURL(await prepareSdkRuntime(dsh,join(build,'sdk'))).href);
const {deepSeekImageTokens}=await load('packages/llm/llm-deepseek/lib/types/image-tokens.js');
const sharp=createRequire(join(dsh,'packages/attachment/attachment-local/package.json'))('sharp');
const sha=bytes=>createHash('sha256').update(bytes).digest('hex');
const run={packageRoot,core,core_sha256:sha(await readFile(core)),plugin_sha256:sha(await readFile(join(packageRoot,'plugin.mjs'))),
 model:config.model,lineageOnly,finalMode,baseline_upper_cny:baselineCost,requested:ids,completed:[]};
const save=(name,result)=>writeFile(join(evidenceRoot,started+'-'+name+'.json'),JSON.stringify(config.redact(result),null,2));
const arms=['original','crops','mosaic','description'];
const maxTokens=Number(process.argv.find(arg=>arg.startsWith('--max-tokens='))?.slice(13)??3072);
assert.ok(Number.isInteger(maxTokens)&&maxTokens>0&&maxTokens<=4096,'invalid output cap');
const settings={maxTokens,reasoningEffort:'off',temperature:0};

for(const row of imageCases.filter(row=>ids.includes(row.key))){
 let result;
 try{result=await scenario(row);}catch(error){result={key:row.key,error:String(error)};process.exitCode=1;}
 if(result.error)process.exitCode=1;
 await save(row.key,result);
 run.completed.push({key:row.key,error:result.error,arms:Object.fromEntries(Object.entries(result.arms??{}).map(([name,arm])=>[name,arm.score])),paid_requests:result.paid_requests});
 console.log(JSON.stringify({case:run.completed.at(-1),paid:config.ledger.requests.length,upper_cny:config.cost()}));
 await save('summary',{...run,settings,attempts:config.ledger.requests.length,upper_cny:config.cost()});
}

async function scenario(row){
 const host=await isolated({evidenceRoot:join(evidenceRoot,'fixtures')});
 const ctx=new Context(),calls=[],capture=[],sdk=[],result={key:row.key,targets:row.targets,expected:row.answer,task:row.task,arms:{},fixture:host.root};
 const start=config.ledger.requests.length;
 let scope,phase='setup',step=1;
 try{
  await ctx.plugin(Runtime);
  await ctx.plugin(SystemPrompt,{includeHarnessIdentity:false});
  await ctx.plugin(ToolRuntime);
  await ctx.plugin(LocalAttachmentStore,{dshHome:join(host.root,'dsh')});
  const attachmentStore=ctx.attachments;
  ctx.systemPrompt.section({name:'native-review',order:0,text:await bundle('native.md'),interpolate:false});
  for(const definition of JSON.parse(await bundle('native-tools.json')))ctx.tools.register({...definition,output:{schema:{},render:()=>[]},
   execute:async()=>{throw Error('native fixture tools cannot be executed');}});
  const session=sessionAPI.Session.create('vision-'+row.key),agent={id:session.id,status:'idle',session};
  scope=createScope(ctx,agent);agent.ctx=scope.ctx;
  const facade={llm:ctx.llm,tools:ctx.tools,agents:{list:()=>[agent],get:id=>id===agent.id?agent:null,roots:()=>[agent],currentInitiator:()=>agent},
   get:name=>name==='attachments'?attachmentStore:undefined,
   jobs:{externalControlVersion:1,externalExecutorJoinVersion:1,joinExecutor:async()=>{},attachExternalLifecycle:()=>()=>{},externalLaunchKind:()=>null},
   sessions:{get:id=>id===session.id?session:null,flush:async()=>{
    await writeFile(join(host.root,'session.jsonl'),session.snapshotEvents().map(event=>JSON.stringify(event)).join('\n')+'\n',{flush:true});return true;
   }},
   goals:{externalDriverVersion:1,defaultMaxGoalRounds:8,attachExternalState:()=>()=>{}},
   effect:()=>{},emit:()=>{},logger:{warn:()=>{}},sessionProjections:{stateOf:()=>({openTurnStartSeq:0,lastStepStartSeq:step})},on:(name,fn)=>ctx.on(name,fn),
   inject:(dependencies,fn)=>dependencies.some(name=>['subagents','workflowEngine','compaction'].includes(name))?{dispose:()=>{}}:fn({effect:body=>body(),connection:{fetch:{register:()=>()=>{}}}})};
  ctx.on('llm/stream',async function*(options,next){
   const role=getEditorInvocation()?'editor':'worker',record={role,phase,chunks:[]};calls.push(record);
   for await(const chunk of next()){record.chunks.push(chunk);if(chunk.type==='usage')await config.usage(role,chunk.usage);yield chunk;}
  });
  const installed=await apply(facade,{enabled:true,imageCompression:true,imageSharp:sharp,actor:'native-http-actor',coreURL:host.baseURL,
   credential:host.credential,projectionRoot:join(host.root,'projections'),bindings:{[session.id]:{chain_id:host.chain_id,scope_id:'vision-live',role:'WORKER'}},
   renderPrompt,llmAPI:api,sessionAPI,agentEvents:()=>({emit:()=>{}})});
  const route=await installNativeApi(ctx,{protocol:'messages',model:config.model,baseURL:config.baseURL,auth:config.auth,
   buildRoot:join(build,'messages'),image:{store:attachmentStore},prepareDeepSeekFixture,
   observeOptions:options=>sdk.push({phase,role:getEditorInvocation()?'editor':'worker',options:{...options,signal:undefined}}),
   fetchNative:async(url,init)=>{
    const editor=getEditorInvocation(),role=editor?'editor':'worker';
    const reserve=reservePaidRequest(config.ledger,JSON.parse(init.body),Buffer.byteLength(init.body),{model:config.model,maxRequests});
    if(baselineCost+config.cost()+reserve>5)throw Error('combined image validation cost cap exceeded');
    const response=await config.fetchProvider(url,init,{role,source_id:editor?.source_id,protocol:'messages'});
    capture.push({number:config.ledger.requests.at(-1).number,role,phase,body:readEventStream(response.clone().body)});
    return response;
   }});
  const fixture=await renderCase(sharp,row);
  const original=await attachmentStore.saveImage({data:fixture.png,mediaType:'image/png'});
  await writeFile(join(host.root,'held-out.png'),fixture.png);
  result.original={attachment:original,sha256:sha(fixture.png),targets:fixture.targets};
  const system=api.createSystemMessage(await bundle('fused.md'));
  const guidance='Only the authenticated Editor should prepare the visual recipe. In sic_edit_blocks, select compact integer-coordinate crops showing the two requested panels, with one crop per panel when possible. Cite one transcription observation per requested panel as description text with the relevant tile number. The observations should say which panel and what is legible. Do not invent a transcription. Set display to '+(lineageOnly?'description':'mosaic')+'. Stay strictly inside the raster: x+width must not exceed raster width and y+height must not exceed raster height. Return one sic_edit_blocks tool call with ops empty and pvi.I containing only the image recipe. All originals and the question stay available through the Host. As authenticated Worker, answer the user question in the specified JSON format.';
  const runtime=api.createUserMessage({source:{kind:'runtime-context',sections:[{name:'sic:activation',text:'{"role":"WORKER","scope_id":"vision-live"}'},{name:'acceptance:image-recipe',text:guidance}]},content:[{type:'text',text:guidance}]});
  const user=api.createUserMessage({source:{kind:'user'},content:[{type:'text',text:row.task},{type:'image',attachment:original}]});
  assertNoAnswerInText([row.task,guidance],row.codes);

  async function sendMessage(messages,sessionId){
   const prepared=await ctx.llm.prepareCall({provider:route.provider,model:route.model,...settings}),assembler=new api.BlockAssembler();
   for await(const chunk of prepared.stream({...prepared.config,messages,sessionId,tools:sessionId?ctx.tools.schemas(agent):[]})){
    if(chunk.type==='finish'&&chunk.reason.kind==='error')throw Error(JSON.stringify(chunk));
    assembler.push(chunk);
   }
   return assembler.message({provider:route.provider,model:route.model});
  }
  const messageText=message=>message.content.filter(block=>block.type==='text').map(block=>block.text).join('').trim();
  async function arm(name,answer){
   phase=name;
   try{const text=await answer();result.arms[name]={text,score:score(text,row.answer)};}
   catch(error){result.arms[name]={error:String(error),score:score('',row.answer)};}
  }

  let initialAnswer;
  await arm('production_i',async()=>{initialAnswer=await sendMessage([system,user,runtime],session.id);return messageText(initialAnswer);});
  const page=await installed.store.list(session.id);
  const projection=page.rows.length?await installed.store.get(session.id,page.rows[0].id):null;
  const imageView=projection?.layout_receipt?.image_view,lineage=imageView?.active?imageView.lineage:null;
  if(lineage){
   result.production={projection_id:projection.id,layout:imageView,coverage:coverage(fixture.targets,lineage.crops),cut:await host.core.operate('cut.get',{})};
   const images=role=>sdk.find(record=>record.phase==='production_i'&&record.role===role)?.options.messages.flatMap(m=>m.content).filter(b=>b.type==='image').map(b=>b.attachment.attachmentId);
   assert.ok(images('editor')?.includes(original.attachmentId),'Editor must receive original image');
   if(lineageOnly)assert.deepEqual(images('worker'),[],'description I must not send any original/crop/mosaic image');
   else assert.deepEqual(images('worker'),[lineage.mosaic.attachment.attachmentId],'production Worker must receive adopted I mosaic only');
  }
  if(lineageOnly){
   assert.ok(lineage&&initialAnswer,'a real admitted recipe and Worker answer are required before recall');
   assert.equal(imageView.pointer.display,'description');
   const hiddenLabel=['A','B','C','D'].find(label=>!row.targets.includes(label));
   const hiddenCode=row.codes['ABCD'.indexOf(hiddenLabel)];
   const hiddenFixture=await renderCase(sharp,{...row,targets:[hiddenLabel]});
   assert.equal(coverage(hiddenFixture.targets,lineage.crops)[0].max_coverage,0,'recall target must be fully absent from every Editor crop');
   assertNoAnswerInText(lineage.description.parts.map(part=>part.text),[hiddenCode]);
   const followup='Now read the four-character code in panel '+hiddenLabel+' of the original image. This is an uncropped panel. Use the manifest_id shown in the current I view. First call sic_image_source from description without a tile to inspect its mosaic. Next call it from mosaic for tile 1, then from crop for tile 1 to read the original. Take one tool step at a time. Return only a JSON array with the panel code, or "?" inside the array if unreadable.';
   const hold='Editor: this is a read-only recall follow-up. Preserve all existing I views and native tool evidence. Deliver one sic_edit_blocks call with ops empty and pvi empty. Only the Worker may use sic_image_source.';
   const recallRuntime=api.createUserMessage({source:{kind:'runtime-context',sections:[{name:'sic:activation',text:'{"role":"WORKER","scope_id":"vision-live"}'},{name:'acceptance:image-recall',text:hold}]},content:[{type:'text',text:hold}]});
   const recallUser=api.createUserMessage({source:{kind:'user'},content:[{type:'text',text:followup}]});
   let history=[system,user,initialAnswer,recallUser,recallRuntime];
   result.recall={manifest_id:imageView.pointer.manifest_id,hidden_label:hiddenLabel,hidden_expected:hiddenCode,
    hidden_region:hiddenFixture.targets[0].region,native_calls:[]};
   for(const [from,target] of [['description',lineage.mosaic],['mosaic',lineage.crops[0]],['crop',lineage.originals.find(item=>item.id===lineage.crops[0].parent_id)]]){
    step++;phase='recall_'+from;
    const assistant=await sendMessage(history,session.id);
    const nativeCalls=assistant.content.filter(block=>block.type==='tool-call');
    assert.equal(nativeCalls.length,1,'one actual native tool call per recall step');
    assert.equal(nativeCalls[0].name,'sic_image_source');
    const args=JSON.parse(nativeCalls[0].arguments);
    assert.deepEqual(args,{manifest_id:imageView.pointer.manifest_id,from,...from==='description'?{}:{tile:1}});
    const tool=await ctx.tools.execute({agent,name:nativeCalls[0].name,arguments:args,callId:nativeCalls[0].id,signal:new AbortController().signal});
    result.recall.native_calls.push({from,call_id:nativeCalls[0].id,arguments:args,result:tool.content,isError:tool.isError});
    assert.equal(tool.isError,false,JSON.stringify(tool));
    assert.deepEqual(tool.content.filter(block=>block.type==='image').map(block=>block.attachment),[target.attachment]);
    const message=api.createToolResultMessage({callId:nativeCalls[0].id,isError:false,content:tool.content});
    history=[...history.slice(0,-1),assistant,message,recallRuntime];
   }
   step++;
   await arm('recall_answer',async()=>messageText(await sendMessage(history,session.id)));
   result.arms.recall_answer.score=score(result.arms.recall_answer.text??'',[hiddenCode]);
   assert.ok(result.arms.recall_answer.score.exact,'real Worker did not answer correctly after original retrieval');
   return result;
  }
  const common=api.createSystemMessage('Answer the user task from its supplied evidence. Return only the requested JSON.');
  const imageBlock=attachment=>({type:'image',attachment});
  const representations={original:[imageBlock(original)],crops:lineage?.crops.map(crop=>imageBlock(crop.attachment)),
   mosaic:lineage&&[imageBlock(lineage.mosaic.attachment)],
   description:lineage&&[{type:'text',text:lineage.description.parts.map(part=>part.text).join('\n')}]};
  for(const name of [...arms.slice(row.index),...arms.slice(0,row.index)]){
   if(!representations[name]){result.arms[name]={error:'No admitted real Editor recipe',score:score('',row.answer)};continue;}
   const parts=[{type:'text',text:row.task},...representations[name]];
   if(name!=='description')assertNoAnswerInText(parts.filter(part=>part.type==='text').map(part=>part.text),row.codes);
   await arm(name,async()=>messageText(await sendMessage([common,api.createUserMessage({source:{kind:'user'},content:parts})])));
  }
 }catch(error){result.error=String(error);}
 finally{
  try{
   result.wire=[];
   for(const item of capture){
    const raw=await item.body;
    const events=raw.text.split(/\r?\n/).filter(line=>line.startsWith('data: ')).flatMap(line=>{
     try{return[JSON.parse(line.slice(6))];}catch{return[];}
    });
    const rawUsage=events.flatMap(event=>[event.usage,event.message?.usage,event.response?.usage].filter(Boolean));
    const visual=rawUsage.flatMap(usage=>visualFields(usage));
    const ledger=config.ledger.requests.find(row=>row.number===item.number);
    const wireImages=await Promise.all(strings(ledger.request).filter(value=>value.startsWith('iVBOR')||value.startsWith('data:image/png;base64,')).map(async value=>{
     const data=Buffer.from(value.replace(/^data:image\/png;base64,/u,''),'base64'),{width,height}=await sharp(data).metadata();
     return{sha256:sha(data),bytes:data.length,width,height,dsh_estimated_visual_tokens:deepSeekImageTokens(width,height)};
    }));
    result.wire.push({phase:item.phase,number:item.number,role:item.role,http_status:ledger.status,native_usage:rawUsage,capture_end:raw.end,
     explicit_visual_usage:visual.length?Object.fromEntries(visual):null,sdk_usage:ledger.usage??null,
     cost_upper_cny:paidRowUpperCost(ledger),wire_images:wireImages,dsh_estimated_visual_tokens:wireImages.reduce((sum,image)=>sum+image.dsh_estimated_visual_tokens,0)});
   }
   result.paid_requests=config.ledger.requests.slice(start).map(row=>row.number);
   result.chunks=calls;result.sdk_options=sdk;
   await save(row.key,result);await config.save();
  }finally{if(scope)await scope.dispose();await ctx.fiber.dispose();await host.close();}
 }
 return result;
}

function visualFields(object,path=[]){
 return Object.entries(object).flatMap(([key,value])=>{
  const next=[...path,key];
  if(value!==null&&typeof value==='object')return visualFields(value,next);
  return /image|vision|visual/i.test(next.join('.'))?[[next.join('.'),value]]:[];
 });
}

async function readEventStream(body){
 const decoder=new TextDecoder();
 let text='',end='complete';
 try{if(body)for await(const chunk of body)text+=decoder.decode(chunk,{stream:true});}
 catch(error){end=String(error);}
 return{text:text+decoder.decode(),end};
}
