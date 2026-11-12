import z from '@deepseek-ai/schemastery';
import { PiAiAdapter } from '@deepseek-ai/dsh-llm-pi-ai';
import { resolveRetryPolicy } from '@deepseek-ai/dsh-llm';
import { openAICompletionsApi } from '@earendil-works/pi-ai/api/openai-completions.lazy';
import { lazyStream } from '@earendil-works/pi-ai/api/lazy';
import { createNativeEventForwarder } from './native-events.mjs';
import { mountSicOverview } from './view-host.mjs';

export const name = 'sic-core-rust-dsh';
export const inject = ['llm', 'agents'];
const CAPACITY = 1_000_000;
const MAX_OUTPUT = 393_216;

export const Config = z.object({
  provider: z.string().default('sic-core-rust'),
  baseURL: z.string().default('http://127.0.0.1:18890/v1'),
  apiKeyEnv: z.string().default('SIC_CORE_TOKEN'),
  model: z.string().default('sic-worker'),
  // When set, bind one already-owned idle Rust Agent to the first DSH Session
  // that uses this provider. The bridge enforces the durable one-to-one rule.
  bindChainId: z.string().default(''),
  bindTimeoutMs: z.number().step(1).min(1).max(60000).default(10000),
  // DSH advertises the supported ceilings; this is the actual fixed request budget.
  maxTokens: z.number().step(1).min(1).max(MAX_OUTPUT).default(4096),
  inputPricePerMillion: z.number().min(0).required(),
  outputPricePerMillion: z.number().min(0).required(),
  cacheReadPricePerMillion: z.number().min(0).required(),
  cacheWritePricePerMillion: z.number().min(0).required(),
  timeoutMs: z.number().step(1).min(1).default(120000),
  // Optional DSH-side allowlist. When present, unlisted native tools are not
  // described to SIC and cannot be returned as host tasks for this route.
  allowedTools: z.array(z.string()).default([]),
  fixtureReadTool: z.boolean().default(false),
});

function localV1Url(value) {
  const url = new URL(value);
  if (url.protocol !== 'http:' || !['127.0.0.1', 'localhost', '[::1]'].includes(url.hostname)
      || url.username || url.password || url.search || url.hash
      || url.pathname.replace(/\/$/, '') !== '/v1') {
    throw new Error('SIC Rust baseURL must be a loopback /v1 endpoint');
  }
  return url;
}

export function apply(ctx, cfg) {
  // Validate the endpoint before installing the event observer.
  localV1Url(cfg.baseURL);
  const nativeEvents = createNativeEventForwarder({ baseURL: cfg.baseURL, credential: () => {
    const token = process.env[cfg.apiKeyEnv];
    if (!token) throw new Error('SIC Rust bearer environment reference is not configured');
    return token;
  }, getSession: id => ctx.get('sessions')?.get(id),
  readEventPage: (sessionId, seq, before, signal) => {
    const query = ctx.get('sessionQuery');
    if (typeof query?.readEvent !== 'function') return null;
    return query.readEvent({ sessionId, seq, before, after: 0 }, signal);
  } });
  ctx.on('session/event', (session, event) => nativeEvents.observe(session, event));
  ctx.effect(() => () => nativeEvents.dispose());
  const provider = createProvider(cfg, { nativeEvents });
  const model = provider.model;
  const profiles = provider.profiles;
  const credential = provider.credential;
  const auth = provider.auth;
  mountSicOverview(ctx, cfg, credential);
  class NoRetryAdapter extends PiAiAdapter {
    async prepareCall(providerId, modelId, signal) {
      const prepared = await super.prepareCall(providerId, modelId, signal);
      return { ...prepared, stream: options => this.stream(options) };
    }
  }
  const adapter = new NoRetryAdapter({ profiles: () => profiles, resolveApiKey: async () => credential(), auth });
  ctx.effect(() => ctx.llm.registerAdapter([cfg.provider], adapter));

  if (cfg.fixtureReadTool) {
    ctx.inject(['tools'], child => child.effect(() => child.tools.register({
      name: 'sic_fixture_read',
      description: 'Return a fixed local integration marker. This fixture reads no files and performs no external action.',
      parameters: { type: 'object', properties: {}, additionalProperties: false },
      output: { schema: { type: 'string' }, render: (_args, value) => [{ type: 'text', text: value }] },
      async execute() { return 'SIC_RUST_SYNTHETIC_FIXTURE_ONLY'; },
    })));
  }
}

