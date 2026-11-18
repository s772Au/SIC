// A reviewed Agent cannot silently resume native execution after the plugin
// that owns its adopted context has unloaded. These guards belong to the Agent
// scope and intentionally survive the installation until that Agent is closed.
const owners=new WeakMap();
const stopped='SIC installation is unavailable; recreate the Agent with a reviewed composition';

export function createInstallationLifetime(ctx){
  let live=true;const fiber=ctx.fiber,agents=new Set();
  function revoke(){
    if(!live)return;live=false;
    for(const agent of agents){
      if(agent.status==='running'&&typeof agent.cancel==='function'){
        try{agent.cancel({kind:'hook',reason:stopped});}catch{try{ctx.logger?.warn('SIC could not cancel an Agent during installation teardown');}catch{}}
      }
    }
  }
  function verify(){
    // Cordis' public FiberState const enum: FAILED=3, DISPOSED=4,
    // UNLOADING=5. The UID is cleared before its first dispose notification.
    if(!live||fiber&&(fiber.uid===null||[3,4,5].includes(fiber.state))){revoke();throw Error(stopped);}
  }
  const lifetime={verify,revoke,async mount(agent){
    verify();
    if(owners.has(agent)){
      if(owners.get(agent)!==lifetime)throw Error(stopped);
      return;
    }
    owners.set(agent,lifetime);agents.add(agent);
    agent.ctx.on('internal/plugin',changed=>{if(changed===fiber&&changed.uid===null)revoke();},{global:true});
    agent.ctx.on('llm/context-dispatch',async function*(exchange,next){
      if(exchange.options?.sessionId===agent.session.id){verify();exchange.addFinalGuard(verify);}
      yield*next();
    },{global:true,prepend:true});
    agent.ctx.on('llm/stream',async function*(options,next){
      if(options?.sessionId===agent.session.id)verify();
      yield*next();
    },{global:true,prepend:true});
    agent.ctx.on('llm/native-fetch',async(exchange,next)=>{
      if(exchange.provenance?.options?.sessionId===agent.session.id){verify();exchange.addFinalGuard(verify);}
      return next();
    },{global:true,prepend:true});
    await new Promise((resolve,reject)=>agent.ctx.inject(['tools'],child=>{
      try{child.tools.guard(exec=>{if(exec.agent===agent){try{verify();}catch{return stopped;}}});resolve();}catch(error){reject(error);}
    }));
    agent.ctx.effect(()=>()=>{agents.delete(agent);});
  }};
  ctx.effect(()=>revoke);
  return lifetime;
}
