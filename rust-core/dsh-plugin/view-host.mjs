// DSH connection route: trusted host keeps the SIC bearer, browser gets only
// explicitly selected per-session data from the loopback bridge.
export function createOverviewHandler({ baseURL, credential, fetchImpl = globalThis.fetch }) {
  const base = new URL(baseURL);
  if (base.protocol !== 'http:' || !['127.0.0.1', 'localhost', '[::1]'].includes(base.hostname)
      || base.username || base.password || base.search || base.hash
      || base.pathname.replace(/\/$/, '') !== '/v1') throw new Error('SIC overview requires a loopback /v1 endpoint');
  const reply = (value, status = 200) => Response.json(value, { status, headers: { 'cache-control': 'no-store' } });
  return async request => {
    let body;
    try {
      const text = await request.text();
      if (Buffer.byteLength(text, 'utf8') > 1024) return reply({ ok: false, error: 'invalid_request' }, 413);
      body = JSON.parse(text);
    } catch { return reply({ ok: false, error: 'invalid_request' }, 400); }
    const sessionId = body?.sessionId;
    if (!body || typeof body !== 'object' || Array.isArray(body) || Object.keys(body).length !== 1
        || typeof sessionId !== 'string' || !sessionId.trim() || Buffer.byteLength(sessionId, 'utf8') > 256
        || /[\u0000-\u001f\u007f-\u009f]/u.test(sessionId)) return reply({ ok: false, error: 'invalid_request' }, 400);
    let token;
    try { token = credential(); } catch { return reply({ ok: false, error: 'host_not_configured' }, 503); }
    const read = async part => {
      const url = new URL(`${base.pathname.replace(/\/$/, '')}/sic/${part}`, base.origin);
      try {
        const response = await fetchImpl(url, {
          method: 'POST', redirect: 'error', signal: AbortSignal.timeout(4000),
          headers: { authorization: `Bearer ${token}`, 'content-type': 'application/json', 'cache-control': 'no-store' },
          body: JSON.stringify({ session_id: sessionId }),
        });
        if (!response.ok) { try { await response.body?.cancel(); } catch {} return { error: response.status === 404 ? 'session_not_found' : 'unavailable' }; }
        const value = await response.json();
        if (!value || typeof value !== 'object' || Array.isArray(value)) return { error: 'unavailable' };
        return { value };
      } catch { return { error: 'unavailable' }; }
    };
    const [status, accounting, events] = await Promise.all(['status', 'accounting', 'tool-events'].map(read));
    return reply({ ok: true, value: { status, accounting, events } });
  };
}

export function mountSicOverview(ctx, cfg, credential) {
  ctx.inject(['connection'], child => {
    const connection = child.get('connection');
    if (typeof connection?.fetch?.register !== 'function') return;
    const handler = createOverviewHandler({ baseURL: cfg.baseURL, credential });
    return child.effect(() => connection.fetch.register({
      path: '/api/sic-core/overview', methods: ['POST'], requestBody: 'buffered', fetch: handler,
    }), 'sic-core: session overview');
  });
  ctx.inject(['connection'], child => {
    const connection = child.get('connection');
    if (typeof connection?.fetch?.register !== 'function') return;
    const handler = createContextHandler({ baseURL: cfg.baseURL, credential });
    return child.effect(() => connection.fetch.register({
      path: '/api/sic-core/context', methods: ['POST'], requestBody: 'buffered', fetch: handler,
    }), 'sic-core: session context');
  });
  ctx.inject(['connection'], child => {
    const connection = child.get('connection');
    if (typeof connection?.fetch?.register !== 'function') return;
    const handler = createDecisionHandler({ baseURL: cfg.baseURL, credential });
    return child.effect(() => connection.fetch.register({
      path: '/api/sic-core/decision-event', methods: ['POST'], requestBody: 'buffered', fetch: handler,
    }), 'sic-core: decision edit');
  });
}

