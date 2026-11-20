import {isDeepStrictEqual} from 'node:util';
import {withEditorInvocation} from './editor-invocation.mjs';
import {runtimeControlText} from './runtime-control.mjs';
import {assignedEditorUnits} from './editor-origins.mjs';
import {normalizeEditorDelivery} from './editor-delivery.mjs';
import {CoreOperationError} from './core-client.mjs';
import {ImageDeliveryError} from './image-compression.mjs';

export function editorLimits(value={}){
  if(!value||typeof value!=='object'||Array.isArray(value)||Object.keys(value).some(key=>!['maxToolRounds','timeoutMs'].includes(key)))throw Error('invalid host Editor limits');
  const {maxToolRounds=16,timeoutMs=120000}=value;
  if(!Number.isSafeInteger(maxToolRounds)||maxToolRounds<0||maxToolRounds>128
      ||(timeoutMs!==null&&(!Number.isSafeInteger(timeoutMs)||timeoutMs<1||timeoutMs>600000)))throw Error('invalid host Editor limits');
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
  const problems=new Set();
  for(const op of delivery.ops){
    if(caps?.ops&&!caps.ops.includes(op?.op))problems.add('Editor operation not activated');
    if(op?.op==='ADD'){
      if(op.anchor&&(!ids.has(op.anchor.target)||!['before','after'].includes(op.anchor.position)))problems.add('Editor ADD anchor outside assigned units or invalid position');
    }else if(!ids.has(op?.target))problems.add('Editor operation outside assigned units');
    const life=op?.lifecycle??'P';
    if(life!=='P'&&caps?.pvi&&!caps.pvi.includes(life))problems.add('Editor lifecycle not activated');
    if(['ADD','COMPRESS','REPLACE','FLAG'].includes(op?.op)&&typeof op.content!=='string')problems.add('ADD, COMPRESS, REPLACE and FLAG each require a content string');
  }
  if(caps?.pvi&&Object.entries(delivery.pvi).some(([key,value])=>!caps.pvi.includes(key)
    &&(Array.isArray(value)?value.length>0:value&&Object.keys(value).length>0)))problems.add('Editor PVI channel not activated. Use only the enabled P, V and I keys; put named instant views inside pvi.I');
  return [...problems].join('. ');
}

