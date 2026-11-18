import {isDeepStrictEqual} from 'node:util';
import {withEditorInvocation} from './editor-invocation.mjs';
import {runtimeControlText} from './runtime-control.mjs';
import {assignedEditorUnits} from './editor-origins.mjs';

export function editorLimits(value={}){
  if(!value||typeof value!=='object'||Array.isArray(value)||Object.keys(value).some(key=>!['maxToolRounds','timeoutMs'].includes(key)))throw Error('invalid host Editor limits');
  const {maxToolRounds=16,timeoutMs=120000}=value;
  if(!Number.isSafeInteger(maxToolRounds)||maxToolRounds<0||maxToolRounds>128
      ||!Number.isSafeInteger(timeoutMs)||timeoutMs<1||timeoutMs>600000)throw Error('invalid host Editor limits');
  return {maxToolRounds,timeoutMs};
}

function freezeSnapshot(value){
  const snapshot=structuredClone(value);
  const freeze=v=>{if(v&&typeof v==='object'){for(const item of Object.values(v))freeze(item);Object.freeze(v);}return v;};
  return freeze(snapshot);
}

function validateDeliveryScope(delivery,authority){
  const ids=new Set(authority.assigned_unit_ids);
  const caps=authority.edit_capabilities;
  for(const op of delivery.ops){
    if(caps?.ops&&!caps.ops.includes(op?.op))throw Error('Editor operation not activated');
    if(op?.op==='ADD'){
      if(op.anchor&&(!ids.has(op.anchor.target)||!['before','after'].includes(op.anchor.position)))throw Error('Editor ADD anchor outside assigned units');
    }else if(!ids.has(op?.target))throw Error('Editor operation outside assigned units');
    const life=op?.lifecycle??'P';
    if(life!=='P'&&caps?.pvi&&!caps.pvi.includes(life))throw Error('Editor lifecycle not activated');
  }
  if(caps?.pvi&&Object.entries(delivery.pvi).some(([key,value])=>!caps.pvi.includes(key)
    &&(Array.isArray(value)?value.length>0:value&&Object.keys(value).length>0)))throw Error('Editor PVI channel not activated');
}

/** Use the Harness's own prepared call, serializer, stream parser and assembler.
 * No Provider transport, tokenizer, choice selection or retry policy lives here.
 * The host supplies the real shared CB projection and trusted candidate scope.
 */
