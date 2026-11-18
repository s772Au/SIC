import {createHarnessEditorDriver} from './editor-driver.mjs';
import {jsonSnapshot} from './session-records.mjs';
import {readNativeSourcePage} from './source-page.mjs';
import {editorCapabilities} from './edit-capabilities.mjs';

export async function composeEditorDriver({ctx,cfg,store,clientFor,bindingFor,recordSession}){
  if(typeof cfg.editorDriver==='function')return cfg.editorDriver;
  const assignments=new Map();
  const driver=await createHarnessEditorDriver({llm:ctx.llm,llmAPI:cfg.llmAPI,limits:cfg.editorLimits,
    authorizeEditor:async assignment=>{
      const binding=bindingFor(assignment.invocation.session_id);
      if(!binding?.chain_id||!binding.scope_id)throw Error('Editor host binding revoked');
      const source=await clientFor(binding).get({source_id:assignment.source.source_id},assignment.signal),ids=source.binding?.editable_unit_ids;
      if(!Array.isArray(ids))throw Error('Core persistent Editor scope missing');
      return{role:'EDITOR',scope_id:source.source_id,source_id:source.source_id,source_digest:source.source_digest,
        expected_version:source.version,assigned_unit_ids:ids,allowed_tool_names:binding.editor_allowed_tools??cfg.editorAllowedTools??['sic_edit_blocks','sic_source_text_page'],
        edit_capabilities:editorCapabilities(binding,source),readable_sources:binding.editor_readable_sources??[]};
    },
    record:async event=>{
      const assignment=assignments.get(event.source_id);if(!assignment)throw Error('Editor trace has no host session binding');
      const sessionId=assignment.invocation.session_id;
      await store.recordTrace(sessionId,event.source_id,jsonSnapshot(event));
      if(event.stage!=='editor-chunk')await recordSession(ctx.sessions.get(sessionId),'sic/editor-trace',{source_id:event.source_id,stage:event.stage,round:event.round});
    },
    executeNativeTool:async execution=>{
      const assignment=execution.assignment,auth=execution.authority,args=execution.arguments;
      if(!auth.allowed_tool_names.includes(execution.name))throw Error('Editor tool not activated');
      if(execution.name==='sic_source_text_page'){
        const core=clientFor(bindingFor(assignment.invocation.session_id));
        return{isError:false,content:[{type:'text',text:JSON.stringify(await readNativeSourcePage({core,authority:auth,args,signal:execution.signal}))}]};
      }
      const editorAgent=cfg.actualEditorAgent?.(auth,assignment);
      if(!editorAgent||bindingFor(editorAgent.session.id,editorAgent)?.role!=='EDITOR')throw Error('actual authorized Editor Agent required for native execution');
      return ctx.tools.execute({agent:editorAgent,name:execution.name,arguments:args,callId:execution.callId,signal:execution.signal});
    },
  });
  return async assignment=>{assignments.set(assignment.source.source_id,assignment);try{return await driver(assignment);}finally{assignments.delete(assignment.source.source_id);}};
}