export function createProvider(cfg, { protocol = openAICompletionsApi(), fetchImpl = globalThis.fetch, nativeEvents } = {}) {
  const url = localV1Url(cfg.baseURL);
  const credential = () => {
    const value = process.env[cfg.apiKeyEnv];
    if (!value) throw new Error('SIC Rust bearer environment reference is not configured');
    return value;
  };
  const bindChainId = typeof cfg.bindChainId === 'string' ? cfg.bindChainId.trim() : '';
  if (bindChainId.length > 256) throw new Error('SIC bindChainId exceeds the supported identity limit');
  const bindUrl = new URL(`${url.pathname.replace(/\/$/, '')}/sic/bind`, url.origin);
  const bindPromises = new Map();
  let chainSessionId;

  const withSession = options => {
    const sessionId = options?.sessionId;
    if (typeof sessionId !== 'string' || !sessionId.trim()) {
      throw new Error('DSH did not supply a session identity for SIC Rust');
    }
    return {
      ...options,
      headers: { ...options.headers, 'x-sic-bridge-session': sessionId },
    };
  };

  const bindSession = async options => {
    if (!bindChainId) return;
    const sessionId = options.sessionId;
    if (chainSessionId !== undefined && chainSessionId !== sessionId) {
      throw new Error('SIC bindChainId is already assigned to a different DSH session in this process');
    }
    chainSessionId ??= sessionId;
    const existing = bindPromises.get(sessionId);
    if (existing) return existing;

    const pending = (async () => {
      const timeoutSignal = AbortSignal.timeout(Math.min(cfg.bindTimeoutMs ?? 10000, cfg.timeoutMs ?? 120000));
      const signal = options.signal ? AbortSignal.any([options.signal, timeoutSignal]) : timeoutSignal;
      let response;
      try {
        response = await fetchImpl(bindUrl, {
          method: 'POST',
          headers: {
            authorization: `Bearer ${credential()}`,
            'content-type': 'application/json',
            'cache-control': 'no-store',
          },
          body: JSON.stringify({ session_id: sessionId, chain_id: bindChainId }),
          signal,
          redirect: 'error',
        });
      } catch {
        if (options.signal?.aborted) throw new Error('SIC session binding was cancelled');
        if (timeoutSignal.aborted) throw new Error('SIC session binding timed out');
        throw new Error('SIC session binding transport failed');
      }
      if (!response.ok) {
        try { await response.body?.cancel(); } catch {}
        throw new Error(`SIC session binding was rejected (HTTP ${response.status})`);
      }
      let result;
      try { result = await response.json(); }
      catch { throw new Error('SIC session binding returned an invalid acknowledgement'); }
      if (result?.session_id !== sessionId || result?.chain_id !== bindChainId) {
        throw new Error('SIC session binding acknowledgement did not match the requested identity');
      }
    })();
    bindPromises.set(sessionId, pending);
    pending.catch(() => {
      if (bindPromises.get(sessionId) === pending) bindPromises.delete(sessionId);
    });
    return pending;
  };

  const streamWithBinding = (simple, selected, context, options) => {
    let sessionOptions;
    try { sessionOptions = withSession(options); }
    catch (error) {
      // Keep the PiAi provider's synchronous stream contract for setup errors.
      return lazyStream(selected, async () => { throw error; });
    }
    return lazyStream(selected, async () => {
      await bindSession(sessionOptions);
      // Give committed native result events a chance to reach SIC before its
      // tool-result continuation. Failure remains an unknown outcome.
      await nativeEvents?.activateAndFlush(sessionOptions.sessionId);
      if (sessionOptions.signal?.aborted) {
        throw new Error('SIC session binding was cancelled');
      }
      const filtered = filterContext(context);
      return simple
        ? protocol.streamSimple(selected, filtered, sessionOptions)
        : protocol.stream(selected, filtered, sessionOptions);
    });
  };

  const filterContext = context => {
    if (!cfg.allowedTools.length || !context || typeof context !== 'object') return context;
    const allowed = new Set(cfg.allowedTools);
    const filtered = { ...context };
    if (Array.isArray(context.tools)) filtered.tools = context.tools.filter(tool => allowed.has(tool?.name));
    if (Array.isArray(context.messages)) {
      filtered.messages = context.messages.map(message => {
        if (!message || typeof message !== 'object') return message;
        const next = { ...message };
        if (Array.isArray(message.toolsAdded)) next.toolsAdded = message.toolsAdded.filter(tool => allowed.has(tool?.name));
        if (Array.isArray(message.toolsRemoved)) next.toolsRemoved = message.toolsRemoved.filter(tool => allowed.has(tool?.name));
        return next;
      });
    }
    return filtered;
  };
  const model = {
    id: cfg.model,
    name: 'SIC Rust Worker',
    provider: cfg.provider,
    api: 'openai-completions',
    baseUrl: cfg.baseURL,
    reasoning: false,
    input: ['text'],
    contextWindow: CAPACITY,
    maxTokens: MAX_OUTPUT,
    cost: {
      input: cfg.inputPricePerMillion,
      output: cfg.outputPricePerMillion,
      cacheRead: cfg.cacheReadPricePerMillion,
      cacheWrite: cfg.cacheWritePricePerMillion,
    },
    compat: {
      supportsStore: false,
      supportsDeveloperRole: false,
      supportsUsageInStreaming: true,
      maxTokensField: 'max_tokens',
    },
  };
  const piProvider = {
    id: cfg.provider,
    name: 'SIC Rust',
    baseUrl: cfg.baseURL,
    auth: {
      apiKey: {
        name: 'SIC Rust host environment',
        resolve: async ({ credential: supplied }) => ({
          auth: { apiKey: supplied?.key },
          source: 'SIC Rust host environment',
        }),
      },
    },
    getModels: () => [model],
    stream: (selected, context, options) => streamWithBinding(false, selected, context, options),
    streamSimple: (selected, context, options) => streamWithBinding(true, selected, context, options),
  };
  const profiles = new Map([[cfg.provider, {
    provider: cfg.provider,
    displayName: 'SIC Rust',
    api: 'openai-completions',
    baseURL: cfg.baseURL,
    piProvider,
    modelErrors: new Map(),
    // This controls actual calls; model.maxTokens remains the documented ceiling.
    configuredMaxTokens: new Map([[cfg.model, cfg.maxTokens]]),
    retryPolicy: resolveRetryPolicy({ mode: 'normal', maxRetries: 0 }, 'SIC Rust'),
    streamIdleTimeoutMs: cfg.timeoutMs,
    timeoutMs: cfg.timeoutMs,
    maxRequestImageBytes: 1,
    requestImagePixelBudget: 1,
    requestImageMaxBytes: 1,
  }]]);
  const auth = {
    credentials: {
      read: async () => undefined,
      list: async () => [],
      modify: async () => { throw new Error('Set the named SIC Rust environment variable; credentials are not stored'); },
      delete: async () => { throw new Error('SIC Rust does not store login credentials'); },
    },
    authContext: { env: async () => undefined, fileExists: async () => false },
  };
  return { model, provider: piProvider, profiles, credential, auth };
}