function reducesAssignedText(delivery,assigned){
  const additions=delivery.pvi.P??[];
  if(!Array.isArray(additions)||additions.some(text=>typeof text!=='string'))return false;
  const sizes=new Map(assigned.map(unit=>[unit.unit_id,unit.text.length]));
  let savings=0;
  for(const op of delivery.ops){
    if((op.op==='COMPRESS'||op.op==='REPLACE')&&(op.lifecycle??'P')==='P'&&typeof op.content==='string')savings+=(sizes.get(op.target)??0)-op.content.length;
    if(op.op==='ADD')savings-=op.content?.length??0;
  }
  return savings>additions.reduce((sum,text)=>sum+text.length,0);
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
    const signals=originalAssignment.signal?[originalAssignment.signal]:[];
    if(timeoutMs!==null)signals.push(AbortSignal.timeout(timeoutMs));
    const signal=AbortSignal.any(signals);
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
    const sections=[...(assignment.runtime_sections??[]).filter(s=>!['sic:activation','sic:instant-views','sic:archive-text'].includes(s.name)),{name:'sic:activation',text:activation}];
    if(auth.allowed_tool_names.includes('sic_source_text_page')){
      sections.push({name:'sic:source-page-scope',text:'For sic_source_text_page, use source_id and assigned_unit_ids from the current activation, or an entry in readable_sources. Historical tool arguments and source references do not grant read access. If a page is denied, use the assigned WB or correct the arguments, then deliver the edit.'});
    }
    if(auth.allowed_tool_names.includes('sic_memory_page')){
      sections.push({name:'sic:read-only-operations',text:'Inspect fixed operations with sic_memory_page, kind "ops", and an object_id from read_only_cb_ids in the current activation. Only assigned WB units can be edit targets.'});
    }
    sections.push({name:'sic:edit-scope',text:'Each operation target or ADD anchor must use an exact unit_id from the current assigned WB. Shared CB ordinals and IDs from earlier calls are references only. Do not invent another unit ID.'});
    sections.push({name:'sic:delivery-shape',text:'ADD, COMPRESS, REPLACE and FLAG each require a content string. In pvi use only the enabled channel keys P, V or I. Put a named view such as status inside I when that channel is enabled.'});
    if(auth.edit_capabilities?.ops?.includes('ADD')){
      sections.push({name:'sic:add-placement',text:'Omit anchor to append. For relative insertion put the assigned target ID and a before or after position inside anchor.'});
    }
    if(assignment.capacity_pressure){
      sections.push({name:'sic:context-pressure',text:'The active context is running out of room. Compress the assigned content substantially with COMPRESS. Retain facts, names, provenance, and the path to exact details. A no-op or KEEP-only batch cannot free room. A text_ref of kind shared-cb points to its exact text in the shared CB prefix, counted by one-based CB ordinal. Use its assigned unit ID in the edit, and page the current assignment only if exact details are needed.'});
    }else if(assignment.reference_shared){
      sections.push({name:'sic:shared-source',text:'A WB unit with text_ref of kind shared-cb uses the exact text already present in the shared CB prefix. Count CB ordinals from one. Use the assigned WB unit ID when editing that text.'});
    }
    if(auth.edit_capabilities?.pvi?.includes('I')&&assignment.image_compression?.length){
      sections.push({name:'sic:image-edit',text:'Optional visual compression. In sic_edit_blocks, set pvi.I["sic-image-compression-v1"] to {crops:[{message_id,block_index,x,y,width,height}],description:[{text,tiles:[1]}]}. Coordinates are integers in the archived raster; tiles are numbered by crop order. Default display is mosaic. Set display to crops for separate region images, or description to send only cited text when no visual detail remains necessary. This view substitutes for the selected full images. Omit the key to keep it; null removes it. Available source coordinates: '+JSON.stringify(assignment.image_compression)});
    }
    const savedActivation=assignment.archivedContinuation?.messages?.[shared.length+(assignment.instant_message?1:0)];
    if(!assignment.archivedContinuation||savedActivation?.source?.sections?.some(section=>section.name==='sic:active-tools')){
      const names=auth.allowed_tool_names.join(', ');
      sections.push({name:'sic:active-tools',text:`This call is in Editor mode. Work on the assigned WB and deliver the edit with sic_edit_blocks. The only active tools for this call are ${names}. Host task tools in the shared prefix are inactive in Editor mode.`});
    }
    const messages=[...shared,...(assignment.instant_message?[assignment.instant_message]:[]),
      createUserMessage({source:{kind:'runtime-context',form:'snapshot',sections},content:[{type:'text',text:runtimeControlText(sections)}]}),
      createUserMessage({source:{kind:'sic-editor-wb',form:'recall'},content:[{type:'text',text:JSON.stringify({contract:'authenticated-wb-origins-v1',source_id:source.source_id,source_digest:source.source_digest,version:source.version,units:assignedEditorUnits(source,assigned,options,shared,{referenceShared:assignment.capacity_pressure||assignment.reference_shared})})}]})];
    // These are the current Harness's concrete call-config keys. Other native
    // request options remain on `options` and pass to its existing adapter.
    const config=Object.fromEntries(['provider','model','reasoningEffort','temperature','maxTokens','stop'].filter(key=>Object.hasOwn(options,key)).map(key=>[key,options[key]]));
    let round=0,deliveryCorrection=false,outputCorrection=false;
    const deliveryReminder=(name,text)=>{
      messages.push(createUserMessage({source:{kind:'runtime-context',form:'snapshot',sections:[{name,text}]},content:[{type:'text',text}]}));
      round++;
    };
    const requestConciseDelivery=()=>{
      deliveryReminder('sic:output-recovery','The provider stopped the previous response at its output limit before a complete edit was delivered. Use the same assigned WB. Submit one concise sic_edit_blocks batch now. Include only necessary edits and their essential facts. Leave unchanged units out of ops. Keep optional views brief or omit them.');
      outputCorrection=true;
    };
    const continuation=assignment.archivedContinuation;
    if(continuation){
      const previous=continuation.messages;
      const fixed=messages.length-2;
      const semantic=({id,timestamp,...message})=>message;
      const persisted=JSON.parse(JSON.stringify(messages));
      if(!Number.isSafeInteger(continuation.round)||continuation.round<0||continuation.round>maxToolRounds
        ||!Array.isArray(previous)||!isDeepStrictEqual(previous.slice(0,fixed),persisted.slice(0,fixed)))throw Error('Archived Editor continuation changed its frozen shared prefix');
      const archivedActivation=previous.slice(fixed,messages.length).map(semantic);
      const currentActivation=persisted.slice(fixed).map(semantic);
      if(continuation.transportRetry||continuation.outputExhausted&&continuation.round===0){
        // Historical host instructions can predate the current delivery text.
        // A transport retry keeps its original activation and exact authority.
        const archivedAuthority=archivedActivation[0]?.source?.sections?.find(section=>section.name==='sic:activation');
        if(archivedActivation[0]?.source?.kind!=='runtime-context'||!archivedAuthority
          ||!isDeepStrictEqual(JSON.parse(archivedAuthority.text),auth))throw Error('Archived Editor recovery changed its authenticated authority');
        if(continuation.transportRetry)messages[fixed]=api.freezeMessage(previous[fixed]);
      }else if(!isDeepStrictEqual(archivedActivation[0],currentActivation[0]))throw Error('Archived Editor continuation changed its frozen role activation');
      if(!isDeepStrictEqual(archivedActivation[1],currentActivation[1]))throw Error('Archived Editor continuation changed its frozen WB');
      const transcript=previous.slice(messages.length);
      if(transcript.filter(message=>message.role==='assistant').length!==continuation.round
        ||transcript.some(message=>!['assistant','tool'].includes(message.role)))throw Error('Archived Editor continuation is not a tool transcript');
      messages.push(...transcript.map(api.freezeMessage));
      round=continuation.round;
      deliveryCorrection=continuation.deliveryCorrection===true;
      if(continuation.restoredPages){
        const text='The host restored your source pages for this assignment. Use the WB and those pages to submit one sic_edit_blocks batch. Request another page only if a necessary fact is missing.';
        messages.push(createUserMessage({source:{kind:'runtime-context',form:'snapshot',sections:[{name:'sic:restored-source-pages',text}]},content:[{type:'text',text}]}));
      }
      if(continuation.outputExhausted){
        if(round>=maxToolRounds)throw Error('Archived output recovery exceeds the Editor round limit');
        requestConciseDelivery();
      }
      if(continuation.deliveryRepair){
        const feedback={
          'missing-batch':'The previous reply was missing a valid final edit batch. Return one sic_edit_blocks call for the same assigned WB.',
          'image-scope':'The previous image recipe cited a source outside this assignment or an offloaded image. Use only the exact currently offered message_id and block_index pairs, or omit the optional image recipe. Return one complete sic_edit_blocks batch.',
          'unauthorized-tool':'The previous tool request was outside this Editor assignment and was rejected. Work only on the assigned WB. Use the tools named in the authenticated activation and submit one sic_edit_blocks batch.',
        }[continuation.deliveryRepair];
        if(typeof feedback!=='string'||deliveryCorrection||round>=maxToolRounds)throw Error('Invalid archived Editor delivery repair');
        deliveryReminder('sic:delivery-recovery',feedback);
        deliveryCorrection=true;
      }
    }
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
        if(round===maxToolRounds){
          const text='This is the final Editor request for this assignment. Use the existing WB and returned pages. Submit one sic_edit_blocks batch now. No further source pages will be requested.';
          messages.push(createUserMessage({source:{kind:'runtime-context',form:'snapshot',sections:[{name:'sic:final-delivery',text}]},content:[{type:'text',text}]}));
        }
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
        if(assembled.finish.kind==='error'){
          const {message,code,...facts}=assembled.finish.failure;
          throw new api.LlmError(message,code,facts);
        }
        if(assembled.finish.kind==='max-tokens'){
          if(outputCorrection||round>=maxToolRounds)throw Error('Editor exhausted the native output allowance before delivering a complete edit batch');
          requestConciseDelivery();
          continue;
        }
        const calls=assistant.content.filter(block=>block.type==='tool-call');
        if(calls.length===1&&calls[0].name==='sic_edit_blocks'){
          await assertFresh();
          let delivery;
          try{delivery=normalizeEditorDelivery(JSON.parse(calls[0].arguments));}catch(error){if(!(error instanceof SyntaxError))throw error;}
          const validBatch=Array.isArray(delivery?.ops)&&delivery.pvi&&typeof delivery.pvi==='object'&&!Array.isArray(delivery.pvi);
          const scopeError=validBatch?validateDeliveryScope(delivery,auth):null;
          let rejection,feedback;
          if(!validBatch){
            rejection=new Error('Editor did not deliver an edit batch');
            feedback='Use one sic_edit_blocks call with valid JSON, an ops array, and pvi as an object. Use an empty pvi object when there are no P, V or I additions.';
          }else if(scopeError){
            rejection=new Error(scopeError);
            feedback=scopeError+'. Return one complete edit batch using the IDs in the current WB and activation. Remove or correct all references outside this assignment.';
          }else if(assignment.capacity_pressure&&!reducesAssignedText(delivery,assigned)){
            rejection=new Error('SIC pressure Editor did not reduce the assigned persistent text');
            feedback='This batch did not shorten the assigned context. Submit a substantially shorter COMPRESS replacement for the assigned units.';
          }else{
            try{await assignment.checkDelivery?.(delivery,signal);}
            catch(error){
              const imageError=error instanceof ImageDeliveryError;
              const nativeError=error instanceof CoreOperationError&&error.operation==='native.preview'
                &&(error.status===422&&error.code==='invalid'||error.status===501&&['native_reorder_unsupported','native_projection_unsupported'].includes(error.code));
              if(!imageError&&!nativeError)throw error;
              rejection=error;
              const reason=imageError?error.message:error.detail;
              const detail=typeof reason==='string'?reason.replace(/\s+/g,' ').slice(0,240):'invalid edit batch';
              feedback='Core rejected this edit before commit. Correct the batch and submit one complete sic_edit_blocks call for the same assignment. Validation reason: '+detail;
            }
          }
          if(!rejection){await assertFresh();return delivery;}
          if(deliveryCorrection||round>=maxToolRounds)throw rejection;
          deliveryCorrection=true;
          const result={isError:true,content:[{type:'text',text:feedback}]};
          messages.push(assistant,createToolResultMessage({callId:calls[0].id,content:result.content,isError:true}));
          await record({stage:'editor-tool-result',source_id:source.source_id,round,call_id:calls[0].id,name:calls[0].name,result},signal);
          round++;
          continue;
        }
        if(!calls.length||calls.some(call=>call.name==='sic_edit_blocks')){
          if(deliveryCorrection||round>=maxToolRounds)throw Error('Editor must deliver one final edit batch');
          deliveryCorrection=true;
          deliveryReminder('sic:delivery-recovery','The previous reply did not deliver a single accepted edit batch. Use this same assigned WB and return one sic_edit_blocks call with the completed ops and pvi.');
          continue;
        }
        if(typeof executeNativeTool!=='function'||calls.some(call=>!auth.allowed_tool_names.includes(call.name))){
          if(deliveryCorrection||round>=maxToolRounds)throw Error('Editor requested an unauthorized native tool');
          deliveryCorrection=true;
          deliveryReminder('sic:tool-scope-correction','The requested tool is outside this Editor assignment. Use only the tools named in the authenticated activation. Work on the assigned WB and submit one sic_edit_blocks batch.');
          continue;
        }
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
