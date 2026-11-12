// Trusted DSH-host event observation. Only bounded identity/outcome metadata
// crosses the loopback bridge; tool content, arguments and error reasons do not.
const MAX_SESSIONS = 64;
const MAX_PENDING = 256;
const BACKFILL_EVENTS = 512;
const MAX_PER_PUMP = 16;
const QUERY_WINDOW = 50;
const RECOVERY_TIMEOUT_MS = 2500;

const validId = value => typeof value === 'string' && value.trim() !== ''
  && Buffer.byteLength(value, 'utf8') <= 256 && !/[\u0000-\u001f\u007f-\u009f]/u.test(value);

export function nativeToolEvent(sessionId, event) {
  if (!validId(sessionId) || event?.type !== 'tool/result' || event.surfaceOp !== 'append'
      || !Number.isSafeInteger(event.seq) || event.seq < 0) return null;
  const msg = event?.data?.message;
  const callId = msg?.source?.callId;
  if (msg?.role !== 'tool' || msg?.source?.kind !== 'tool' || !validId(callId)
      || msg.toolCallId !== callId || (msg.isError !== undefined && typeof msg.isError !== 'boolean')) return null;
  const code = event?.data?.error?.code;
  if ((code === 'TOOL_OUTCOME_UNKNOWN' || code === 'TOOL_NOT_STARTED') && msg.isError !== true) return null;
  const outcome = code === 'TOOL_OUTCOME_UNKNOWN' ? 'unknown'
    : code === 'TOOL_NOT_STARTED' ? 'not_started'
      : msg.isError === true ? 'error' : 'ok';
  return { session_id: sessionId, call_id: callId, event_seq: event.seq, outcome };
}

