// Isolated reviewed LlmRuntime patch. The primary seam is SDK-native and applies
// before ANY original Provider adapter; the Provider HTTP/SSE implementations
// and its registration/choice/model/capacity handling remain unchanged.
import{readFile,writeFile,mkdir}from'node:fs/promises';import{join}from'node:path';import{createRequire}from'node:module';import{pathToFileURL}from'node:url';
const once=(s,a,b)=>{if(s.split(a).length!==2)throw Error('unrecognized actual LlmRuntime source: '+a);return s.replace(a,b);};
const seam=`const contextGuards: Array<(options: GenerateOptions) => void | Promise<void>> = []
      const exchange = {options: this.forAdapter(projectedOptions, adapter)} as ContextDispatchExchange
      Object.defineProperty(exchange, 'addFinalGuard', {value: (guard: (options: GenerateOptions) => void | Promise<void>) => contextGuards.push(guard), writable: false, configurable: false})
      Object.preventExtensions(exchange)
      const stream = this.ctx.waterfall(this, 'llm/context-dispatch', exchange, () => (async function* () {
        for (const guard of contextGuards) await guard(exchange.options)
        yield* dispatch(exchange.options)
      })())`;
export function patchSdkRuntime(source){
 let s=once(source,'export class LlmRuntime extends TypertRemoteService {','export class LlmRuntime extends TypertRemoteService {\n  get contextDispatchVersion(): number { return 1 }');
 s=once(s,'const stream = dispatch(this.forAdapter(projectedOptions, adapter))',seam);
 s+=`\ninterface ContextDispatchExchange {\n  options: GenerateOptions\n  readonly addFinalGuard: (guard: (options: GenerateOptions) => void | Promise<void>) => void\n}\ndeclare module '@deepseek-ai/cordis' { interface Events {\n  'llm/context-dispatch'(this: LlmRuntime, exchange: ContextDispatchExchange, next: () => AsyncIterable<StreamChunk>): AsyncIterable<StreamChunk>\n} }\n`;
 return s;
}
export async function prepareSdkRuntime(dshRoot,outputRoot){
 const source=join(dshRoot,'packages/llm/llm/lib/index.js'),raw=await readFile(source,'utf8');let modified=once(raw,'return class LlmRuntime extends _classSuper {','return class LlmRuntime extends _classSuper {\n get contextDispatchVersion(){return 1;}');
 const js=`const guards=[];const exchange={options:this.forAdapter(projectedOptions,adapter)};Object.defineProperty(exchange,'addFinalGuard',{value:guard=>guards.push(guard),writable:false,configurable:false});Object.preventExtensions(exchange);const stream=this.ctx.waterfall(this,'llm/context-dispatch',exchange,()=> (async function*(){for(const guard of guards)await guard(exchange.options);yield* dispatch(exchange.options);})());iterator=stream[Symbol.asyncIterator]();`;
 modified=once(modified,'iterator = dispatch(this.forAdapter(projectedOptions, adapter))[Symbol.asyncIterator]();',js);
 const require=createRequire(source);modified=modified.replace(/from "([^"\n]+)"/g,(all,spec)=>spec.startsWith('node:')?all:`from ${JSON.stringify(pathToFileURL(require.resolve(spec)).href)}`);
 modified=modified.replaceAll('createRequire(import.meta.url)',`createRequire(${JSON.stringify(pathToFileURL(source).href)})`);
 await mkdir(outputRoot,{recursive:true});const output=join(outputRoot,'dsh-llm-sdk-context.mjs');await writeFile(output,modified,'utf8');
 const sourceTS=join(dshRoot,'packages/llm/llm/src/index.ts'),original=await readFile(sourceTS,'utf8'),patched=patchSdkRuntime(original);await writeFile(join(outputRoot,'typescript-overlay.json'),JSON.stringify({[sourceTS]:patched}));return output;
}
