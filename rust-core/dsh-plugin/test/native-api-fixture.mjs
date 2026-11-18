import {dshSourcePath} from './test-paths.mjs';
import {join} from 'node:path';
import {pathToFileURL} from 'node:url';
import {createHash} from 'node:crypto';
import {deflateSync} from 'node:zlib';
export const dsh=dshSourcePath();
export const load=p=>import(pathToFileURL(join(dsh,p)).href);

export function redImage(color=[255,0,0]){
 const crc=bytes=>{let n=0xffffffff;for(const byte of bytes){n^=byte;for(let b=0;b<8;b++)n=(n>>>1)^((n&1)?0xedb88320:0);}return(n^0xffffffff)>>>0;};
 const part=(type,data)=>{const tag=Buffer.from(type),size=Buffer.alloc(4),sum=Buffer.alloc(4);size.writeUInt32BE(data.length);sum.writeUInt32BE(crc(Buffer.concat([tag,data])));return Buffer.concat([size,tag,data,sum]);};
 const width=64,height=64,header=Buffer.alloc(13);header.writeUInt32BE(width,0);header.writeUInt32BE(height,4);header[8]=8;header[9]=2;
 const raw=Buffer.alloc(height*(width*3+1));for(let y=0;y<height;y++)for(let x=0;x<width;x++)raw.set(color,y*(width*3+1)+1+x*3);
 const data=Buffer.concat([Buffer.from([137,80,78,71,13,10,26,10]),part('IHDR',header),part('IDAT',deflateSync(raw)),part('IEND',Buffer.alloc(0))]);
 const sha=createHash('sha256').update(data).digest('hex'),ref={attachmentId:'sha256:'+sha,mediaType:'image/png',bytes:data.length,width,height};
 return{ref,data,sha,store:{readImageRequest:async actual=>{if(actual.attachmentId!==ref.attachmentId)throw Error('unknown actual attachment');return{variantId:'sha256:'+sha,attachment:ref,data,mediaType:'image/png',bytes:data.length,width,height,depth:'uchar',space:'srgb',hasAlpha:false};},imageHostPath:()=>undefined}};
}

/** These are the original DSH adapters and their own serializers/parsers. The
 * fetch hook observes native bodies; it does not translate any protocol.
 */
export async function installNativeApi(ctx,{protocol,model,baseURL,fetchNative,auth,buildRoot,image,prepareDeepSeekFixture,observeOptions=()=>{}}){
 const api=await load('packages/llm/llm/lib/index.js'),provider='actual-'+protocol;
 const register=adapter=>{const prepare=adapter.prepareCall.bind(adapter);adapter.prepareCall=async(...args)=>{const prepared=await prepare(...args);return{...prepared,stream:options=>{observeOptions(options);return prepared.stream(options);}};};ctx.llm.registerAdapter([provider],adapter);};
 if(protocol==='messages'){
  const output=await prepareDeepSeekFixture(dsh,buildRoot),{DeepSeekAdapter,resolveAdapterOptions,DeepSeekFileStore}=await import(pathToFileURL(output).href);
  // Exercise the original documented inline fallback, without uploading files
  // or sending credentials to a second external resource during acceptance.
  const files=new DeepSeekFileStore({fetch:async()=>new Response('{"error":{"message":"Files resource intentionally unavailable in isolated inline acceptance"}}',{status:404,headers:{'content-type':'application/json'}})});
  register(new DeepSeekAdapter({options:()=>resolveAdapterOptions({baseURL,thinking:'disabled',reasoningEffort:'off',maxTokens:2048,models:[{id:model,inputModalities:image?['text','image']:['text']}],retryPolicy:{mode:'normal',maxRetries:0}}),resolveAuth:async()=>({headers:auth()}),resolveUserId:()=> 'sic-api-acceptance',resolveAttachments:()=>image?.store,resolveImageAccess:()=>undefined,resolveFiles:()=>files,prepareExtensions:async()=>({fields:{},accept:async()=>{}}),fetch:fetchNative}));
 }else{
  const apiName=protocol==='chat'?'openai-completions':'openai-responses';
  const {PiAiAdapter}=await load('packages/llm/llm-pi-ai/lib/index.js'),module=await load('packages/llm/llm-pi-ai/node_modules/@earendil-works/pi-ai/dist/api/'+apiName+'.lazy.js');
  const native=(protocol==='chat'?module.openAICompletionsApi:module.openAIResponsesApi)();
  const descriptor={id:model,name:model,provider,api:apiName,baseUrl:baseURL,reasoning:false,input:image?['text','image']:['text'],contextWindow:32768,maxTokens:4096,cost:{input:2,output:8,cacheRead:2,cacheWrite:2},compat:{supportsDeveloperRole:false,supportsStore:false,supportsUsageInStreaming:true,maxTokensField:'max_tokens'}};
  const piProvider={id:provider,name:'Original DSH native '+apiName,baseUrl:baseURL,auth:{apiKey:{name:'process-only credential',resolve:async({credential})=>({auth:{apiKey:credential?.key},source:'isolated'})}},getModels:()=>[descriptor],stream:(m,c,o)=>native.stream(m,c,{...o,fetch:fetchNative}),streamSimple:(m,c,o)=>native.streamSimple(m,c,{...o,fetch:fetchNative})};
  const profiles=new Map([[provider,{provider,displayName:'Native API acceptance',api:apiName,baseURL,piProvider,modelErrors:new Map(),configuredMaxTokens:new Map([[model,2048]]),retryPolicy:api.resolveRetryPolicy({mode:'normal',maxRetries:0},provider),streamIdleTimeoutMs:120000,timeoutMs:120000,maxRequestImageBytes:1048576,requestImagePixelBudget:262144,requestImageMaxBytes:1048576}]]);
  const noStorage={credentials:{read:async()=>undefined,list:async()=>[],modify:async()=>{throw Error('credential storage disabled');},delete:async()=>{throw Error('credential storage disabled');}},authContext:{env:async()=>undefined,fileExists:async()=>false}};
  register(new PiAiAdapter({profiles:()=>profiles,resolveApiKey:async()=>auth()['x-api-key'],auth:noStorage,resolveAttachments:()=>image?.store,resolveImageAccess:()=>undefined}));
 }
 return{provider,model};
}
