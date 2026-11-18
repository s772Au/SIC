import {evidencePath} from './test-paths.mjs';
// Opt-in live-test support. Provider credentials stay in this process only.
import {readFile,writeFile,mkdir} from 'node:fs/promises';
import {join} from 'node:path';
export const evidenceRoot=evidencePath('production-readiness-20261118/sdk-live');
export function paidRowUpperCost(row){
 if(!Number.isFinite(row.reserved_upper_cny)||row.reserved_upper_cny<0)throw Error('invalid persisted paid-request reservation');
 const usage=row.usage,values=usage&&[usage.inputTokens,usage.outputTokens,usage.cacheReadTokens??0,usage.cacheWriteTokens??0];
 if(!values||values.some(value=>!Number.isFinite(value)||value<0)||values[0]+values[2]+values[3]===0||values[1]===0)return row.reserved_upper_cny;
 return(values[0]+values[2]+values[3])*2/1e6+values[1]*8/1e6;
}
export function reservePaidRequest(ledger,request,utf8Bytes,{model,maxRequests}){
 const output=request.max_tokens??request.max_completion_tokens??request.max_output_tokens;
 if(request.model!==model||!Number.isSafeInteger(output)||output<1||output>4096)throw Error('live model/output budget mismatch');
 const reserve=utf8Bytes*2/1e6+output*8/1e6,reserved=ledger.requests.reduce((sum,row)=>sum+paidRowUpperCost(row),0);
 if(ledger.requests.length>=maxRequests||reserved+reserve>5)throw Error('AutoDL live request/cost budget exhausted');
 return reserve;
}
export async function autoDL({maxRequests=20,evidenceDirectory=evidenceRoot}={}){
 if(!Number.isSafeInteger(maxRequests)||maxRequests<1||maxRequests>60)throw Error('explicit live request limit must be between 1 and 60');
 const providerFile=process.env.SIC_AUTODL_PROVIDER_FILE;
 if(!providerFile)throw Error('SIC_AUTODL_PROVIDER_FILE is required for paid tests');
 const text=(await readFile(providerFile,'utf8')).split(/Provider2/i)[0];
 const key=text.match(/^\s*Key\s*[:：=]\s*(\S+)/im)?.[1],model=text.match(/^\s*(DeepSeek-[\w.-]+)\s*$/im)?.[1];
 const endpoint=[...text.matchAll(/https:\/\/[^\s]+/g)].map(match=>new URL(match[0])).find(url=>url.hostname==='www.autodl.art'&&url.pathname.endsWith('/anthropic'));
 const openAIEndpoint=[...text.matchAll(/https:\/\/[^\s]+/g)].map(match=>new URL(match[0])).find(url=>url.hostname==='www.autodl.art'&&url.pathname.endsWith('/v1'));
 if(!key||!model||!endpoint)throw Error('AutoDL provider metadata missing');
 const redact=value=>JSON.parse(JSON.stringify(value,(_k,v)=>typeof v==='string'?v.replaceAll(key,'[REDACTED]'):v));
 await mkdir(evidenceDirectory,{recursive:true});const ledgerPath=join(evidenceDirectory,'paid-request-ledger.json');
 let ledger;try{ledger=JSON.parse(await readFile(ledgerPath,'utf8'));}catch(error){if(error.code!=='ENOENT')throw error;ledger={provider:'AutoDL.ART',model,upper_prices_cny_per_million:{input:2,output:8},max_requests:20,max_tokens:4096,max_cny:5,requests:[]};}
 ledger.max_requests=maxRequests;
 const save=async()=>writeFile(ledgerPath,JSON.stringify(redact(ledger),null,2));
 const cost=()=>ledger.requests.reduce((sum,row)=>sum+paidRowUpperCost(row),0);
 const fetchProvider=async(url,init,provenance)=>{
  const target=new URL(url),expected=provenance.protocol==='responses'?openAIEndpoint.pathname+'/responses':provenance.protocol==='openai'?openAIEndpoint.pathname+'/chat/completions':endpoint.pathname+'/v1/messages';
  if(target.origin!==endpoint.origin||target.pathname!==expected||target.username||target.password||target.search||target.hash)throw Error('only the configured AutoDL model endpoint is authorized');
  const request=JSON.parse(init.body),reserve=reservePaidRequest(ledger,request,Buffer.byteLength(init.body),{model,maxRequests});
  const row={number:ledger.requests.length+1,role:provenance.role??'probe',source_id:provenance.source_id??null,protocol:provenance.protocol??'messages',started_at:new Date().toISOString(),reserved_upper_cny:reserve,request:redact(request)};
  ledger.requests.push(row);await save();
  try{const response=await fetch(url,{...init,redirect:'error'});row.status=response.status;row.content_type=response.headers.get('content-type');if(!response.ok)row.provider_error=redact((await response.clone().text()).slice(0,8192));await save();return response;}
  catch(error){row.transport_error=redact(String(error));await save();throw error;}
 };
 const usage=async(role,value)=>{const row=ledger.requests.findLast(row=>row.role===role&&!row.usage);if(!row)throw Error('usage without observed request');row.usage=redact(value);await save();};
 return{model,baseURL:endpoint.href,openAIURL:openAIEndpoint?.href,auth:()=>({'x-api-key':key}),fetchProvider,redact,usage,ledger,save,cost};
}
