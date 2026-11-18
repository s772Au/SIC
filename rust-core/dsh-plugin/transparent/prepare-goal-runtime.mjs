// Reproducible, isolated source patch. Never rewrites the installed DSH tree.
import {readFile,writeFile,mkdir} from 'node:fs/promises';
import {join,dirname} from 'node:path';
import {createRequire} from 'node:module';
import {pathToFileURL} from 'node:url';
import {createHash} from 'node:crypto';

const once=(s,a,b)=>{if(s.split(a).length!==2)throw Error('DSH goal source contract changed: '+a.slice(0,80));return s.replace(a,b);};
export function patchGoalSources(files){
 let s=files['goal/index.ts'];
 s=once(s,"import { SessionSeq } from '@deepseek-ai/dsh-session'","import { SessionSeq, pluginRecordOf } from '@deepseek-ai/dsh-session'");
 s=once(s,'export class GoalService extends TypertRemoteService {',`export interface ExternalGoalStatePort {
  get(): GoalView | undefined
  refresh(): Promise<void>
  start(source: {goalId: string; revision: number; round: number}): Promise<void>
  settle(attempt: {phase: string; cancelled: boolean}): Promise<void>
  disarm(reason?: string): Promise<void>
  mutate(action: string, ref?: GoalRef, request?: unknown, source?: unknown): Promise<any>
}
export class GoalService extends TypertRemoteService {
  private readonly externalPorts = new WeakMap<Agent, ExternalGoalStatePort>()
  private externalDriverOwner: object | undefined
  get externalDriverVersion(): number | undefined { return this.externalDriverOwner ? 1 : undefined }
  attachExternalDriver(owner: object): () => void {
    if (this.externalDriverOwner) throw new Error('only one native goal lifecycle driver may run')
    this.externalDriverOwner = owner
    return () => { if (this.externalDriverOwner === owner) this.externalDriverOwner = undefined }
  }
  get defaultMaxGoalRounds(): number { return this.resolved.defaultMaxGoalRounds }
  externalStateFor(agent: Agent): ExternalGoalStatePort | undefined { return this.externalPorts.get(agent) }
  attachExternalState(agent: Agent, port: ExternalGoalStatePort): () => void {
    this.assertLive(agent)
    if (this.externalPorts.has(agent)) throw new Error('goal state port already attached')
    this.externalPorts.set(agent, port)
    return () => { if (this.externalPorts.get(agent) === port) this.externalPorts.delete(agent) }
  }
`);
 s=once(s,'return this.view(this.state(agent.session), this.runtimeState(agent.session))','return this.externalStateFor(agent)?.get() ?? (this.externalStateFor(agent) ? undefined : this.view(this.state(agent.session), this.runtimeState(agent.session)))');
 s=once(s,'this.setActivation(agent.session, \'disarmed\')\n    const runtime',`if (this.externalStateFor(agent)) throw new Error('external goal lifecycle requires its asynchronous state port')
    this.setActivation(agent.session, 'disarmed')
    const runtime`);
 s=once(s,'return [this.state(agent.session), this.runtimeState(agent.session)]',`if (this.externalStateFor(agent)) throw new Error('external goal mutation requires its asynchronous state port')
    return [this.state(agent.session), this.runtimeState(agent.session)]`);
 for(const op of ['get','edit','pause','resume','complete','clear'])s=once(s,`  @Remote('${op}')\n`, '');
 s=once(s,'remoteExportCreate(agent: Agent, request: CreateGoalRequest): CreateGoalResult {\n    const view = this.create(agent, request)',`async remoteExportCreate(agent: Agent, request: CreateGoalRequest): Promise<CreateGoalResult> {
    const port = this.externalStateFor(agent)
    const view = port ? await port.mutate('create', undefined, request, {remoteInvocation: this.ctx.invocation}) : this.create(agent, request)`);
 const wrappers=['edit','pause','resume','complete','clear'].map(op=>`  @Remote('${op}')
  async remoteExport${op[0].toUpperCase()+op.slice(1)}(agent: Agent, ref: GoalRef${op==='edit'?', request: EditGoalRequest':''}): Promise<${op==='clear'?'GoalRef':'GoalView'}> {
    const port = this.externalStateFor(agent)
    return port ? await port.mutate('${op}', ref, ${op==='edit'?'request':'undefined'}, {remoteInvocation: this.ctx.invocation}) : this.${op}(agent, ref${op==='edit'?', request':''})
  }`).join('\n');
 s=once(s,'\n}\n\nexport default GoalService',`\n  @Remote('get')
  async remoteExportGet(agent: Agent): Promise<GoalView | undefined> {
    await this.externalStateFor(agent)?.refresh()
    return this.get(agent)
  }
`+wrappers+'\n}\n\nexport default GoalService');
 s=once(s,"  if (state.failure !== null) return state",`  const record = pluginRecordOf(event)
  if (record?.type === 'plugin:sic/goal/receipt') {
    const data = record.data as {revision: number; current: GoalProjection | null}
    const current = goalProjectionSchema.parse(data.current)
    if (!Number.isSafeInteger(data.revision) || data.revision < 0) throw new Error('invalid Core receipt projection revision')
    return { current, seenGoalIds: current ? [...new Set([...state.seenGoalIds, current.goal.id])] : state.seenGoalIds,
      failure: null, externalRevision: data.revision }
  }
  if (state.externalRevision !== undefined) return state
  if (state.failure !== null) return state`);
 s=once(s,"  failure: zod.string().min(1).nullable(),","  failure: zod.string().min(1).nullable(),\n  externalRevision: zod.number().int().nonnegative().optional(),");
 s=once(s,'stateVersion: 6,','stateVersion: 7,');
 files['goal/index.ts']=s;
 files['goal/types.ts']=once(files['goal/types.ts'],'export interface GoalProjectionState {','export interface GoalProjectionState {\n  readonly externalRevision?: number');
 files['goal/domain.ts']=once(files['goal/domain.ts'],"declare module '@deepseek-ai/dsh-session/types' {",`declare module '@deepseek-ai/dsh-session/types' {
  interface PluginRecordMap {
    /** Receipt-derived view cache: Core remains authoritative after omission. */
    'plugin:sic/goal/receipt': { revision: number; current: import('./types.ts').GoalProjection | null }
    'plugin:sic/goal/intent': { request_id: string; peer_id: string; action: string; goal_id?: string; revision?: number }
  }`);
 s=files['command-goal/index.ts'];
 s=once(s,'function executeGoalCommand(ctx: Context, invocation: CommandInvocation): CommandResult {','async function executeGoalCommand(ctx: Context, invocation: CommandInvocation): Promise<CommandResult> {');
 s=once(s,'    const current = ctx.goals.get(invocation.agent)',`    const port = ctx.goals.externalStateFor?.(invocation.agent)
    await port?.refresh()
    const mutate = async (action: string, ref?: GoalRef, request?: unknown) => port
      ? port.mutate(action, ref, request, invocation)
      : (ctx.goals as any)[action](invocation.agent, ...ref ? [ref, request] : [request])
    const current = ctx.goals.get(invocation.agent)`);
 s=s.replaceAll('ctx.goals.create(invocation.agent, ',"await mutate('create', undefined, ")
   .replaceAll('ctx.goals.edit(invocation.agent, ',"await mutate('edit', ")
   .replaceAll('ctx.goals.pause(invocation.agent, ',"await mutate('pause', ")
   .replaceAll('ctx.goals.resume(invocation.agent, ',"await mutate('resume', ")
   .replaceAll('ctx.goals.clear(invocation.agent, ',"await mutate('clear', ");
 files['command-goal/index.ts']=s;
 s=files['goal-round-driver/index.ts'];
 s=once(s,'export function apply(ctx: Context): void {','export function apply(ctx: Context): void {\n  const releaseExternalDriver = ctx.goals.attachExternalDriver?.(ctx)');
 s=once(s,'  function disarm(state: DriverState): void {','  async function disarm(state: DriverState): Promise<void> {');
 s=once(s,"      if (goal?.activation === 'armed') ctx.goals.disarm(state.agent)","      const port = ctx.goals.externalStateFor?.(state.agent)\n      if (port) await port.disarm('driver-disarm')\n      else if (goal?.activation === 'armed') ctx.goals.disarm(state.agent)");
 s=once(s,'    if (!readyToDrive(state)) return\n','    if (!readyToDrive(state)) return\n    await ctx.goals.externalStateFor?.(agent)?.refresh()\n    if (!readyToDrive(state)) return\n');
 s=once(s,'    if (attempt !== undefined) {\n      state.attempt = undefined', '    if (attempt !== undefined) {\n      await ctx.goals.externalStateFor?.(agent)?.settle(attempt)\n      state.attempt = undefined');
 s=once(s,"    if (goal.roundsStarted >= goal.maxGoalRounds) {","    if (goal.maxGoalRounds !== undefined && goal.roundsStarted >= goal.maxGoalRounds) {");
 // Keep host queue failures separate from the model's persistent-condition audit.
 s=s.replaceAll('ctx.goals.block(agent, goalRef(', 'block(agent, goalRef(');
 s=once(s,'  /** Preserve claimed step context',`  function block(agent: Agent, ref: GoalRef, reason: {code: string; message: string}): void {
    const port = ctx.goals.externalStateFor?.(agent)
    if (port) void port.disarm(reason.code).catch(error => ctx.logger.warn(String(error)))
    else ctx.goals.block(agent, ref, reason)
  }

  /** Preserve claimed step context`);
 s=once(s,"            if (pause) ctx.goals.pause(agent, goalRef(goal))", "            const port = ctx.goals.externalStateFor?.(agent)\n            if (port) { if (attempt) void port.settle({...attempt, cancelled: true}).catch(error => ctx.logger.warn(String(error))) }\n            else if (pause) ctx.goals.pause(agent, goalRef(goal))");
 s=once(s,'      let valid = false\n      try {','      let valid = false\n      try {\n        await ctx.goals.externalStateFor?.(agent)?.refresh()');
 s=once(s,"      return { ...decision, startsRequestSeries: true }", "      await ctx.goals.externalStateFor?.(agent)?.start(source)\n      return { ...decision, startsRequestSeries: true }");
 s=s.replaceAll('        disarm(state)\n        return','        await disarm(state)\n        return');
 s=once(s,'        disarm(state)\n        const attempt = state.attempt','        waits.push(disarm(state))\n        const attempt = state.attempt');
 s=once(s,'      states.clear()','      states.clear()\n      releaseExternalDriver?.()');
 // Non-awaiting event outlets contain asynchronous failures in the helper.
 files['goal-round-driver/index.ts']=s;
 return files;
}

