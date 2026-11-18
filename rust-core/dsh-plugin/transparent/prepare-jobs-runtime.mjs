import{readFile,writeFile,mkdir,mkdtemp,readdir}from'node:fs/promises';import{join,dirname,resolve}from'node:path';import{createRequire}from'node:module';import{fileURLToPath,pathToFileURL}from'node:url';import{createHash}from'node:crypto';import{execFile}from'node:child_process';import{promisify}from'node:util';
const once=(s,a,b)=>{if(s.split(a).length!==2)throw Error('unreviewed DSH lifecycle source: '+a.slice(0,85));return s.replace(a,b);};
export function patchJobsSources(files){
 files['jobs/jobs/types.ts']+=`\n\ndeclare module '@deepseek-ai/dsh-session/types' {
  interface PluginRecordMap {
    'plugin:sic/jobs/receipt': {object_id: string; revision: number; external_id: string | null; status: string; receipt_id: string | null}
    'plugin:sic/jobs/collection': {lifecycle_object_id: string; collection_receipt_id: string; collection_digest: string; chain_id: string | null; message_id: string; call_id: string; material: {block_index: number; utf8_start: number; utf8_end: number}}
  }
}\n`;
 let s=files['jobs/jobs/index.ts'];
 s=once(s,'export abstract class JobRegistry extends Service {',`export interface ExternalJobsLifecycle {
  handles(owner: SessionId | undefined): boolean
  beforeStart(spec: JobSpec): void
  reserve(id: JobId, spec: JobSpec): void
  deliver(event: import('./types.ts').JobEvent, owner: SessionId, snapshot: {job: JobView; read: JobOutputRead; result?: string}, next: () => void): void
  list(owner: SessionId): JobView[]
  output(id: JobId, from: number, owner: SessionId): JobOutputRead
  consumed(id: JobId, owner: SessionId): void
  kill(id: JobId, owner: SessionId, reason?: string, invocation?: unknown): Promise<'requested' | 'already-finished'>
  control<T>(name: string, args: unknown, exec: unknown, waitTimeoutMs?: number): Promise<T>
  collectionMeta(): {sic_collection?: {lifecycle_object_id: string; collection_receipt_id: string; collection_digest: string; chain_id: string | null}}
  drain(): Promise<unknown>
}
export abstract class JobRegistry extends Service {
  private readonly externalLaunchTools = new WeakMap<object, 'job' | 'peer' | 'workflow'>()
  registerExternalLaunchTool(definition: object, kind: 'job' | 'peer' | 'workflow'): void { this.externalLaunchTools.set(definition, kind) }
  externalLaunchKind(definition: object | undefined): 'job' | 'peer' | 'workflow' | undefined { return definition ? this.externalLaunchTools.get(definition) : undefined }
  externalControlVersion: number | undefined
  get externalExecutorJoinVersion(): number { return 0 }
  externalLifecycle: ExternalJobsLifecycle | undefined
  attachExternalLifecycle(port: ExternalJobsLifecycle): () => void {
    if (this.externalLifecycle) throw new Error('one external job lifecycle authority only')
    this.externalLifecycle = port
    return () => { if (this.externalLifecycle === port) this.externalLifecycle = undefined }
  }
  cancelExecutor(id: JobId, caller?: SessionId, reason?: string): 'requested' | 'already-finished' { return this.kill(id, caller, reason) }
  joinExecutor(_id: JobId, _caller?: SessionId): Promise<void> { return Promise.reject(new Error('native executor settlement join port required')) }
  acknowledgeCollection(_id: JobId, _throughByte: number, _resultCollected: boolean, _caller?: SessionId): JobView | undefined { return undefined }
`);files['jobs/jobs/index.ts']=s;
 s=files['jobs/jobs-local/index.ts'];
 s=once(s,'  async wait(id: JobId, timeoutMs: number, caller?: SessionId, signal?: AbortSignal): Promise<JobView> {',`  override get externalExecutorJoinVersion(): number { return 1 }
  override async joinExecutor(id: JobId, caller?: SessionId): Promise<void> {
    // Native removal/owner disposal only drops records after their settlement.
    // A resource already retired by that path needs no second lookup/join.
    const job = this.store.get(id)
    if (job === undefined) return
    this.expect(id, caller)
    await job.settled
  }
  async wait(id: JobId, timeoutMs: number, caller?: SessionId, signal?: AbortSignal): Promise<JobView> {`);
 s=once(s,'    // The producer uses the same short UUID identity', '    this.externalLifecycle?.beforeStart(spec)\n\n    // The producer uses the same short UUID identity');
 s=once(s,'    const hooks = spec.run(handle)','    this.externalLifecycle?.reserve(id, spec)\n    const hooks = spec.run(handle)');
 s=once(s,'  list(caller?: SessionId): JobView[] {','  list(caller?: SessionId): JobView[] {\n    if (caller !== undefined && this.externalLifecycle?.handles(caller)) return this.externalLifecycle.list(caller)');
 s=once(s,'  get(id: JobId, caller?: SessionId): JobView {',`  get(id: JobId, caller?: SessionId): JobView {
    if (caller !== undefined && this.externalLifecycle?.handles(caller)) {
      const job = this.externalLifecycle.list(caller).find(job => job.id === id)
      if (!job) throw new Error('job is not visible in authoritative owner scope')
      return job
    }`);
 s=once(s,'  read(id: JobId, caller?: SessionId): JobRead {',`  override acknowledgeCollection(id: JobId, throughByte: number, resultCollected: boolean, caller?: SessionId): JobView {
    const job = this.expect(id, caller)
    if (!Number.isSafeInteger(throughByte) || throughByte < 0 || throughByte > job.ring.total) throw new Error('invalid authoritative collection resource watermark')
    job.modelCursor = Math.max(job.modelCursor, throughByte)
    if (resultCollected) job.resultDelivered = true
    if (isTerminal(job.status)) job.ring.trim(Math.max(this.settledRetainBytes, job.ring.total - job.modelCursor))
    return this.view(job)
  }
  read(id: JobId, caller?: SessionId): JobRead {`);
 s=once(s,'    return this.readJob(this.expect(id, caller))',`    const result = this.readJob(this.expect(id, caller))
    if (caller !== undefined && this.externalLifecycle?.handles(caller)) this.externalLifecycle.consumed(id, caller)
    return result`);
 s=once(s,'  readAt(id: JobId, from: number, caller?: SessionId): JobOutputRead {','  readAt(id: JobId, from: number, caller?: SessionId): JobOutputRead {\n    if (caller !== undefined && this.externalLifecycle?.handles(caller)) return this.externalLifecycle.output(id, from, caller)');
 s=once(s,"  kill(id: JobId, caller?: SessionId, reason?: string): 'requested' | 'already-finished' {",`  override cancelExecutor(id: JobId, caller?: SessionId, reason?: string): 'requested' | 'already-finished' {
    return this.killJob(this.expect(id, caller), reason)
  }
  kill(id: JobId, caller?: SessionId, reason?: string): 'requested' | 'already-finished' {
    if (this.externalLifecycle?.handles(caller)) throw new Error('external job cancellation requires asynchronous durable admission')`);
 s=once(s,'    this.hub.emit(event, owner)',`    if (owner !== undefined && this.externalLifecycle?.handles(owner.id)) {
      const id = event.type === 'output' ? event.id : event.job.id
      const job = this.store.get(id)
      const snapshot = job ? {job: this.view(job), read: job.ring.readFrom(0), ...job.result === undefined ? {} : {result: job.result}}
        : {job: (event as Exclude<JobEvent, {type: 'output'}>).job, read: {chunks: [], next: 0, lossy: false}}
      this.externalLifecycle.deliver(event, owner.id, snapshot, () => this.hub.emit(event, owner))
    } else this.hub.emit(event, owner)`);
 s=once(s,'    this.drop(owned)','    this.drop(owned)\n    await this.externalLifecycle?.drain()');
 s=once(s,'    await Promise.all(ownerCleanups.map(cleanup => Promise.resolve(cleanup())))','    await Promise.all(ownerCleanups.map(cleanup => Promise.resolve(cleanup())))\n    await this.externalLifecycle?.drain()');
 files['jobs/jobs-local/index.ts']=s;
 s=files['jobs/tool-jobs/index.ts'];
 s=once(s,"  ctx.jobs.attachController('tool-jobs')","  ctx.jobs.attachController('tool-jobs')\n  ctx.jobs.externalControlVersion = 1");
 s=once(s,'    async execute(args, exec) {\n      const id = validateJobId(args.job_id)',`    async execute(args, exec) {
      if (ctx.jobs.externalLifecycle?.handles(exec.agent?.id)) return ctx.jobs.externalLifecycle.control<{text: string; job: PublicJobSnapshot}>('job_output', args, exec, Math.min(args.timeout_ms ?? waitDefault, waitCap))
      const id = validateJobId(args.job_id)`);
 s=once(s,'      render: (_args, value) => {\n        const body = value.text.length',`      presentationMeta: () => ctx.jobs.externalLifecycle?.collectionMeta() ?? {},
      render: (_args, value) => {
        const body = value.text.length`);
 s=once(s,'    execute(_args, exec) {\n      const jobs = ctx.jobs.list',`    execute(_args, exec) {
      if (ctx.jobs.externalLifecycle?.handles(exec.agent?.id)) return ctx.jobs.externalLifecycle.control<PublicJobSnapshot[]>('job_list', _args, exec)
      const jobs = ctx.jobs.list`);
 s=once(s,'    execute(args, exec) {\n      const id = validateJobId(args.job_id)',`    execute(args, exec) {
      if (ctx.jobs.externalLifecycle?.handles(exec.agent?.id)) return ctx.jobs.externalLifecycle.control<{outcome: 'cancellation-requested' | 'already-finished'; job: PublicJobSnapshot}>('job_kill', args, exec)
      const id = validateJobId(args.job_id)`);files['jobs/tool-jobs/index.ts']=s;
 for(const pkg of ['tool-bash','tool-pwsh']){
  const key='shell/'+pkg+'/index.ts';if(!files[key])continue;
  files[key]=files[key].replaceAll('registry.kill(attached.id, owner, reason)',"await (owner !== undefined && registry.externalLifecycle?.handles(owner) ? registry.externalLifecycle.kill(attached.id, owner, reason) : registry.kill(attached.id, owner, reason))");
  files[key]=once(files[key],'return defineTool({','const definition = defineTool({');
  files[key]=once(files[key],"    })\n  }\n\n  // The background surface follows","    })\n    jobs?.registerExternalLaunchTool(definition, 'job')\n    return definition\n  }\n\n  // The background surface follows");
 }
 s=files['subagent/tool-subagent/index.ts'];s=once(s,'      const disposeTool = runtimeCtx.effect(() => {',"      runtimeCtx.inject(['jobs'], child => { child.jobs.registerExternalLaunchTool(definition, 'peer') })\n      const disposeTool = runtimeCtx.effect(() => {");files['subagent/tool-subagent/index.ts']=s;
 s=files['workflow/tool-workflow/index.ts'];s=once(s,'  ctx.tools.register(defineTool({','  const definition = defineTool({');s=once(s,"    presentResult: (args, result) => presentWorkflowResult(args, result),\n  }))","    presentResult: (args, result) => presentWorkflowResult(args, result),\n  })\n  ctx.inject(['jobs'], child => { child.jobs.registerExternalLaunchTool(definition, 'workflow') })\n  ctx.tools.register(definition)");files['workflow/tool-workflow/index.ts']=s;
 s=files['api/job-controller/index.ts'];
 s=once(s,'  kill(request: JobKillRequest): JobKillValue {','  async kill(request: JobKillRequest): Promise<JobKillValue> {');
 s=once(s,"    const outcome = jobs.kill(request.jobId, request.sessionId, 'cancelled by the user')",`    const outcome = jobs.externalLifecycle?.handles(request.sessionId)
      ? await jobs.externalLifecycle.kill(request.jobId, request.sessionId, 'cancelled by the user', this.ctx.invocation)
      : jobs.kill(request.jobId, request.sessionId, 'cancelled by the user')`);files['api/job-controller/index.ts']=s;
 s=files['subagent/subagent/index.ts'];
 s="import type { SubagentResult as ExternalSubagentResult } from './types.ts'\n"+s;
 s=once(s,'export class SubagentRuntime extends TypertRemoteService {',`export interface ExternalSubagentLifecycle {
  prepare(parent: Agent, nativeId: string, provider: string, kind: 'local' | 'external'): Promise<unknown>
  attach(parent: Agent, childId: string, dispose: () => Promise<void>, claim: unknown): Promise<void>
  settle(parent: Agent, childId: string, result: ExternalSubagentResult, claim: unknown): Promise<void>
}
export class SubagentRuntime extends TypertRemoteService {
  externalLifecycle: ExternalSubagentLifecycle | undefined
  get externalPeerResidencyVersion(): number { return 2 }
  attachExternalLifecycle(port: ExternalSubagentLifecycle): () => void {
    if (this.externalLifecycle) throw new Error('one external subagent authority only')
    this.externalLifecycle = port
    return () => { if (this.externalLifecycle === port) this.externalLifecycle = undefined }
  }
`);
 s=once(s,'        startExternal: (name, request) => {',`        prepareActivation: (parent, id, provider, kind) => this.externalLifecycle?.prepare(parent, id, provider, kind),
        attachActivation: (parent, id, dispose, claim) => this.externalLifecycle?.attach(parent, id, dispose, claim),
        settleActivation: (parent, id, result, claim) => this.externalLifecycle?.settle(parent, id, result, claim),
        startExternal: (name, request) => {`);files['subagent/subagent/index.ts']=s;
 s=files['subagent/subagent/manager.ts'];
 s=once(s,'interface SubagentHost {',`interface SubagentHost {
  prepareActivation(parent: Agent, id: string, provider: string, kind: 'local' | 'external'): unknown | Promise<unknown>
  attachActivation(parent: Agent, id: string, dispose: () => Promise<void>, claim: unknown): void | Promise<void>
  settleActivation(parent: Agent, id: string, result: SubagentResult, claim: unknown): void | Promise<void>`);
 s=once(s,'  private readonly resident =', '  private readonly externalClaims = new WeakMap<object, unknown>()\n  private readonly resident =');
 s=once(s,'    void activation.result.promise.catch', '    this.externalClaims.set(activation, externalClaim)\n    void activation.result.promise.catch');
 s=once(s,'    let resource: ActivationResource','    const externalClaim = await this.host.prepareActivation(parent, childId, provider, inputs.kind)\n    let resource: ActivationResource');
 s=once(s,'      observer.start(activationAgent(activation))','      await this.host.attachActivation(parent, childId, () => this.dispose(activation), externalClaim)\n      observer.start(activationAgent(activation))');
 s=once(s,'    this.notifySettlement(activation, result)','    await this.host.settleActivation(activation.parent, activation.childId, terminal, this.externalClaims.get(activation))\n    this.notifySettlement(activation, result)');files['subagent/subagent/manager.ts']=s;
 s=files['workflow/workflow/index.ts'];
 s=once(s,'export abstract class WorkflowEngine extends Service {',`export interface ExternalWorkflowLifecycle {
  start(request: WorkflowStartRequest, execute: () => WorkflowRun): WorkflowRun
  event(name: string, args: unknown[], deliver: () => void): void
}
export abstract class WorkflowEngine extends Service {
  externalLifecycle: ExternalWorkflowLifecycle | undefined
  attachExternalLifecycle(port: ExternalWorkflowLifecycle): () => void {
    if (this.externalLifecycle) throw new Error('one external workflow authority only')
    this.externalLifecycle = port
    const original = this.start
    this.start = function(request) { return port.start(request, () => original.call(this, request)) }
    return () => { this.start = original; if (this.externalLifecycle === port) this.externalLifecycle = undefined }
  }
`);
 s=once(s,'    for (const callback of this.ctx.events.dispatch',`    const deliver = (): void => this.deliverWorkflowEvent(name, ...args)
    if (this.externalLifecycle) this.externalLifecycle.event(name, args, deliver)
    else deliver()
  }
  private deliverWorkflowEvent(name: WorkflowEventName, ...args: unknown[]): void {
    for (const callback of this.ctx.events.dispatch`);files['workflow/workflow/index.ts']=s;
 return files;
}
export async function prepareJobsRuntime(dshRoot,outputRoot,{productionPatch=process.env.SIC_JOBS_PRODUCTION_PATCH??fileURLToPath(new URL('./jobs-external-lifecycle.patch',import.meta.url))}={}){
 const packages=['jobs/jobs','jobs/jobs-local','jobs/tool-jobs','shell/tool-bash','shell/tool-pwsh','api/job-controller','subagent/subagent','subagent/tool-subagent','workflow/workflow','workflow/workflow-ptc','workflow/tool-workflow'];
 const files={},original={},manifest=[];
 async function collect(pkg,dir,rel=''){for(const e of await readdir(dir,{withFileTypes:true})){if(e.isDirectory())await collect(pkg,join(dir,e.name),rel+e.name+'/');else if(e.name.endsWith('.ts')){const p=join(dir,e.name),raw=await readFile(p,'utf8'),key=pkg+'/'+rel+e.name;files[key]=original[key]=raw.replaceAll('\r\n','\n');manifest.push({path:p,sha256:createHash('sha256').update(raw).digest('hex')});}}}
 for(const pkg of packages)await collect(pkg,join(dshRoot,'packages',pkg,'src'));
 patchJobsSources(files);await mkdir(outputRoot,{recursive:true});
 const staged=await mkdtemp(join(outputRoot,'shipped-patch-')),patch=resolve(productionPatch);
 const sourcePath=key=>{const[group,pkg,...path]=key.split('/');return join(staged,'packages',group,pkg,'src',...path);};
 for(const[key,text]of Object.entries(original)){const path=sourcePath(key);await mkdir(dirname(path),{recursive:true});await writeFile(path,text);}
 await promisify(execFile)('git',['-c','core.autocrlf=false','apply',patch],{cwd:staged,windowsHide:true});
 for(const[key,generated]of Object.entries(files)){
  const shipped=await readFile(sourcePath(key));
  if(!shipped.equals(Buffer.from(generated)))throw Error('shipped Jobs patch differs from tested native source: '+key);
  files[key]=shipped.toString('utf8');
 }
 const ts=createRequire(join(dshRoot,'package.json'))('typescript');
 const replacements={'@deepseek-ai/dsh-jobs':'jobs/jobs/index.mjs','@deepseek-ai/dsh-subagent':'subagent/subagent/index.mjs','@deepseek-ai/dsh-workflow':'workflow/workflow/index.mjs'};
 for(const[key,text]of Object.entries(files)){
  const pkg=key.split('/').slice(0,2).join('/'),require=createRequire(join(dshRoot,'packages',pkg,'lib/index.js'));
  let js=ts.transpileModule(text,{compilerOptions:{target:ts.ScriptTarget.ES2023,module:ts.ModuleKind.ESNext,experimentalDecorators:false}}).outputText;
  js=js.replace(/import \{ FiberState \} from ['"]@deepseek-ai\/cordis['"];?\n/,'').replaceAll('FiberState.ACTIVE','2');
  js=js.replace(/from (['"])([^'"\n]+)\1/g,(all,q,spec)=>spec.startsWith('node:')?all:spec.startsWith('.')?`from '${spec.replace(/\.ts$/,'.mjs')}'`:`from ${JSON.stringify(pathToFileURL(replacements[spec]?join(outputRoot,replacements[spec]):require.resolve(spec)).href)}`);
  const out=join(outputRoot,key.replace(/\.ts$/,'.mjs'));await mkdir(dirname(out),{recursive:true});await writeFile(out,js);await writeFile(join(outputRoot,key),text);
 }
 await writeFile(join(outputRoot,'manifest.json'),JSON.stringify({sources:manifest,patch:{path:patch,sha256:createHash('sha256').update(await readFile(patch)).digest('hex')}},null,2));
 return{files,original,root:outputRoot};
}
