/** SIC transport integration; pi-ai owns native conversion and replay. */
import { randomUUID } from 'node:crypto';
import { createRequire } from 'node:module';
import { pathToFileURL } from 'node:url';
import z from '@deepseek-ai/schemastery';
import { PiAiAdapter } from '@deepseek-ai/dsh-llm-pi-ai';
import { resolveRetryPolicy } from '@deepseek-ai/dsh-llm';
import { openAICompletionsApi } from '@earendil-works/pi-ai/api/openai-completions.lazy';
import {peerHost} from './peer-host.mjs';

// Session helpers must use the live host's class identity, including private fields.
const hostRequire = createRequire(pathToFileURL(process.argv[1]));
const { appendPluginRecord, pluginRecordOf } = await import(
  pathToFileURL(hostRequire.resolve('@deepseek-ai/dsh-session')).href);

export const name = 'sic-core-dsh';
export const inject = ['llm', 'agents'];
export const Config = z.object({
  provider: z.string().default('sic-core'),
  baseURL: z.string().default('http://127.0.0.1:18890/v1'),
  apiKeyEnv: z.string().default('SIC_CORE_TOKEN'),
  model: z.string().default('sic-worker'),
  contextWindow: z.number().step(1).min(1).default(128000),
  maxTokens: z.number().step(1).min(1).default(4096),
  inputPricePerMillion: z.number().min(0).required(),
  outputPricePerMillion: z.number().min(0).required(),
  cacheReadPricePerMillion: z.number().min(0).required(),
  cacheWritePricePerMillion: z.number().min(0).required(),
  timeoutMs: z.number().min(1).default(120000),
  toolEvents: z.boolean().default(true),
  fixtureWaitTool: z.boolean().default(false),
  fixtureWaitMs: z.number().step(1).min(1).default(400),
  fixtureWaitFailure: z.boolean().default(false),
  peerTools: z.array(z.string()).default([]),
  peerConcurrency: z.number().step(1).min(1).max(4).default(2),
  peerMaxDepth: z.number().step(1).min(1).default(4),
  fixturePeerTool: z.boolean().default(false),
  fixturePeerCount: z.number().step(1).min(1).max(4).default(1),
  fixturePeerReturnLifecycle: z.string().default('V'),
  fixtureCorePort: z.number().step(1).min(1).max(65535).default(18888),
});