export async function prepareGoalRuntime(dshRoot,outputRoot){
 const names={'goal':['index.ts','types.ts','domain.ts','fold.ts','runtime.ts'],'goal-round-driver':['index.ts','prompt.ts'],'command-goal':['index.ts']};
 const files={},original={},manifest=[];
 for(const [pkg,names_]of Object.entries(names))for(const name of names_){const path=join(dshRoot,'packages/goal',pkg,'src',name),text=(await readFile(path,'utf8')).replaceAll('\r\n','\n');files[pkg+'/'+name]=text;original[pkg+'/'+name]=text;manifest.push({path,sha256:createHash('sha256').update(await readFile(path)).digest('hex')});}
 patchGoalSources(files);await mkdir(outputRoot,{recursive:true});
 const require=createRequire(join(dshRoot,'package.json')),ts=require('typescript');
 for(const [key,text]of Object.entries(files)){
   const pkg=key.split('/')[0],nativeRequire=createRequire(join(dshRoot,'packages/goal',pkg,'lib/index.js'));
   let js=ts.transpileModule(text,{compilerOptions:{target:ts.ScriptTarget.ES2023,module:ts.ModuleKind.ESNext,experimentalDecorators:false}}).outputText;
   // Isolated transpileModule cannot inline this native exported const enum;
   // DSH's compiled driver emits the same ACTIVE numeric value (2).
   js=js.replace(/import \{ FiberState \} from ['"]@deepseek-ai\/cordis['"];?\n/,'').replaceAll('FiberState.ACTIVE','2');
   js=js.replace(/from (['"])([^'"\n]+)\1/g,(all,q,spec)=>spec.startsWith('node:')?all:spec.startsWith('.')?`from '${spec.replace(/\.ts$/,'.mjs')}'`:`from ${JSON.stringify(pathToFileURL(nativeRequire.resolve(spec)).href)}`);
   const out=join(outputRoot,key.replace(/\.ts$/,'.mjs'));await mkdir(dirname(out),{recursive:true});await writeFile(out,js);
   await writeFile(join(outputRoot,key),text);
 }
 await writeFile(join(outputRoot,'manifest.json'),JSON.stringify({sources:manifest,patch:'GoalService selected-agent external authority port, receipt UI projection, async native command and original unique driver checkpoints'},null,2));
 return {goal:join(outputRoot,'goal/index.mjs'),driver:join(outputRoot,'goal-round-driver/index.mjs'),command:join(outputRoot,'command-goal/index.mjs'),files,original};
}
