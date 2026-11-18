import {fixturePath,evidencePath,testCoreBinary} from './test-paths.mjs';
import{test}from'node:test';import assert from'node:assert/strict';import{spawn,execFile}from'node:child_process';import{promisify}from'node:util';import{mkdir,writeFile,readFile,copyFile}from'node:fs/promises';import{join}from'node:path';import{randomUUID}from'node:crypto';import{pathToFileURL}from'node:url';import{createServer}from'node:net';
import{createCoreClient}from'../transparent/core-client.mjs';
export async function isolated({allowRevoke=false,evidenceRoot=evidencePath('dsh-sic-transparent-comparison-20261117/adaptation/fixtures')}={}){
 const sourceBinary=process.env.SIC_CORE_TEST_BIN??testCoreBinary(),root=join(evidenceRoot,'native-http-'+randomUUID());await mkdir(root,{recursive:true});
 const build=fixturePath('native-http-copies');await mkdir(build,{recursive:true});const binary=join(build,'sic-core-'+randomUUID()+'.exe');await copyFile(sourceBinary,binary);
 const db=join(root,'core.db'),token='synthetic-'+randomUUID(),envName='SIC_NATIVE_HTTP_FIXTURE_TOKEN',env={PATH:process.env.PATH,SystemRoot:process.env.SystemRoot,TEMP:root,TMP:root,[envName]:token};await promisify(execFile)(binary,['init',db],{windowsHide:true,env});
 const reserve=createServer();await new Promise(r=>reserve.listen(0,'127.0.0.1',r));const port=reserve.address().port;await new Promise(r=>reserve.close(r));
 const child=spawn(binary,['serve',db,`127.0.0.1:${port}`,'native-http-actor',envName,...allowRevoke?['--allow-revoke']:[]],{windowsHide:true,env,stdio:['ignore','pipe','pipe']}),baseURL=`http://127.0.0.1:${port}`;let chain=null;
 const close=async()=>{child.kill();if(child.exitCode===null)await new Promise(r=>child.once('exit',r));};
 for(let i=0;i<100;i++){try{if((await fetch(baseURL+'/health',{headers:{authorization:'Bearer '+token}})).ok){const core=createCoreClient({baseURL,credential:()=>token,context:()=>({actor:'native-http-actor',chain_id:chain,allow_write:true,allow_model:false,allow_revoke:false})});chain=(await core.operate('chain.create',{})).chain_id;return{core,root,close,chain_id:chain,baseURL,credential:()=>token};}}catch{}await new Promise(r=>setTimeout(r,50));}await close();throw Error('isolated Core did not become ready');
}