/** Registers one dedicated SIC route and scoped lifecycle observation. */
export function apply(ctx, cfg) {
  const url = new URL(cfg.baseURL);
  if (url.protocol !== 'http:' || !['127.0.0.1', 'localhost', '[::1]'].includes(url.hostname)
      || url.username || url.password || url.search || url.hash || url.pathname.replace(/\/$/, '') !== '/v1') {
    throw new Error('SIC baseURL must be an authenticated local bridge /v1 origin');
  }
  const origin = url.origin;
  const key = () => {
    const value = process.env[cfg.apiKeyEnv];
    if (!value) throw new Error('SIC bridge credential environment reference is not configured');
    return value;
  };
  const request = async (path, body) => {
    const response = await fetch(origin + path, {
      method: body === undefined ? 'GET' : 'POST',
      headers: { Authorization: 'Bearer ' + key(), 'Content-Type': 'application/json' },
      ...(body === undefined ? {} : { body: JSON.stringify(body) }),
      signal: AbortSignal.timeout(cfg.timeoutMs),
    });
    if (!response.ok) {
      const error = new Error('SIC bridge operation failed with HTTP ' + response.status);
      error.httpStatus = response.status;
      try {const body=await response.json();const code=body.error?.code??body.code;if(typeof code==='string')error.code=code;}catch{}
      throw error;
    }
    return response.json();
  };
  const verifyBridge = async () => {
    const facts = await request('/api/v2/bridge/capabilities');
    if (facts.implementation !== 'sic-core-v2-fullhistory' || facts.public_model !== cfg.model) {
      throw new Error('The configured endpoint is not the expected SIC bridge/model');
    }
  };
  const protocol = openAICompletionsApi();
  const withSession = (options) => {
    if (!options?.sessionId) throw new Error('A real DSH SessionId is required for SIC');
    return { ...options, headers: { ...options.headers, 'x-sic-bridge-session': String(options.sessionId) } };
  };
  const model = {
    id: cfg.model, name: 'SIC Worker', provider: cfg.provider, api: 'openai-completions',
    baseUrl: cfg.baseURL, reasoning: false, input: ['text'],
    contextWindow: cfg.contextWindow, maxTokens: cfg.maxTokens,
    cost: { input: cfg.inputPricePerMillion, output: cfg.outputPricePerMillion,
      cacheRead: cfg.cacheReadPricePerMillion, cacheWrite: cfg.cacheWritePricePerMillion },
    compat: { supportsStore: false, supportsDeveloperRole: false, supportsUsageInStreaming: true,
      maxTokensField: 'max_tokens' },
  };
  const provider = {
    id: cfg.provider, name: 'SIC', baseUrl: cfg.baseURL,
    auth: { apiKey: { name: 'SIC host environment', resolve: async ({ credential }) => ({
      auth: { apiKey: credential?.key }, source: 'SIC host environment' }) } },
    getModels: () => [model],
    stream: (m, context, options) => protocol.stream(m, context, withSession(options)),
    streamSimple: (m, context, options) => protocol.streamSimple(m, context, withSession(options)),
  };
  const profile = {
    provider: cfg.provider, displayName: 'SIC', api: 'openai-completions', baseURL: cfg.baseURL,
    piProvider: provider, modelErrors: new Map(), configuredMaxTokens: new Map([[cfg.model, cfg.maxTokens]]),
    retryPolicy: resolveRetryPolicy({ mode: 'normal', maxRetries: 0 }, 'SIC'),
    streamIdleTimeoutMs: cfg.timeoutMs, timeoutMs: cfg.timeoutMs,
    maxRequestImageBytes: 1, requestImagePixelBudget: 1, requestImageMaxBytes: 1,
  };
  const profiles = new Map([[cfg.provider, profile]]);
  // This route has no interactive sign-in or ambient provider discovery.
  const auth = {
    credentials: {
      read: async () => undefined, list: async () => [],
      modify: async () => { throw new Error('Configure SIC credentials through the named environment reference'); },
      delete: async () => { throw new Error('SIC has no stored login'); },
    },
    authContext: { env: async () => undefined, fileExists: async () => false },
  };
  const postEvent = async (session, event) => request('/api/v2/bridge/events', {
    session_key: String(session.id), event,
  });
  const flushToolEvents = async (session) => {
    const pending = new Map();
    for (const event of session.snapshotEvents()) {
      const record = pluginRecordOf(event);
      if (record?.type !== 'plugin:sic/tool-event') continue;
      const data = record.data;
      if (data?.schema !== 1 || typeof data.event?.event_id !== 'string') continue;
      if (data.phase === 'pending') pending.set(data.event.event_id, data.event);
      else if (data.phase === 'delivered') pending.delete(data.event.event_id);
    }
    for (const event of pending.values()) {
      await postEvent(session, event);
      appendPluginRecord(session, 'plugin:sic/tool-event', { schema: 1, phase: 'delivered', event });
    }
  };
  const boundSessions=new Set(), terminalDeliveries=new Map();
  // Observe an actual successful native turn, not a model finish/proposal.
  ctx.on('session/event',(session,event)=>{
    if(event.type!=='turn/end'||event.data.reason?.kind!=='completed'||!boundSessions.has(String(session.id)))return;
    const terminal={type:'work_end',event_id:randomUUID(),observed_by:'dsh-native-turn-end',host_time_ms:Date.now(),turn:event.data.turn};
    const sending=(async()=>{
      // Session observers run inside the native append guard. Wait until that
      // commit finishes before appending the durable plugin outbox record.
      await Promise.resolve();
      appendPluginRecord(session,'plugin:sic/tool-event',{schema:1,phase:'pending',event:terminal});
      try {await postEvent(session,terminal);appendPluginRecord(session,'plugin:sic/tool-event',{schema:1,phase:'delivered',event:terminal});}
      catch(error){appendPluginRecord(session,'plugin:sic/event-delivery-error',{schema:1,event_id:terminal.event_id,error_type:error?.name??'Error'});}
    })();
    terminalDeliveries.set(String(session.id),sending);
  });
  ctx.on('session/flush',async session=>{await terminalDeliveries.get(String(session.id));});
  ctx.effect(()=>async()=>{await Promise.allSettled(terminalDeliveries.values());});
  let peerRuntime;
  const ensurePeerHost=async parent=>{
    if(!cfg.peerTools.length)return;
    if(!peerRuntime){
      const delegation=await import(pathToFileURL(hostRequire.resolve('@deepseek-ai/dsh-subagent')).href);
      peerRuntime=peerHost(ctx,cfg,{request,log:appendPluginRecord,recordOf:pluginRecordOf,delegation});
    }
    await peerRuntime.ensure(parent);
  };

  class LoggedSicAdapter extends PiAiAdapter {
    async prepareCall(providerId, modelId, signal) {
      const prepared = await super.prepareCall(providerId, modelId, signal);
      return { ...prepared, stream: options => this.stream(options) };
    }
    async *stream(options) {
      if (!options.sessionId) throw new Error('DSH did not supply the SIC session identity');
      const agent = ctx.agents.get(options.sessionId);
      if (!agent) throw new Error('SIC model dispatch requires a live Harness Agent');
      await verifyBridge();
      boundSessions.add(String(agent.id));
      await ensurePeerHost(agent);
      await flushToolEvents(agent.session);
      const chunks = [];
      for await (const chunk of super.stream(options)) chunks.push(chunk);
      // A native SDK failure has no delivered projection. Preserve that failure
      // instead of replacing it with an evidence-endpoint "not ready" error.
      if (chunks.some(chunk => chunk.type === 'finish' && ['error', 'aborted'].includes(chunk.reason?.kind))) {
        for (const chunk of chunks) yield chunk;
        return;
      }
      const evidence = await request('/api/v2/bridge/evidence', { session_key: String(options.sessionId) });
      appendPluginRecord(agent.session, 'plugin:sic/provider-projection', { schema: 1, ...evidence });
      // The bridge buffers its response. Log its exact model view before exposing output.
      for (const chunk of chunks) yield chunk;
    }
  }
  const adapter = new LoggedSicAdapter({ profiles: () => profiles, resolveApiKey: async () => key(), auth });
  ctx.effect(() => ctx.llm.registerAdapter([cfg.provider], adapter));

  if (cfg.toolEvents) ctx.on('tools/execute', async (exec, next) => {
    const session = exec.agent?.session;
    if (!session || exec.parent || session.requestContext()?.provider !== cfg.provider) return next();
    const start = { type: 'tool_start', call_id: String(exec.callId), event_id: randomUUID(),
      observed_by: 'dsh-around-dispatch', host_time_ms: Date.now() };
    await postEvent(session, start);
    appendPluginRecord(session, 'plugin:sic/tool-event', { schema: 1, phase: 'delivered', event: start });
    let outcome = 'failed';
    try {
      const result = await next();
      outcome = result.isError ? 'failed' : 'completed';
      return result;
    } finally {
      const end = { type: 'tool_end', call_id: String(exec.callId), event_id: randomUUID(),
        observed_by: 'dsh-around-dispatch', outcome, host_time_ms: Date.now() };
      appendPluginRecord(session, 'plugin:sic/tool-event', { schema: 1, phase: 'pending', event: end });
      try {
        await postEvent(session, end);
        appendPluginRecord(session, 'plugin:sic/tool-event', { schema: 1, phase: 'delivered', event: end });
      } catch (error) {
        // Preserve the actual tool outcome; the next dispatch must flush the durable outbox.
        appendPluginRecord(session, 'plugin:sic/event-delivery-error', {
          schema: 1, event_id: end.event_id, error_type: error?.name ?? 'Error',
        });
      }
    }
  });

  if (cfg.fixtureWaitTool) ctx.inject(['tools'], (child) => {
    child.effect(() => child.tools.register({
      name: 'sic_fixture_wait', description: 'Wait briefly and return a synthetic integration marker.',
      parameters: {}, output: { schema: { type: 'string' },
        render: (_args, value) => [{ type: 'text', text: value }] },
      async execute(_args, exec) {
        appendPluginRecord(exec.agent.session, 'plugin:sic/fixture-body', { phase: 'started', time_ms: Date.now() });
        try {
          exec.signal.throwIfAborted();
          await new Promise((resolve,reject)=>{
            const abort=()=>{clearTimeout(timer);exec.signal.removeEventListener('abort',abort);reject(exec.signal.reason??new Error('Fixture wait aborted'));};
            const timer=setTimeout(()=>{exec.signal.removeEventListener('abort',abort);resolve();},cfg.fixtureWaitMs);
            exec.signal.addEventListener('abort',abort,{once:true});
            if(exec.signal.aborted)abort();
          });
        }catch(error){
          appendPluginRecord(exec.agent.session,'plugin:sic/fixture-body',{phase:'aborted',time_ms:Date.now()});
          throw error;
        }
        appendPluginRecord(exec.agent.session, 'plugin:sic/fixture-body', { phase: 'ended', time_ms: Date.now() });
        if(cfg.fixtureWaitFailure)throw new Error('SIC_NATIVE_TOOL_FAILED');
        return 'SIC_SYNTHETIC_TOOL_RESULT';
      },
    }));
  });
  if(cfg.fixturePeerTool)ctx.inject(['tools'],child=>{
    child.effect(()=>child.tools.register({
      name:'sic_fixture_peer_roundtrip',description:'Run an isolated synthetic historical peer task through its native Harness tool host.',
      parameters:{},output:{schema:{type:'string'},render:(_args,value)=>[{type:'text',text:value}]},
      async execute(_args,exec){
        const evidence=await request('/api/v2/bridge/evidence',{session_key:String(exec.agent.id)});
        // The control plane has its own port; do not send it to the Chat origin.
        const control=async(operation,payload)=>{
          const controlURL=new URL(cfg.baseURL);controlURL.port=String(cfg.fixtureCorePort);controlURL.pathname='/api/v2/operations';
          const response=await fetch(controlURL,{method:'POST',headers:{Authorization:'Bearer '+key(),'Content-Type':'application/json'},body:JSON.stringify({operation,context:{chain_id:evidence.chain_id,actor:'host',allow_write:true,allow_model:false},payload}),signal:AbortSignal.timeout(cfg.timeoutMs)});
          if(!response.ok){
            let code='unavailable';
            try {const failure=await response.json();if(typeof failure.error?.code==='string')code=failure.error.code;}catch{}
            throw new Error('Synthetic peer control '+operation+' failed: HTTP '+response.status+' ('+code+')');
          }
          return (await response.json()).result;
        };
        const checkpoint=await control('chain.checkpoint',{});
        const sockets=[];
        for(let i=0;i<cfg.fixturePeerCount;i++)sockets.push(await control('socket.create',{checkpoint_id:checkpoint.checkpoint_id,spawn_acl:['host'],message_acl:['host'],signal_acl:['host']}));
        const socket=sockets[0];
        const requestId='dsh-peer-fixture:'+exec.callId;
        await control('collaboration.dispatch',{socket_ids:sockets.map(value=>value.socket_id),request_id:requestId,question:'SIC_PEER_WORK_TOOL_TEST: execute the approved wait tool and return its actual result.',lifecycle:'V',return_lifecycle:cfg.fixturePeerReturnLifecycle});
        appendPluginRecord(exec.agent.session,'plugin:sic/peer-fixture-request',{request_id:requestId,root_chain_id:evidence.chain_id,socket_id:socket.socket_id,socket_ids:sockets.map(value=>value.socket_id)});
        const deadline=Date.now()+cfg.timeoutMs;
        while(Date.now()<deadline){
          const state=await control('collaboration.status',{request_id:requestId});const result=state.requests[requestId];
          if(result.status==='completed'){appendPluginRecord(exec.agent.session,'plugin:sic/peer-fixture-return',{request_id:requestId,result});return 'SIC_PEER_TOOL_DONE '+JSON.stringify(result);}
          if(['failed','cancelled'].includes(result.status))throw new Error('Synthetic peer task did not complete');
          exec.signal.throwIfAborted();await new Promise(resolve=>setTimeout(resolve,100));
        }
        throw new Error('Synthetic peer task deadline; no automatic tool replay');
      },
    }));
  });
}
