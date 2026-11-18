import {dshSourcePath,fixturePath} from './test-paths.mjs';
import{prepareCompactionRuntime}from'../transparent/prepare-compaction-runtime.mjs';import{createRequire}from'node:module';import{join,relative}from'node:path';
const dsh=dshSourcePath(),output=fixturePath('compaction-runtime-20261118-source-fixture');await prepareCompactionRuntime(dsh,output);
const ts=createRequire(join(dsh,'package.json'))('typescript'),options={noEmit:true,target:ts.ScriptTarget.ES2024,module:ts.ModuleKind.ESNext,moduleResolution:ts.ModuleResolutionKind.Bundler,strict:true,skipLibCheck:true,allowImportingTsExtensions:true,exactOptionalPropertyTypes:true,noUncheckedIndexedAccess:true,noUnusedLocals:true,noUnusedParameters:true,types:['node'],typeRoots:[join(dsh,'node_modules/@types')]},host=ts.createCompilerHost(options);
host.resolveModuleNames=(names,file)=>names.map(name=>{
 if(name==='@deepseek-ai/dsh-compaction')return{resolvedFileName:join(output,'compaction/index.ts'),extension:ts.Extension.Ts};
 if(name==='@deepseek-ai/dsh-compaction-tool-result-pruner')return{resolvedFileName:join(output,'compaction-tool-result-pruner/index.ts'),extension:ts.Extension.Ts};
 if(name==='@deepseek-ai/cosmokit')return{resolvedFileName:join(dsh,'vendor/cosmokit/lib/types/index.d.ts'),extension:ts.Extension.Dts};
 const parts=relative(output,file).split(/[\\/]/),from=file.startsWith(output)?join(dsh,'packages',parts[0]==='agent-loop'?'core':'compaction',parts[0],'src',parts.slice(1).join('/')):file;
 return ts.resolveModuleName(name,name.startsWith('.')?file:from,options,ts.sys).resolvedModule;
});
const program=ts.createProgram(['compaction/index.ts','compaction-basic/index.ts','compaction-tool-result-pruner/index.ts','command-compact/index.ts','agent-loop/index.ts'].map(p=>join(output,p)),options,host),diagnostics=ts.getPreEmitDiagnostics(program);
if(diagnostics.length){console.error(ts.formatDiagnosticsWithColorAndContext(diagnostics,{getCurrentDirectory:()=>output,getCanonicalFileName:p=>p,getNewLine:()=> '\n'}));process.exitCode=1;}else console.log('Patched native compaction, BasicCompaction, pruner, /compact and Agent request resolver source types: PASS');
