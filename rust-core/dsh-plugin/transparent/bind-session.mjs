#!/usr/bin/env node
import {readFile} from 'node:fs/promises';
import {bindNativeSession} from './binding-store.mjs';
const argumentsOf=process.argv.slice(2),take=name=>{const index=argumentsOf.indexOf(name);return index<0?undefined:argumentsOf[index+1];};
try{
 if(argumentsOf.includes('--migrate'))throw Error('explicit historical SIC migration is not implemented; native history was not imported or rewritten');
 const options=new Set(['--config','--session','--chain','--create']),seen=new Set();for(let index=0;index<argumentsOf.length;index++){const option=argumentsOf[index];if(!options.has(option)||seen.has(option))throw Error('unsupported or repeated binding option: '+option);seen.add(option);if(option!=='--create'&&(!argumentsOf[++index]||argumentsOf[index].startsWith('--')))throw Error('binding option requires a value: '+option);}
 const filename=take('--config'),sessionId=take('--session');if(!filename||!sessionId)throw Error('usage: bind-session --config <JSON> --session <native ID> (--create | --chain <Core ID>)');
 const result=await bindNativeSession(JSON.parse(await readFile(filename,'utf8')),{sessionId,chainId:take('--chain'),create:argumentsOf.includes('--create')});
 console.log(JSON.stringify({session_id:sessionId,...result,history_admission:'fresh-only',migration_performed:false}));
}catch(error){console.error(JSON.stringify({error:error.message}));process.exitCode=1;}
