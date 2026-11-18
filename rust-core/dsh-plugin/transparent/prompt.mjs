import { digest } from './transport.mjs';

export function validateTools(tools) {
  if(!Array.isArray(tools)||!tools.length)throw Error('nonempty native registry required');
  const names=new Set();
  for(const t of tools) {
    if(!t?.name||names.has(t.name)||t.parameters?.type!=='object'||!t.parameters.properties)throw Error('invalid native tool shape');
    names.add(t.name);
    for(const key of t.parameters.required??[])if(!Object.hasOwn(t.parameters.properties,key))throw Error('missing required native parameter');
  }
  for(const name of ['read','write','edit','glob','grep','bash']) {
    const t=tools.find(x=>x.name===name);
    if(!t?.parameters.required?.length)throw Error('missing core native schema');
  }
}

/** Guard the FINAL assembly after complete restoration and context refresh. */
export function createAdmissionGuard({system,tools,contextName='sic:activation',authorization}) {
  validateTools(tools);
  if(typeof authorization!=='function')throw TypeError('host authorization resolver required');
  const toolsHash=digest(tools);
  return ({renderedSystem,tools:actual,contexts,scope})=>{
    if(renderedSystem!==system)throw Error('final system prompt changed after adaptation');
    validateTools(actual);
    if(digest(actual)!==toolsHash)throw Error('final native schema changed');
    const auth=authorization(scope);
    if(!auth||!['WORKER','EDITOR'].includes(auth.role))throw Error('missing host authorization');
    const activation=contexts.find(c=>c.name===contextName);
    if(!activation||activation.text!==JSON.stringify(auth))throw Error('runtime activation changed or missing');
    return auth;
  };
}

// Install in an explicitly selected agent.ctx; no global/profile changes.
// Full replacement is limited to the reviewed snapshot. Unknown live sections
// must be reconciled before this function is called, never silently deleted.
export async function registerReviewedPrompt({systemPrompt,scope,expectedNative,renderPrompt,system,authorization}) {
  const baseline=await systemPrompt.assemble({scope});
  if(renderPrompt(baseline).trimEnd()!==expectedNative.trimEnd())throw Error('unreviewed native prompt composition');
  const disposeSection=systemPrompt.section({name:'sic:reviewed-frozen',order:0,text:system,interpolate:false,complete:true});
  const disposeContext=systemPrompt.context({name:'sic:activation',order:1000,required:true,interpolate:false,
    text:context=>JSON.stringify(authorization(context.scope))});
  return()=>{disposeContext();disposeSection();};
}
