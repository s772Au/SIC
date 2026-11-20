import {nativeToolResults} from './tool-linkage.mjs';

export const SOURCE_PAGE='sic_source_text_page';
export const SOURCE_SEARCH='sic_memory_search';
export const ARCHIVE_EXPAND='sic_hierarchy_expand';
export const ARCHIVE_OPS='sic_memory_page';
const archiveResultTools=new Set([SOURCE_PAGE,SOURCE_SEARCH,ARCHIVE_EXPAND,ARCHIVE_OPS]);

export class SourcePageRequestError extends Error {
 constructor(message, recovery='Use the current authenticated source ID and an assigned unit ID, or an explicitly granted source and unit.'){
  super(`${message} ${recovery} Continue the edit with the material already available.`);
 }
}

// The Worker must see each result before the Editor can summarize it. A user
// message queued while tools are running does not consume any tool result.
export function protectedWorkerText(history,preserveQuestion=false){
 const protectedIds=new Set();
 const question=preserveQuestion&&history.findLast(message=>message.role==='user'&&message.source?.kind==='user');
 if(question)protectedIds.add(question.id);
 const latestModel=history.findLastIndex(message=>message.role==='assistant'&&message.source?.kind==='model');
 const pairs=nativeToolResults(history);
 for(const message of history.slice(latestModel+1)){
  if(message.role==='tool'&&archiveResultTools.has(pairs.get(message.id)?.call.name))protectedIds.add(message.id);
 }
 return protectedIds;
}

export function sharedOperationIds(tools,messages){
 if(!tools?.some(tool=>tool.name===ARCHIVE_OPS))return [];
 return messages.filter(message=>message.source?.kind==='sic-cb').map(message=>message.source.cb_id);
}

export function readEditorOperations({core,authority,args,signal}){
 if(args?.kind!=='ops'||!authority.read_only_cb_ids?.includes(args.object_id)){
  throw new SourcePageRequestError('The operation page is outside the shared CB background for this Editor.',
   'Use kind "ops" and an object_id from read_only_cb_ids in the current activation.');
 }
 return core.operate('memory.archive_ops',args,signal);
}

export function hasNativePageGrant(readableSources,args){
 return (readableSources??[]).some(source=>source.kind==='native-source'&&source.source_id===args?.source_id&&source.unit_ids?.includes(args?.unit_id));
}

export async function sourcePageToolResult(read){
 try{return {isError:false,content:[{type:'text',text:JSON.stringify(await read())}]};}
 catch(error){
  if(!(error instanceof SourcePageRequestError))throw error;
  return {isError:true,content:[{type:'text',text:error.message}]};
 }
}

export function pageSourceText(text,offset,limit){
 const scalars=Array.from(text);
 return{text:scalars.slice(offset,offset+limit).join(''),next_offset:offset+limit<scalars.length?offset+limit:null};
}

export async function readNativeSourcePage({core,authority,args,signal}){
 const offset=args?.offset??0,limit=args?.limit??4096;
 if(typeof args?.source_id!=='string'||typeof args?.unit_id!=='string'||!Number.isSafeInteger(offset)||offset<0||!Number.isSafeInteger(limit)||limit<1||limit>8192)throw new SourcePageRequestError('Invalid source page arguments.');
 const own=args.source_id===authority?.source_id&&authority.assigned_unit_ids?.includes(args.unit_id);
 const extra=hasNativePageGrant(authority?.readable_sources,args);
 if(!own&&!extra)throw new SourcePageRequestError('The requested source page is outside the authenticated read scope.');
 const source=await core.get({source_id:args.source_id},signal);
 if(own){
  if(Array.isArray(source.binding?.editable_unit_ids)&&!source.binding.editable_unit_ids.includes(args.unit_id))throw Error('source page is outside the persisted candidate scope');
  if(authority.source_digest!==undefined&&authority.source_digest!==source.source_digest||authority.expected_version!==undefined&&authority.expected_version!==source.version)throw Error('source page authority is stale');
 }
 const unit=source.units.find(u=>u.unit_id===args.unit_id);if(!unit)throw Error('authorized source unit missing');
 const text=unit.anchor.reduce((v,k)=>v[k],source.source_request);
 if(typeof text!=='string')throw Error('authorized native source text missing');
 return{source_id:args.source_id,unit_id:args.unit_id,offset,...pageSourceText(text,offset,limit)};
}
