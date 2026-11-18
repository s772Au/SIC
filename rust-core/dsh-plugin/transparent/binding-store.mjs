import {readFile,mkdir,open,rename,unlink} from 'node:fs/promises';
import {dirname,isAbsolute} from 'node:path';
import {randomUUID} from 'node:crypto';
import {createCoreClient} from './core-client.mjs';
import {editorLimits} from './editor-driver.mjs';

export function declarativeHostConfig(config){
 const allowed=['enabled','mode','baseline','actor','coreURL','credentialEnv','projectionRoot','bindingsFile','hierarchyPolicy','imageCompression','editorLimits'];
 if(!config||Object.keys(config).some(key=>!allowed.includes(key)))throw Error('unsupported declarative transparent configuration field');
 for(const field of ['actor','coreURL','credentialEnv','projectionRoot','bindingsFile'])if(typeof config[field]!=='string'||!config[field].trim())throw Error('declarative transparent configuration requires '+field);
 if(!isAbsolute(config.projectionRoot)||!isAbsolute(config.bindingsFile))throw Error('projectionRoot and bindingsFile must be absolute durable paths');
 const url=new URL(config.coreURL);if(!['http:','https:'].includes(url.protocol)||url.username||url.password||url.search||url.hash)throw Error('invalid Core control endpoint');
 if(!/^[A-Za-z_][A-Za-z0-9_]{0,127}$/.test(config.credentialEnv))throw Error('invalid Core credential environment reference');
 if(config.mode!==undefined&&!['edit','passthrough'].includes(config.mode))throw Error('invalid transparent mode');
 if(config.baseline!==undefined&&!['native','append-only','fused'].includes(config.baseline))throw Error('invalid reviewed baseline');
 if(config.imageCompression!==undefined&&typeof config.imageCompression!=='boolean')throw Error('imageCompression must be boolean');
 if(config.editorLimits!==undefined)editorLimits(config.editorLimits);
 const credential=()=>{const value=process.env[config.credentialEnv];if(!value)throw Error('the named Core HostAuth environment reference is not configured');return value;};
 return{...structuredClone(config),coreURL:url.href,credential};
}
function validateStore(value,config){
 if(value?.version!==1||value.actor!==config.actor||value.coreURL!==config.coreURL||!value.sessions||typeof value.sessions!=='object'||Array.isArray(value.sessions))throw Error('binding store actor/Core/version does not match this host');
 if(Object.keys(value).some(key=>!['version','actor','coreURL','sessions'].includes(key)))throw Error('unsupported binding store metadata');
 for(const [session,binding]of Object.entries(value.sessions)){
  if(!session||!binding||typeof binding.chain_id!=='string'||!binding.chain_id||typeof binding.scope_id!=='string'||!binding.scope_id||binding.role!=='WORKER')throw Error('binding store requires exact session/chain/scope Worker identities');
  if(Object.keys(binding).some(key=>!['chain_id','scope_id','role','edit_capabilities','editor_readable_sources','max_goal_rounds','lifecycle_calls'].includes(key)))throw Error('unsupported persistent host binding field');
 }
 return value;
}
export async function readHostBindings(config,{allowMissing=false}={}){
 try{return validateStore(JSON.parse(await readFile(config.bindingsFile,'utf8')),config);}
 catch(error){if(error.code==='ENOENT'&&allowMissing)return{version:1,actor:config.actor,coreURL:config.coreURL,sessions:{}};throw error;}
}
export function hostControlClient(config,chain_id=null){return createCoreClient({baseURL:config.coreURL,credential:config.credential,context:()=>({actor:config.actor,chain_id,allow_write:true,allow_model:false,allow_revoke:false})});}
async function writeHostJson(filename,value){
 await mkdir(dirname(filename),{recursive:true});const temporary=filename+'.'+randomUUID()+'.pending';
 const handle=await open(temporary,'wx');try{await handle.writeFile(JSON.stringify(value,null,2)+'\n');await handle.sync();}finally{await handle.close();}
 await rename(temporary,filename);
}
async function creationIntent(filename){
 try{return JSON.parse(await readFile(filename,'utf8'));}catch(error){if(error.code==='ENOENT')return null;if(error instanceof SyntaxError)return{corrupt:true};throw error;}
}
async function clearCompletedIntent(filename,sessionId,chainId,{explicit=false}={}){
 const intent=await creationIntent(filename);if(!intent)return;
 if(intent.corrupt){if(!explicit)throw Error('corrupt creation intent: reconcile with an explicit existing chain');}
 else{if(intent.sessionId!==sessionId)return;if(intent.chainId&&intent.chainId!==chainId)throw Error('creation intent differs from the persisted native binding');}
 try{await unlink(filename);}catch(error){if(error.code!=='ENOENT')throw error;}
}
export async function bindNativeSession(input,{sessionId,chainId,create=false}={}){
 const config=declarativeHostConfig(input);if(typeof sessionId!=='string'||!sessionId.trim()||sessionId.length>256)throw Error('an exact native DSH session identity is required');
 if(create===Boolean(chainId))throw Error('choose either an existing chainId or explicit create');
 config.credential();await mkdir(dirname(config.bindingsFile),{recursive:true});const lockPath=config.bindingsFile+'.lock',lock=await open(lockPath,'wx');
 try{
  await lock.writeFile(JSON.stringify({pid:process.pid,sessionId,created_at:new Date().toISOString()}));await lock.sync();
  const store=await readHostBindings(config,{allowMissing:true}),previous=Object.hasOwn(store.sessions,sessionId)?store.sessions[sessionId]:null,intentPath=config.bindingsFile+'.create-intent';
  if(previous){if(chainId&&previous.chain_id!==chainId)throw Error('native session already binds a different Core chain');await hostControlClient(config,previous.chain_id).operate('chain.get',{});await clearCompletedIntent(intentPath,sessionId,previous.chain_id,{explicit:!create});return{...previous,replayed:true};}
  if(create){
   // An uncertain chain.create is never blindly repeated after a process crash.
   const intent=await creationIntent(intentPath);
   if(intent){if(intent.corrupt)throw Error('corrupt creation intent: reconcile with an explicit existing chain');if(intent.sessionId!==sessionId)throw Error('another native binding creation requires reconciliation');if(!intent.chainId)throw Error('uncertain Core chain creation: reconcile chain.list before binding an existing chain');chainId=intent.chainId;}
   else{await writeHostJson(intentPath,{sessionId,state:'creating'});chainId=(await hostControlClient(config).operate('chain.create',{})).chain_id;await writeHostJson(intentPath,{sessionId,chainId,state:'created'});}
  }
  await hostControlClient(config,chainId).operate('chain.get',{});
  const pending=await creationIntent(intentPath);if(pending&&!pending.corrupt&&pending.sessionId===sessionId&&pending.chainId&&pending.chainId!==chainId)throw Error('existing chain differs from the recorded creation intent');
  const binding={chain_id:chainId,scope_id:'dsh:'+sessionId,role:'WORKER'};
  Object.defineProperty(store.sessions,sessionId,{value:binding,enumerable:true,writable:true,configurable:true});validateStore(store,config);await writeHostJson(config.bindingsFile,store);
  await clearCompletedIntent(intentPath,sessionId,chainId,{explicit:!create});return{...binding,replayed:false};
 }finally{await lock.close();await unlink(lockPath);}
}
