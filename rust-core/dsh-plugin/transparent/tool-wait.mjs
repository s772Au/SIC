import {digest} from './transport.mjs';
import {nativeToolResults} from './tool-linkage.mjs';

const intentType = 'sic/tool-wait';
const sourceType = 'sic/tool-wait-source';
const isModel = message => message?.role === 'assistant' && message.source?.kind === 'model';
const callsOf = message => message.content.filter(block => block.type === 'tool-call').map(block => block.id);

// The finalized native assistant event is a boundary candidate. The Host still
// decides whether and when each proposed tool actually executes.
export function installToolWait(ctx, {bindingFor, recordSession, sessionAPI, edit}) {
  const armed = new WeakMap();
  const last = new WeakMap();
  const active = new Set();
  const lifetime = new AbortController();

  function observeAssistant(session, event) {
    if (event.type !== 'assistant/message' || lifetime.signal.aborted) return;
    const agent = ctx.agents.get(session.id);
    const previous = agent && armed.get(agent);
    if (!previous || agent.session !== session) return;
    armed.delete(agent);
    const {message, interrupted} = event.data;
    const {options, boundary, binding} = previous;
    const step = ctx.sessionProjections.stateOf(session, 'turnBoundary');
    if (interrupted || !isModel(message) || options.signal?.aborted
      || step?.openTurnStartSeq !== boundary.openTurnStartSeq || step?.lastStepStartSeq !== boundary.lastStepStartSeq
      || message.source.provider !== options.provider || message.source.model !== options.model
      || !message.content.some(block => block.type === 'text' && typeof block.text === 'string' && block.text.trim())) return;
    const call_ids = callsOf(message);
    if (!call_ids.length || call_ids.some(id => typeof id !== 'string' || !id) || new Set(call_ids).size !== call_ids.length) return;

    const toolWait = {assistant_event_seq:event.seq, assistant_message_id:message.id, call_ids};
    const requestId = digest({kind:'sdk-tool-wait', session:session.id, step:boundary.lastStepStartSeq, ...toolWait});
    const identity = {request_id:requestId, chain_id:binding.chain_id, scope_id:binding.scope_id, ...toolWait};
    const signal = AbortSignal.any([lifetime.signal, ...(options.signal ? [options.signal] : [])]);
    const frozen = Object.freeze({...options, signal, messages:Object.freeze([...options.messages, message])});
    const entry = {identity, joined:false, ready:false};
    last.set(agent, entry);
    // Schedule outside the Session emitter. Recording or publishing can await
    // IO while the native tool scheduler starts and settles its call group.
    entry.task = Promise.resolve().then(async () => {
      const record = (type, data) => recordSession(session, type, {...identity, ...data}, {key:requestId});
      try {
        const current = bindingFor(session.id, agent);
        if (current?.chain_id !== binding.chain_id || current.scope_id !== binding.scope_id) return;
        signal.throwIfAborted();
        await record(intentType, {});
        const publication = await edit({
          agent, binding:current, options:frozen, boundary, requestId, toolWait,
          recordSource:source => record(sourceType, {source_id:source.source_id}),
        });
        if (!publication) throw Error('tool-wait edit completed without a publication');
        entry.ready = true;
        await record('sic/tool-wait-settled', {status:'published', publication_id:publication.publication_id,
          cb_id:publication.cb_id, source_id:publication.source_id});
      } catch (error) {
        const status = signal.aborted ? 'cancelled' : 'failed';
        try { await record('sic/tool-wait-settled', {status, code:error?.code ?? error?.name ?? 'error'}); }
        catch { ctx.logger?.warn('SIC tool-wait settlement could not be recorded'); }
      }
    }).finally(() => {
      entry.ready = true;
      active.delete(entry.task);
    });
    active.add(entry.task);
  }

  async function join(agent, signal) {
    const entry = last.get(agent);
    if (!entry || entry.joined) return;
    signal?.throwIfAborted();
    const started = performance.now();
    const blocked = !entry.ready;
    const cancelled = Promise.withResolvers();
    const abort = () => cancelled.reject(signal.reason);
    signal?.addEventListener('abort', abort, {once:true});
    try { await Promise.race([entry.task,cancelled.promise]); }
    finally { signal?.removeEventListener('abort', abort); }
    signal?.throwIfAborted();
    if (entry.joined) return;
    entry.joined = true;
    await recordSession(agent.session, 'sic/tool-wait-joined', {...entry.identity, blocked,
      wait_ms:blocked ? performance.now() - started : 0}, {key:entry.identity.request_id});
  }

  function continuation(agent, binding, options) {
    const assistant = options.messages.findLast(isModel);
    if (!assistant || !callsOf(assistant).length) return null;
    const events = agent.session.snapshotEvents();
    const intent = events.findLast(event => {
      const record = sessionAPI.pluginRecordOf(event);
      return record?.type === 'plugin:' + intentType && record.data?.assistant_message_id === assistant.id
        && record.data.chain_id === binding.chain_id && record.data.scope_id === binding.scope_id;
    });
    if (!intent) return null;
    const identity = sessionAPI.pluginRecordOf(intent).data;
    const staged = events.findLast(event => {
      const record = sessionAPI.pluginRecordOf(event);
      return record?.type === 'plugin:' + sourceType && record.data?.request_id === identity.request_id;
    });
    // Without this receipt the Editor could not have run. Use the ordinary
    // closed-boundary path, including with a Core that rejected the open scope.
    if (!staged) return null;
    const original = agent.session.eventAt(identity.assistant_event_seq);
    if (original?.type !== 'assistant/message' || original.data.interrupted || original.data.message.id !== assistant.id
      || JSON.stringify(callsOf(assistant)) !== JSON.stringify(identity.call_ids)) throw Error('original native tool-wait boundary changed');
    const unresolved = new Set(identity.call_ids);
    for (const {message_id,call} of nativeToolResults(options.messages).values()) {
      if (message_id === assistant.id) unresolved.delete(call.id);
    }
    if (unresolved.size) throw Error('Worker continuation requires the complete native tool-wait result group');
    return {...identity, source_id:sessionAPI.pluginRecordOf(staged).data.source_id};
  }

  ctx.on('session/event', observeAssistant);
  ctx.on('agent/pre-step', async ({agent, signal}, next) => {
    await join(agent, signal);
    return next();
  }, {prepend:true});
  ctx.effect(() => async () => {
    lifetime.abort();
    await Promise.allSettled(active);
  });
  return {arm:(agent, previous) => armed.set(agent, previous), clear:agent => armed.delete(agent), join, continuation};
}
