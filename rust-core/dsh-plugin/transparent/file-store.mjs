import {mkdir,readFile,readdir,open,link,unlink} from 'node:fs/promises';
import {randomUUID} from 'node:crypto';
import {resolve,join} from 'node:path';
import {createReadStream} from 'node:fs';
import {isDeepStrictEqual} from 'node:util';
import {digest,wireDigest} from './transport.mjs';

// Only newline-terminated trace records have been completely appended. A crash
// may leave a final fragment; readers ignore it and the next writer repairs it.
async function* completeTraceLines(path){
  const input=createReadStream(path,{encoding:'utf8'});let pending='';
  try{
    for await(const chunk of input){
      pending+=chunk;let end;
      while((end=pending.indexOf('\n'))!==-1){const line=pending.slice(0,end);pending=pending.slice(end+1);yield line;}
    }
  }finally{input.destroy();}
}

async function immutableJSON(destination,text,onExisting){
  const temporary=destination+'.'+randomUUID()+'.tmp';
  try{
    const handle=await open(temporary,'wx');
    try{await handle.writeFile(text,'utf8');await handle.sync();}finally{await handle.close();}
    try{await link(temporary,destination);}
    catch(error){if(error.code!=='EEXIST'||!onExisting)throw error;return await onExisting();}
  }finally{try{await unlink(temporary);}catch(error){if(error.code!=='ENOENT')throw error;}}
}

