import {dshSourcePath,fixturePath,evidencePath} from './test-paths.mjs';
import{readFile,writeFile,mkdir}from'node:fs/promises';import{join,dirname}from'node:path';import{fileURLToPath}from'node:url';import{createHash,randomUUID}from'node:crypto';import{execFile}from'node:child_process';import{promisify}from'node:util';
const dsh=dshSourcePath(),patchRoot=process.env.SIC_PATCH_ROOT??fileURLToPath(new URL('../transparent',import.meta.url)),id=randomUUID(),fixture=join(process.env.SIC_PATCH_BUILD_ROOT??fixturePath(),'dsh-patch-composition-'+id),evidence=process.env.SIC_PATCH_EVIDENCE_ROOT??evidencePath('dsh-sic-transparent-comparison-20261117/jobs-runtime');
const hash=b=>createHash('sha256').update(b).digest('hex'),patches=[],sources=new Map();await mkdir(fixture,{recursive:true});await mkdir(evidence,{recursive:true});
for(const name of ['harness-sdk-context.patch','deepseek-native-fetch.patch','goal-external-state.patch','jobs-external-lifecycle.patch','compaction-external-state.patch']){
 const path=join(patchRoot,name),bytes=await readFile(path),snapshot=join(fixture,'_patches',name);await mkdir(dirname(snapshot),{recursive:true});await writeFile(snapshot,bytes);patches.push({path,immutable_snapshot:snapshot,sha256:hash(bytes)});
 for(const match of bytes.toString('utf8').matchAll(/^--- a\/(.+)$/gm)){
  const relative=match[1].trim();if(!relative.startsWith('packages/')||relative.split('/').includes('..')||relative.includes(':'))throw Error('patch path outside isolated fixture');
  if(sources.has(relative))continue;const original=await readFile(join(dsh,relative));sources.set(relative,{relative,original_sha256:hash(original)});const target=join(fixture,relative);await mkdir(dirname(target),{recursive:true});await writeFile(target,original);
 }
}
const execute=promisify(execFile);await execute('git',['apply','--check','--ignore-space-change',...patches.map(p=>p.immutable_snapshot)],{cwd:fixture,windowsHide:true});await execute('git',['apply','--ignore-space-change',...patches.map(p=>p.immutable_snapshot)],{cwd:fixture,windowsHide:true});
for(const row of sources.values()){row.patched_sha256=hash(await readFile(join(fixture,row.relative)));row.original_unchanged=row.original_sha256===hash(await readFile(join(dsh,row.relative)));if(!row.original_unchanged)throw Error('installed DSH source changed during isolated check');}
const report=join(evidence,'patch-composition-'+id+'.json');await writeFile(report,JSON.stringify({result:'PASS',fixture,patches,sources:[...sources.values()],application:'all five source patches checked and actually applied to the same isolated source copy; native checkout unchanged'},null,2));console.log('Five native patches applied together: PASS ('+sources.size+' source files); report '+report);
