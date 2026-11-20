import{readFile,writeFile,mkdir,readdir}from'node:fs/promises';import{join,dirname}from'node:path';import{createRequire}from'node:module';import{pathToFileURL}from'node:url';import{createHash}from'node:crypto';
const once=(s,a,b)=>{if(s.split(a).length!==2)throw Error('native compaction contract changed: '+a.slice(0,90));return s.replace(a,b);};
export function patchCompactionSources(files){
 let s=files['compaction/types.ts'];
 s+=`
/** A committed external context transaction; no native surface nodes are claimed. */
export interface ExternalCompactionResult {
  external: { recorded: true; request_id: string; receipt_id: string; progressed: boolean; publication_ids: readonly string[] }
  text: string
}
export type CompactionOutcome = CompactionResult | ExternalCompactionResult
`;files['compaction/types.ts']=s;
 s=files['compaction/index.ts'];
 s=s.replaceAll("import type { CompactionResult } from './types.ts'","import type { CompactionResult, CompactionOutcome, ExternalCompactionResult } from './types.ts'");
 s=s.replaceAll("export type { CompactionResult } from './types.ts'","export type { CompactionResult, CompactionOutcome, ExternalCompactionResult } from './types.ts'");
 s=once(s,'export abstract class CompactionEngine extends Service {',`export interface ExternalCompactionRequest {
  agent: CompactionAgentContext
  trigger: CompactionTrigger | 'manual' | 'region'
  signal: AbortSignal
  sourceCommandId?: CommandId
  range?: { start: SessionSeq; end: SessionSeq }
}
export interface ExternalCompactionPort {
  handles(agent: CompactionAgentContext): boolean
  compact(request: ExternalCompactionRequest): Promise<ExternalCompactionResult>
}
export abstract class CompactionEngine extends Service {
  private externalPort: ExternalCompactionPort | undefined
  private externalOwners = new WeakSet<CompactionAgentContext>()
  get externalCompactionVersion(): number { return 2 }
  attachExternalCompaction(port: ExternalCompactionPort): () => void {
    if (this.externalPort) throw new Error('one external compaction authority only')
    this.externalPort = port
    return () => { if (this.externalPort === port) this.externalPort = undefined }
  }
  externalCompactionFor(agent: CompactionAgentContext): ExternalCompactionPort | undefined {
    if (this.externalPort?.handles(agent)) { this.externalOwners.add(agent); return this.externalPort }
    if (this.externalOwners.has(agent)) throw new Error('SIC compaction authority is unavailable; original history is preserved')
    return undefined
  }
`);
 s=s.replaceAll('Promise<CompactionResult | null>','Promise<CompactionOutcome | null>');files['compaction/index.ts']=s;
 s=files['compaction-basic/index.ts'];
 s=once(s,'  static inject =', '  get externalBasicVersion(): number { return 1 }\n\n  static inject =');
 s=once(s,'import type { CompactionResult, CompactionTrigger }', 'import type { CompactionOutcome, CompactionTrigger }');
 s=s.replaceAll('CompactionResult','CompactionOutcome');
 s=once(s,'import type { CompactionOutcome, CompactionTrigger }','import type { CompactionResult, CompactionOutcome, CompactionTrigger }');
 s=once(s,'    const logResult = (result: CompactionOutcome, trigger: string): void => {',`    const logResult = (result: CompactionOutcome, trigger: string): void => {
      if ('external' in result) { ctx.logger.info(result.text); return }`);
 s=once(s,'        || agent.session.surface.replaceGeneration <= generation) return next()',`        || (result !== null && 'external' in result
          ? !result.external.progressed
          : agent.session.surface.replaceGeneration <= generation)) return next()`);
 s=once(s,'    const target = routedTarget(agent.session)\n    if (target === undefined) return null',`    signal.throwIfAborted()
    const external = this.externalCompactionFor(agent)
    if (external) return external.compact({ agent, trigger, signal })
    const target = routedTarget(agent.session)
    if (target === undefined) return null`);
 s=once(s,'      result = await this.compactRegion(range.start, range.end, agent, signal)\n      measurement',`      result = await this.compactRegion(range.start, range.end, agent, signal)
      measurement`);
 s=once(s,'  ): Promise<CompactionOutcome> {\n    return compactSurfaceRegion(',`  ): Promise<CompactionResult> {
    const external = this.externalCompactionFor(agent)
    if (external) throw new ManualCompactionError('changed', 'native surface region has no authorized SIC CB range mapping; no compaction was dispatched and original history is preserved')
    return compactSurfaceRegion(`);
 s=once(s,'          operationSignal.throwIfAborted()\n          const range',`          operationSignal.throwIfAborted()
          const external = this.externalCompactionFor(agent)
          if (external) {
            const result = await external.compact({ agent, trigger: 'manual', signal: operationSignal, ...sourceCommandId === undefined ? {} : { sourceCommandId } })
            operationSignal.throwIfAborted()
            try { await this.ctx.sessions.flush(agent.session) }
            catch (error: unknown) { throw new ManualCompactionError('persistence', 'external compaction receipt could not be saved', { cause: error }) }
            return result
          }
          const range`);
 // A selected session cannot continue after its authority callback fails.
 s=once(s,'        } catch (error: unknown) {\n          if (error instanceof TargetPressureConfigError)',`        } catch (error: unknown) {
          if (this.externalCompactionFor(agent)) throw error
          if (error instanceof TargetPressureConfigError)`);
 files['compaction-basic/index.ts']=s;
 s=files['command-compact/index.ts'];
 s=once(s,"    if (result === null) return { kind: 'success', text: 'No compactable history yet.' }",`    if (result === null) return { kind: 'success', text: 'No compactable history yet.' }
    if ('external' in result) return { kind: 'success', text: result.text }`);files['command-compact/index.ts']=s;
 s=files['compaction-tool-result-pruner/index.ts'];
 s=once(s,'export class ToolResultPruner extends Service {',`export interface ExternalPrunePort { handles(session: Session): boolean }
export class ToolResultPruner extends Service {
  private externalPort: ExternalPrunePort | undefined
  private externalSessions = new WeakSet<Session>()
  get externalPruneVersion(): number { return 2 }
  protectExternalSession(session: Session): void { this.externalSessions.add(session) }
  attachExternalPrune(port: ExternalPrunePort): () => void {
    if (this.externalPort) throw new Error('one external prune authority only')
    this.externalPort = port
    return () => { if (this.externalPort === port) this.externalPort = undefined }
  }
`);
 s=once(s,'  pruneSession(session: Session): PruneResult {',`  pruneSession(session: Session): PruneResult {
    // The external context transaction owns text reduction. This synchronous
    // resource helper neither starts another scheduler nor claims replacements.
    if (this.externalPort?.handles(session)) this.externalSessions.add(session)
    if (this.externalSessions.has(session)) return { pruned: [], charsRemoved: 0 }`);files['compaction-tool-result-pruner/index.ts']=s;
 s=files['agent-loop/agent.ts'];
 s=once(s,'  /** Resolve request config and bind its adapter before admitting model-visible input. */',`  /** Resolve the original request waterfall and adapter without logging or dispatching a Worker request. */
  async resolveContextEditCall(signal: AbortSignal): Promise<{ config: LlmCallConfig; preparedCall?: PreparedLlmCall; options: GenerateOptions }> {
    signal.throwIfAborted()
    const phase = this.phase
    const turn = phase.kind === 'running' ? phase.turn : phase.lastTurn + 1
    const step = phase.kind === 'running' ? phase.step : 0
    const resolved = await this.prepareRequest(turn, step, signal)
    signal.throwIfAborted()
    const messages = this.session.deriveMessages()
    Object.freeze(messages)
    const options = Object.freeze({ ...resolved.config, sessionId: this.session.id,
      messages, toolHistory: this.session.toolHistory(),
      tools: (await this.loopCtx.systemPrompt.assemble(assembleContextFor(this, signal))).tools, signal })
    return { ...resolved, options }
  }

  /** Resolve request config and bind its adapter before admitting model-visible input. */`);files['agent-loop/agent.ts']=s;
 return files;
}
export async function prepareCompactionRuntime(dshRoot,outputRoot){
 const packages=['compaction','compaction-basic','compaction-tool-result-pruner','command-compact','agent-loop'],files={},original={},sources=[];
 for(const pkg of packages)for(const e of await readdir(join(dshRoot,'packages',pkg==='agent-loop'?'core':'compaction',pkg,'src'),{withFileTypes:true}))if(e.isFile()&&e.name.endsWith('.ts')){
  const path=join(dshRoot,'packages',pkg==='agent-loop'?'core':'compaction',pkg,'src',e.name),raw=await readFile(path);files[pkg+'/'+e.name]=original[pkg+'/'+e.name]=raw.toString().replaceAll('\r\n','\n');sources.push({path,sha256:createHash('sha256').update(raw).digest('hex')});
 }
 patchCompactionSources(files);await mkdir(outputRoot,{recursive:true});const ts=createRequire(join(dshRoot,'package.json'))('typescript');
 for(const[key,text]of Object.entries(files)){
  const pkg=key.split('/')[0],require=createRequire(join(dshRoot,'packages',pkg==='agent-loop'?'core':'compaction',pkg,'lib/index.js'));
  let js=ts.transpileModule(text,{compilerOptions:{target:ts.ScriptTarget.ES2023,module:ts.ModuleKind.ESNext}}).outputText;
  js=js.replace(/import \{([^}]*\bFiberState\b[^}]*)\} from ['"]@deepseek-ai\/cordis['"];?/g,(_all,names)=>{const keep=names.split(',').map(s=>s.trim()).filter(s=>s!=='FiberState');return keep.length?`import {${keep.join(',')}} from '@deepseek-ai/cordis';`:'';}).replaceAll('FiberState.ACTIVE','2').replaceAll('FiberState.FAILED','3').replaceAll('FiberState.DISPOSED','4').replaceAll('FiberState.UNLOADING','5');
  js=js.replace(/from (['"])([^'"\n]+)\1/g,(all,q,spec)=>spec.startsWith('node:')?all:spec.startsWith('.')?`from '${spec.replace(/\.ts$/,'.mjs')}'`:`from ${JSON.stringify(pathToFileURL(spec==='@deepseek-ai/dsh-compaction'?join(outputRoot,'compaction/index.mjs'):require.resolve(spec)).href)}`);
  const out=join(outputRoot,key.replace(/\.ts$/,'.mjs'));await mkdir(dirname(out),{recursive:true});await writeFile(out,js);await writeFile(join(outputRoot,key),text);
 }
 await writeFile(join(outputRoot,'manifest.json'),JSON.stringify({sources,patch:'selected-session external compaction and native prune protection; native image offload unchanged'},null,2));return{root:outputRoot,files,original};
}
