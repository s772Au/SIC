import {dshSourcePath,fixturePath} from './test-paths.mjs';
// Type-check only the five changed DSH source modules against its real built
// package declarations. No installed source or build output is overwritten.
import {prepareGoalRuntime} from '../transparent/prepare-goal-runtime.mjs';
import {createRequire} from 'node:module';
import {join,relative} from 'node:path';
const dsh=dshSourcePath();
const output=process.env.SIC_GOAL_SOURCE_FIXTURE??fixturePath('goal-runtime-20261117-source-fixture');
await prepareGoalRuntime(dsh,output);
const ts=createRequire(join(dsh,'package.json'))('typescript');
const options={noEmit:true,target:ts.ScriptTarget.ES2024,module:ts.ModuleKind.ESNext,moduleResolution:ts.ModuleResolutionKind.Bundler,
 strict:true,skipLibCheck:true,allowImportingTsExtensions:true,exactOptionalPropertyTypes:true,noUncheckedIndexedAccess:true,noUnusedLocals:true,noUnusedParameters:true,
 types:['node'],typeRoots:[join(dsh,'node_modules/@types')]};
const host=ts.createCompilerHost(options);
host.resolveModuleNames=(names,file)=>names.map(name=>{
 if(name==='@deepseek-ai/dsh-goal')return{resolvedFileName:join(output,'goal/index.ts'),extension:ts.Extension.Ts};
 const from=file.startsWith(output)?join(dsh,'packages/goal',relative(output,file).split(/[\\/]/)[0],'src',relative(output,file).split(/[\\/]/).slice(1).join('/')):file;
 return ts.resolveModuleName(name,name.startsWith('.')?file:from,options,ts.sys).resolvedModule;
});
const program=ts.createProgram(['goal/index.ts','goal-round-driver/index.ts','command-goal/index.ts'].map(p=>join(output,p)),options,host);
const diagnostics=ts.getPreEmitDiagnostics(program);
if(diagnostics.length){console.error(ts.formatDiagnosticsWithColorAndContext(diagnostics,{getCurrentDirectory:()=>output,getCanonicalFileName:p=>p,getNewLine:()=> '\n'}));process.exitCode=1;}
else console.log('Patched native GoalService, command, driver and goal projection types: PASS');