export async function createHarnessEditorDriver({llm,authorizeEditor,record,executeNativeTool,llmAPI,limits}) {
  if(typeof llm?.prepareCall!=='function'||typeof authorizeEditor!=='function'||typeof record!=='function')throw TypeError('Harness LLM, trusted Editor authorization and durable trace required');
  const api=llmAPI??await import('@deepseek-ai/dsh-llm');
  const {BlockAssembler,createUserMessage,createToolResultMessage}=api;
  if(![BlockAssembler,createUserMessage,createToolResultMessage].every(v=>typeof v==='function'))throw TypeError('actual Harness message/assembler API required');
  const {maxToolRounds,timeoutMs}=editorLimits(limits);
  return async originalAssignment=>{
    const deadline=AbortSignal.timeout(timeoutMs);
    const signal=originalAssignment.signal?AbortSignal.any([originalAssignment.signal,deadline]):deadline;
    const assignment={...originalAssignment,signal};
    const {source,invocation}=assignment;
    const options=assignment.originalOptions??assignment.options;
    const shared=assignment.shared_messages;
    if(!source?.source_id||!options?.provider||!options.model||!Array.isArray(options.tools)||!Array.isArray(shared))throw Error('native route, frozen tools and actual shared message projection required');
    if(shared.some(m=>m.source?.kind==='runtime-context'))throw Error('role activation belongs after the shared prefix');
    signal?.throwIfAborted();
    const auth=freezeSnapshot(await authorizeEditor(assignment));
    const ids=auth?.assigned_unit_ids;
    if(auth?.role!=='EDITOR'||auth.source_id!==source.source_id||auth.source_digest!==source.source_digest
      ||auth.expected_version!==source.version||!auth.scope_id||!Array.isArray(ids)||new Set(ids).size!==ids.length
      ||!Array.isArray(auth.allowed_tool_names)||!auth.allowed_tool_names.includes('sic_edit_blocks'))throw Error('trusted Editor scope does not match native candidate');
    const permitted=source.binding?.editable_unit_ids;
    if(!Array.isArray(permitted)||ids.some(id=>!permitted.includes(id)))throw Error('Editor assignment exceeds persistent native scope');
    const assigned=source.units.filter(unit=>ids.includes(unit.unit_id));
    if(assigned.length!==ids.length)throw Error('assigned source unit missing');
    const activation=JSON.stringify(auth);
    const sections=[...(assignment.runtime_sections??[]).filter(s=>s.name!=='sic:activation'&&s.name!=='sic:instant-views'),{name:'sic:activation',text:activation}];
    if(auth.edit_capabilities?.pvi?.includes('I')&&assignment.image_compression?.length){
      sections.push({name:'sic:image-edit',text:'Optional visual compression. In sic_edit_blocks, set pvi.I["sic-image-compression-v1"] to {crops:[{message_id,block_index,x,y,width,height}],description:[{text,tiles:[1]}]}. Coordinates are integers in the archived raster; tiles are numbered by crop order. Default display is mosaic. Set display to crops for separate region images, or description to send only cited text when no visual detail remains necessary. This view substitutes for the selected full images. Omit the key to keep it; null removes it. Available source coordinates: '+JSON.stringify(assignment.image_compression)});
    }
    const messages=[...shared,...(assignment.instant_message?[assignment.instant_message]:[]),
      createUserMessage({source:{kind:'runtime-context',form:'snapshot',sections},content:[{type:'text',text:runtimeControlText(sections)}]}),
      createUserMessage({source:{kind:'sic-editor-wb',form:'recall'},content:[{type:'text',text:JSON.stringify({contract:'authenticated-wb-origins-v1',source_id:source.source_id,source_digest:source.source_digest,version:source.version,units:assignedEditorUnits(source,assigned,options,shared)})}]})];
    // These are the current Harness's concrete call-config keys. Other native
    // request options remain on `options` and pass to its existing adapter.
    const config=Object.fromEntries(['provider','model','reasoningEffort','temperature','maxTokens','stop'].filter(key=>Object.hasOwn(options,key)).map(key=>[key,options[key]]));
    let round=0;
    const assertFresh=async()=>{
      signal.throwIfAborted();
      const renewed=await authorizeEditor({...assignment,round,authority:auth});
      signal.throwIfAborted();
      if(!isDeepStrictEqual(renewed,auth))throw Error('Editor authority changed before continuation or delivery');
    };
    let expectedRequest,expectedSignal;
    const nativeSnapshot=request=>Object.fromEntries(Object.entries(request).filter(([key])=>key!=='signal'));
    const verifyNativeOptions=actual=>{
      if(!expectedRequest||!actual||actual.signal!==expectedSignal||!isDeepStrictEqual(nativeSnapshot(actual),expectedRequest))throw Error('Editor final native request changed after authorization');
    };
    return withEditorInvocation(Object.freeze({role:'EDITOR',source_id:source.source_id,scope_id:auth.scope_id,verifyNativeOptions}),async()=>{
      while(true){
        signal?.throwIfAborted();
        if(round>0)await assertFresh();
        const prepared=await llm.prepareCall(config,signal);
        if(prepared.config.provider!==options.provider||prepared.config.model!==options.model)throw Error('Harness changed shared Editor route');
        const request={...options,...prepared.config,messages:[...messages],tools:options.tools,signal};
        expectedRequest=freezeSnapshot(nativeSnapshot(request));expectedSignal=request.signal;
        const {signal:requestSignal,...traceRequest}=request;
        await record({stage:'editor-request',source_id:source.source_id,round,config:prepared.config,request:traceRequest},signal);
        await assertFresh();
        const assembled=new BlockAssembler();
        for await(const chunk of prepared.stream(request)){
          signal?.throwIfAborted();
          assembled.push(chunk);
          await record({stage:'editor-chunk',source_id:source.source_id,round,chunk},signal);
        }
        signal?.throwIfAborted();
        const assistant=assembled.message({provider:options.provider,model:options.model});
        await record({stage:'editor-response',source_id:source.source_id,round,message:assistant,finish:assembled.finish,
          ...(assembled.usage===undefined?{}:{usage:assembled.usage})},signal);
        const calls=assistant.content.filter(block=>block.type==='tool-call');
        if(calls.length===1&&calls[0].name==='sic_edit_blocks'){
          await assertFresh();
          const delivery=JSON.parse(calls[0].arguments);
          if(!Array.isArray(delivery?.ops)||!delivery.pvi||typeof delivery.pvi!=='object'||Array.isArray(delivery.pvi))throw Error('Editor did not deliver an edit batch');
          validateDeliveryScope(delivery,auth);
          return delivery;
        }
        if(!calls.length||calls.some(call=>call.name==='sic_edit_blocks'))throw Error('Editor must deliver one final edit batch');
        if(typeof executeNativeTool!=='function'||calls.some(call=>!auth.allowed_tool_names.includes(call.name)))throw Error('Editor requested an unauthorized native tool');
        if(round>=maxToolRounds)throw Error('host Editor tool round limit reached');
        messages.push(assistant);
        for(const call of calls){
          await assertFresh();
          const args=JSON.parse(call.arguments);
          const result=await executeNativeTool({name:call.name,arguments:args,callId:call.id,authority:auth,assignment,signal});
          if(typeof result?.isError!=='boolean'||!Array.isArray(result.content))throw Error('native tool did not return a Harness result');
          messages.push(createToolResultMessage({callId:call.id,content:result.content,isError:result.isError}));
          await record({stage:'editor-tool-result',source_id:source.source_id,round,call_id:call.id,name:call.name,result},signal);
        }
        round++;
      }
    });
  };
}