export function createNativeEventForwarder({ baseURL, credential, fetchImpl = globalThis.fetch, getSession, readEventPage }) {
  const base = new URL(baseURL);
  if (base.protocol !== 'http:' || !['127.0.0.1', 'localhost', '[::1]'].includes(base.hostname)
      || base.username || base.password || base.search || base.hash
      || base.pathname.replace(/\/$/, '') !== '/v1') {
    throw new Error('SIC native event bridge must be a loopback /v1 endpoint');
  }
  const endpoint = new URL(`${base.pathname.replace(/\/$/, '')}/sic/native-tool-event`, base.origin);
  const sessions = new Map();
  let disposed = false;

  function stateFor(id) {
    let state = sessions.get(id);
    if (state) { sessions.delete(id); sessions.set(id, state); return state; }
    if (sessions.size >= MAX_SESSIONS) {
      const [oldId, old] = sessions.entries().next().value;
      if (old.pending.size) console.warn('SIC native event queue evicted; some outcomes remain unknown');
      if (old.timer) clearTimeout(old.timer);
      sessions.delete(oldId);
    }
    state = { active: false, session: null, pending: new Map(), acknowledged: new Set(), running: null, timer: null,
      overflowWarned: false, recoveryNext: null, recoveryFloor: null, recoveryDone: false, recovering: null, outstandingQuery: null };
    sessions.set(id, state);
    return state;
  }

  function retryLater(id, state) {
    if (disposed || state.timer || !state.pending.size) return;
    state.timer = setTimeout(() => { state.timer = null; void pump(id, state); }, 2000);
    state.timer.unref?.();
  }

  function queue(id, state, event) {
    const item = nativeToolEvent(id, event);
    if (!item || state.acknowledged.has(item.event_seq) || state.pending.has(item.event_seq)) return;
    if (state.pending.size >= MAX_PENDING) {
      // Keep recent results when recovery sees more events than the queue can
      // hold. Otherwise an old snapshot tail can displace the very tool result
      // the next Worker continuation is about to consume.
      const oldest = Math.min(...state.pending.keys());
      if (!state.overflowWarned) console.warn('SIC native event queue is full; older outcomes may remain unknown');
      state.overflowWarned = true;
      if (item.event_seq <= oldest) return;
      state.pending.delete(oldest);
    }
    state.pending.set(item.event_seq, item);
  }

  async function pump(id, state) {
    if (disposed || !state.active) return;
    if (state.running) return state.running;
    state.running = (async () => {
      let sent = 0;
      // Newest results are most likely to belong to the immediate tool
      // continuation. The persisted bridge still identifies every result by
      // call ID and event sequence; this ordering grants no execution rights.
      const pending = [...state.pending].sort((a, b) => b[0] - a[0]);
      for (const [seq, item] of pending) {
        if (state.pending.get(seq) !== item) continue;
        if (disposed || ++sent > MAX_PER_PUMP) break;
        let response;
        try {
          response = await fetchImpl(endpoint, {
            method: 'POST', redirect: 'error', signal: AbortSignal.timeout(1500),
            headers: { authorization: `Bearer ${credential()}`, 'content-type': 'application/json', 'cache-control': 'no-store' },
            body: JSON.stringify(item),
          });
        } catch { break; }
        if (response.ok) {
          let result;
          try { result = await response.json(); } catch { break; }
          if (result?.recorded !== true) break;
        } else if (response.status !== 404 && response.status !== 409) {
          try { await response.body?.cancel(); } catch {}
          break;
        } else {
          // Non-SIC historical calls have no task mapping. A conflicting event
          // must never be allowed to overwrite the already-recorded result.
          try { await response.body?.cancel(); } catch {}
        }
        state.pending.delete(seq);
        if (state.pending.size < MAX_PENDING) state.overflowWarned = false;
        state.acknowledged.add(seq);
        if (state.acknowledged.size > BACKFILL_EVENTS) state.acknowledged.delete(state.acknowledged.values().next().value);
      }
    })().finally(() => { state.running = null; retryLater(id, state); });
    return state.running;
  }

  function observe(session, event) {
    if (disposed || !validId(session?.id)) return;
    const state = stateFor(session.id);
    state.session = typeof WeakRef === 'function' ? new WeakRef(session) : null;
    if (!state.active) return;
    queue(session.id, state, event);
    void pump(session.id, state);
  }

  async function recover(id, state) {
    if (state.recovering) return state.recovering;
    if (state.recoveryDone || state.outstandingQuery || typeof readEventPage !== 'function') return;
    state.recovering = (async () => {
      try {
        const session = state.session?.deref() ?? getSession?.(id);
        if (!Number.isSafeInteger(session?.seq) || session.seq < 0) return;
        if (state.recoveryNext === null) {
          state.recoveryNext = session.seq - 1;
          state.recoveryFloor = Math.max(0, session.seq - BACKFILL_EVENTS);
        }
        if (state.recoveryNext < state.recoveryFloor) { state.recoveryDone = true; return; }
        const signal = AbortSignal.timeout(RECOVERY_TIMEOUT_MS);
        const timedOut = new Promise(resolve => signal.addEventListener('abort', () => resolve(null), { once: true }));
        while (!disposed && state.recoveryNext >= state.recoveryFloor && state.pending.size < MAX_PENDING) {
          const end = state.recoveryNext;
          const before = Math.min(QUERY_WINDOW, end - state.recoveryFloor, MAX_PENDING - state.pending.size - 1);
          const query = Promise.resolve().then(() => readEventPage(id, end, before, signal)).catch(() => null);
          state.outstandingQuery = query;
          void query.then(() => { if (state.outstandingQuery === query) state.outstandingQuery = null; });
          const page = await Promise.race([query, timedOut]);
          const start = end - before;
          if (!page || page.startSeq !== start || page.endSeq !== end || !Array.isArray(page.events)
              || page.events.length !== end - start + 1
              || page.events.some((event, index) => event?.seq !== start + index)) return;
          for (const event of page.events) queue(id, state, event);
          state.recoveryNext = start - 1;
        }
        if (state.recoveryNext < state.recoveryFloor) state.recoveryDone = true;
      } catch { /* Query unavailable: leave missing outcomes unknown. */ }
    })().finally(() => { state.recovering = null; });
    return state.recovering;
  }

  async function activateAndFlush(sessionId) {
    if (disposed || !validId(sessionId)) return;
    const state = stateFor(sessionId);
    state.active = true;
    // Read bounded historical windows through DSH's asynchronous query
    // service. Never depend on its deprecated synchronous event-log reads.
    await recover(sessionId, state);
    const alreadyRunning = Boolean(state.running);
    await pump(sessionId, state);
    // A pump already in progress may have selected its bounded batch before
    // this snapshot arrived. Give newly queued events their own bounded pass.
    if (alreadyRunning && state.pending.size) await pump(sessionId, state);
  }

  function dispose() {
    disposed = true;
    for (const state of sessions.values()) if (state.timer) clearTimeout(state.timer);
    sessions.clear();
  }
  return { observe, activateAndFlush, dispose };
}
