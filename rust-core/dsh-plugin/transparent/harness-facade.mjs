const GOALS=new Set(['create_goal','get_goal','update_goal']);
const LIFECYCLE=new Set([...GOALS,'job_list','job_output','job_kill']);

// A Core-issued grant is supplied by the authenticated host bridge. There is
// intentionally no model-facing grant minting API or goal duplication fallback.
export function createGoalFacade({operate,resolveGrant}){
  return createLifecycleFacade({operate,resolveGrant,names:GOALS});
}

export function createLifecycleFacade({operate,resolveGrant,names=LIFECYCLE,executeCancellation}){
  if(typeof operate!=='function'||typeof resolveGrant!=='function')throw TypeError('host operation/grant bindings required');
  return async(name,args,exec)=>{
    if(!names.has(name))throw Error('unbound Harness compatibility tool');
    const grant=await resolveGrant(exec,name,args);
    if(grant?.role!=='WORKER'||!grant.scope_id||!grant.allowed_calls?.includes(name))throw Error('goal call not authorized for this host scope');
    const result=await operate('harness.call',{scope_id:grant.scope_id,name,arguments:args,
      ...(grant.grant_id?{grant_id:grant.grant_id}:{}),...(grant.event_id?{event_id:grant.event_id}:{})},exec.signal);
    const value=result?.result??result;
    if(!Object.hasOwn(value??{},'value'))throw Error('invalid Harness goal result');
    if(value.executor_request){
      if(typeof executeCancellation!=='function')throw Error('native cancellation executor not connected');
      await executeCancellation(value.executor_request,exec);
    }
    return value.value;
  };
}

// Run on an explicitly selected scoped agent.ctx, preserving native parameters,
// output schema/rendering/presentation. The old goal executor is NOT called.
export function installGoalCompatibility({tools,scope,facade}){
  return installLifecycleCompatibility({tools,scope,facade,names:GOALS});
}

export function installLifecycleCompatibility({tools,scope,facade,names=LIFECYCLE}){
  const disposers=[];
  try{
    for(const name of names){
      const original=tools.get(name,scope);
      if(!original)throw Error(`native ${name} definition missing`);
      disposers.push(tools.register({...original,execute:(args,exec)=>facade(name,args,exec)}));
    }
  }catch(error){for(const dispose of disposers.reverse())dispose();throw error;}
  return()=>{for(const dispose of disposers.reverse())dispose();};
}