// Immutable per-request records. Root is an explicit host-owned directory.
export function createFileProjectionStore(root){
  const base=resolve(root),sessionDir=s=>join(base,digest(s)),traceCounters=new Map(),traceQueues=new Map();
  const file=(s,id)=>{
    if(typeof id!=='string'||!(/^[a-zA-Z0-9_-]{1,128}$/u).test(id))throw Error('invalid projection identity');
    return join(sessionDir(s),id+'.json');
  };
  async function get(session,id){
    try{
      const v=JSON.parse(await readFile(file(session,id),'utf8'));if(v.session_id!==session||v.id!==id)return null;
      let names=[];try{names=(await readdir(file(session,id)+'.events')).filter(name=>name.endsWith('.json'));}catch(e){if(e.code!=='ENOENT')throw e;}
      if(names.length){
        v.outcomes=[];
        for(const name of names.sort()){
          const text=await readFile(join(file(session,id)+'.events',name),'utf8');
          try{v.outcomes.push(JSON.parse(text));}
          catch(error){if(!(error instanceof SyntaxError))throw error;(v.observation_errors??=[]).push({file:name,error:'incomplete or corrupt observation'});}
        }
      }
      return v;
    }
    catch(e){if(e.code==='ENOENT')return null;throw e;}
  }
  async function save(record){
    if(record.adopted_digest!==(record.raw_adopted===undefined?digest(record.adopted):wireDigest(record.raw_adopted)))throw Error('adopted digest mismatch');
    await mkdir(sessionDir(record.session_id),{recursive:true});
    const text=JSON.stringify(record);
    const destination=file(record.session_id,record.id);
    await immutableJSON(destination,text,async()=>{const old=JSON.parse(await readFile(destination,'utf8')),incoming=JSON.parse(text);if(!isDeepStrictEqual(old,incoming)){const changed=Object.keys(incoming).filter(key=>!isDeepStrictEqual(incoming[key],old[key]));throw Error('EEXIST immutable projection retry changed: '+changed.join(','));}});
  }
  async function list(session,offset=0,limit=20){
    let names;try{names=(await readdir(sessionDir(session))).filter(n=>n.endsWith('.json')).sort();}
    catch(e){if(e.code==='ENOENT')return{rows:[],total:0,next_offset:null};throw e;}
    const rows=[];for(const name of names.slice(offset,offset+limit)){
      const r=await get(session,name.slice(0,-5));if(r)rows.push({id:r.id,request_id:r.request_id,source_id:r.source_id,
        format:r.format,role:r.role,stage:r.stage,adopted_digest:r.adopted_digest});
    }
    return{rows,total:names.length,next_offset:offset+limit<names.length?offset+limit:null};
  }
  async function observe(session,id,event){
    // Observations do not authorize dispatch; a corrupt old observation must
    // not prevent recovery of the independently committed projection.
    const record=JSON.parse(await readFile(file(session,id),'utf8'));
    if(record.session_id!==session||record.id!==id)throw Error('projection not found');
    const dir=file(session,id)+'.events';await mkdir(dir,{recursive:true});
    await immutableJSON(join(dir,Date.now()+'-'+randomUUID()+'.json'),JSON.stringify(event));
  }
  async function recordTrace(session,source,event){
    const dir=join(sessionDir(session),'traces'),path=join(dir,digest(source)+'.jsonl');
    const work=(traceQueues.get(path)??Promise.resolve()).then(async()=>{
      await mkdir(dir,{recursive:true});let sequence=traceCounters.get(path);
      const handle=await open(path,'a+');
      try{
        if(sequence===undefined){
          let end=0;sequence=0;
          for await(const line of completeTraceLines(path)){
            end+=Buffer.byteLength(line+'\n');if(!line)continue;
            const prior=JSON.parse(line).trace_sequence;
            if(!Number.isSafeInteger(prior)||prior<=sequence)throw Error('invalid trace sequence');
            sequence=prior;
          }
          if(end<(await handle.stat()).size){
            // Windows append handles cannot be truncated. Keep append writes
            // on their original handle, and repair with a writable handle.
            const repair=await open(path,'r+');try{await repair.truncate(end);}finally{await repair.close();}
          }
        }
        sequence++;
        await handle.writeFile(JSON.stringify({...event,trace_sequence:sequence})+'\n','utf8');
        // Token chunks use append writes; semantic boundaries make prior chunks
        // durable together. No per-token files or per-token fsync.
        if(event.stage!=='editor-chunk')await handle.sync();
        // Long-lived hosts must not retain a promise/counter for every source.
        traceCounters.delete(path);traceCounters.set(path,sequence);
        if(traceCounters.size>256)traceCounters.delete(traceCounters.keys().next().value);
      }catch(error){traceCounters.delete(path);throw error;
      }finally{await handle.close();}
    });
    const queued=work.catch(()=>{});traceQueues.set(path,queued);
    try{return await work;}finally{if(traceQueues.get(path)===queued)traceQueues.delete(path);}
  }
  async function traces(session,source,offset=0,limit=100){
    if(!Number.isSafeInteger(offset)||offset<0||!Number.isSafeInteger(limit)||limit<1||limit>1000)throw Error('invalid trace page');
    const path=join(sessionDir(session),'traces',digest(source)+'.jsonl');
    await(traceQueues.get(path)??Promise.resolve());
    const rows=[];let index=0,more=false;
    try{for await(const line of completeTraceLines(path)){if(!line)continue;if(index++<offset)continue;if(rows.length===limit){more=true;break;}rows.push(JSON.parse(line));}}
    catch(e){if(e.code!=='ENOENT')throw e;}
    return{rows,next_offset:more?offset+rows.length:null};
  }
  const contextQueues=new Map();
  async function context(session,id,create){
    if(!/^[a-f0-9]{64}$/.test(id))throw Error('invalid context identity');
    const dir=join(sessionDir(session),'contexts'),key=join(dir,id+'.json');if(contextQueues.has(key))return contextQueues.get(key);
    const work=(async()=>{try{return JSON.parse(await readFile(key,'utf8'));}catch(e){if(e.code!=='ENOENT')throw e;}
      const record=await create();await mkdir(dir,{recursive:true});
      return await immutableJSON(key,JSON.stringify(record),async()=>JSON.parse(await readFile(key,'utf8')))??record;
    })();contextQueues.set(key,work);try{return await work;}finally{if(contextQueues.get(key)===work)contextQueues.delete(key);}
  }
  return{save,get,list,observe,recordTrace,traces,context};
}