export function createContextHandler({ baseURL, credential, fetchImpl = globalThis.fetch }) {
  const base = new URL(baseURL);
  if (base.protocol !== 'http:' || !['127.0.0.1', 'localhost', '[::1]'].includes(base.hostname)
      || base.username || base.password || base.search || base.hash || base.pathname.replace(/\/$/, '') !== '/v1') {
    throw new Error('SIC context requires a loopback /v1 endpoint');
  }
  const reply = (value, status = 200) => Response.json(value, { status, headers: { 'cache-control': 'no-store' } });
  return async request => {
    let body;
    try {
      const text = await request.text();
      if (Buffer.byteLength(text, 'utf8') > 4096) return reply({ ok: false, error: 'invalid_request' }, 413);
      body = JSON.parse(text);
    } catch { return reply({ ok: false, error: 'invalid_request' }, 400); }
    if (!body || typeof body !== 'object' || Array.isArray(body) || Object.keys(body).length !== 3
        || typeof body.sessionId !== 'string' || !body.sessionId.trim() || Buffer.byteLength(body.sessionId, 'utf8') > 256
        || /[\u0000-\u001f\u007f-\u009f]/u.test(body.sessionId)
        || !['timeline', 'expand', 'page', 'decision', 'candidate'].includes(body.action)
        || !body.params || typeof body.params !== 'object' || Array.isArray(body.params)) return reply({ ok: false, error: 'invalid_request' }, 400);
    try {
      const url = new URL(`${base.pathname.replace(/\/$/, '')}/sic/context`, base.origin);
      const response = await fetchImpl(url, { method: 'POST', redirect: 'error', signal: AbortSignal.timeout(5000),
        headers: { authorization: `Bearer ${credential()}`, 'content-type': 'application/json', 'cache-control': 'no-store' },
        body: JSON.stringify({ session_id: body.sessionId, action: body.action, params: body.params }) });
      if (!response.ok) { try { await response.body?.cancel(); } catch {} return reply({ ok: false, error: 'context_unavailable' }, response.status); }
      const value = await response.json();
      return reply({ ok: true, value });
    } catch { return reply({ ok: false, error: 'context_unavailable' }, 503); }
  };
}

export function createDecisionHandler({ baseURL, credential, fetchImpl = globalThis.fetch }) {
  const base = new URL(baseURL);
  if (base.protocol !== 'http:' || !['127.0.0.1', 'localhost', '[::1]'].includes(base.hostname)
      || base.username || base.password || base.search || base.hash || base.pathname.replace(/\/$/, '') !== '/v1') {
    throw new Error('SIC decision route requires a loopback /v1 endpoint');
  }
  const reply = (value, status = 200) => Response.json(value, { status, headers: { 'cache-control': 'no-store' } });
  return async request => {
    // Writes require a same-origin browser request in addition to the DSH
    // connection's own access control; cross-site forms are not sufficient.
    let originMatchesHost = false;
    try {
      // DSH reconstructs request.url with an internal origin. The original
      // browser Host and Origin headers survive that handoff.
      const originText = request.headers.get('origin');
      const origin = new URL(originText);
      const host = request.headers.get('host');
      if (host && !/[\s/@\\?#]/u.test(host) && ['http:', 'https:'].includes(origin.protocol)
          && origin.origin === originText) {
        originMatchesHost = new URL(`${origin.protocol}//${host}`).host === origin.host;
      }
    } catch {}
    if (!originMatchesHost || request.headers.get('sec-fetch-site') !== 'same-origin'
        || !/^application\/json(?:;|$)/i.test(request.headers.get('content-type') || '')) {
      return reply({ ok: false, error: 'origin_rejected' }, 403);
    }
    let body;
    try {
      const text = await request.text();
      if (Buffer.byteLength(text, 'utf8') > 8192) return reply({ ok: false, error: 'invalid_request' }, 413);
      body = JSON.parse(text);
    } catch { return reply({ ok: false, error: 'invalid_request' }, 400); }
    if (!body || typeof body !== 'object' || Array.isArray(body) || Object.keys(body).length !== 5
        || typeof body.sessionId !== 'string' || !body.sessionId.trim() || Buffer.byteLength(body.sessionId, 'utf8') > 256
        || /[\u0000-\u001f\u007f-\u009f]/u.test(body.sessionId)
        || !Number.isSafeInteger(body.expectedRevision) || body.expectedRevision < 0
        || typeof body.eventId !== 'string' || !body.eventId.trim() || Buffer.byteLength(body.eventId, 'utf8') > 256
        || !['work', 'candidate', 'overlooked'].includes(body.kind)
        || typeof body.text !== 'string' || !body.text.trim() || Buffer.byteLength(body.text, 'utf8') > 4096) {
      return reply({ ok: false, error: 'invalid_request' }, 400);
    }
    try {
      const url = new URL(`${base.pathname.replace(/\/$/, '')}/sic/decision-event`, base.origin);
      const response = await fetchImpl(url, { method: 'POST', redirect: 'error', signal: AbortSignal.timeout(5000),
        headers: { authorization: `Bearer ${credential()}`, 'content-type': 'application/json', 'cache-control': 'no-store' },
        body: JSON.stringify({ session_id: body.sessionId, expected_revision: body.expectedRevision,
          event_id: body.eventId, kind: body.kind, text: body.text }) });
      if (!response.ok) {
        try { await response.body?.cancel(); } catch {}
        return reply({ ok: false, error: response.status === 409 ? 'decision_changed' : 'decision_rejected' }, response.status);
      }
      return reply({ ok: true, value: await response.json() });
    } catch { return reply({ ok: false, error: 'decision_unknown' }, 503); }
  };
}
