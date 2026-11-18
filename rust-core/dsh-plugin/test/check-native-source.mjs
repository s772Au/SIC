import {dshSourcePath,fixturePath} from './test-paths.mjs';
import{readFile}from'node:fs/promises';import{createRequire}from'node:module';import{join}from'node:path';
const dsh=dshSourcePath();
const ts=createRequire(join(dsh,'package.json'))('typescript');
const overlay=JSON.parse(await readFile(fixturePath('dsh-transparent-fixture-20261117/typescript-overlay.json'),'utf8'));
const mapped=new Map(Object.entries(overlay).map(([p,s])=>[p.replaceAll('\\','/').toLowerCase(),s]));
const config=ts.readConfigFile(join(dsh,'packages/llm/llm-deepseek/tsconfig.json'),ts.sys.readFile);
const parsed=ts.parseJsonConfigFileContent(config.config,ts.sys,join(dsh,'packages/llm/llm-deepseek'));
const options={...parsed.options,noEmit:true,composite:false,rootDir:undefined,incremental:false};
function check(patched){const host=ts.createCompilerHost(options),read=host.readFile;host.readFile=p=>(patched?mapped.get(p.replaceAll('\\','/').toLowerCase()):undefined)??read(p);return ts.getPreEmitDiagnostics(ts.createProgram(parsed.fileNames,options,host));}
const key=d=>JSON.stringify([d.file?.fileName,d.code,ts.flattenDiagnosticMessageText(d.messageText,'\n')]);
const baseline=new Set(check(false).map(key)),added=check(true).filter(d=>!baseline.has(key(d)));
if(added.length){console.error(ts.formatDiagnosticsWithColorAndContext(added,{getCurrentDirectory:()=>dsh,getCanonicalFileName:p=>p,getNewLine:()=> '\n'}));process.exitCode=1;}else console.log('Native context/fetch source patch: no added TypeScript diagnostics ('+baseline.size+' pre-existing).');
