// JSON/YAML Cordis entry: existing Harness services supply the native route.
import {apply as applyHost,inject} from './plugin.mjs';
import {declarativeHostConfig,readHostBindings,hostControlClient} from './binding-store.mjs';
export {inject};
export const name='sic-transparent-host';
export async function apply(ctx,input={}){
 if(input.enabled!==true)return;
 const config=declarativeHostConfig(input);config.credential();
 const stored=await readHostBindings(config),bindings=structuredClone(stored.sessions);
 for(const binding of Object.values(bindings))await hostControlClient(config,binding.chain_id).operate('chain.get',{});
 // Resolve only persisted native identities. No automatic Agent/profile/model
 // creation or Provider credentials are introduced at installation time.
 const installed=await applyHost(ctx,{enabled:true,mode:config.mode??'edit',baseline:config.baseline??'fused',actor:config.actor,coreURL:config.coreURL,
  credential:config.credential,projectionRoot:config.projectionRoot,bindingFor:sessionId=>Object.hasOwn(bindings,sessionId)?bindings[sessionId]:undefined,hierarchyPolicy:config.hierarchyPolicy,imageCompression:config.imageCompression,editorLimits:config.editorLimits});
 ctx.reflect.provide('sicTransparent',installed);
}
