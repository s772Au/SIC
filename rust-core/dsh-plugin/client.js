/* SIC's DSH conversation view. The host owns all credentials. */
window.__ModuleLoader__.load({ id: 'sic-core-rust-dsh', factory: (require) => {
  const React = require('react');
  const h = React.createElement;
  const { useEffect, useRef, useState } = React;
  const box = { border: '1px solid var(--border, #d7dce2)', borderRadius: 10, padding: 16, marginBottom: 14 };
  const subdued = { opacity: 0.7, fontSize: 12, lineHeight: 1.5 };
  const code = { overflowWrap: 'anywhere', fontSize: 12 };
  const cell = { textAlign: 'left', padding: '8px 10px', borderBottom: '1px solid var(--border, #d7dce2)', verticalAlign: 'top' };
  const names = { ok: '工具返回正常', error: '工具返回错误', not_started: '工具未启动', unknown: '执行结果未知' };

  function count(value) {
    if (!value || value.reported_attempts === 0) return '未知';
    const known = Number(value.known_tokens || 0).toLocaleString();
    return value.unknown_attempts ? `${known}（${value.unknown_attempts} 次未知）` : known;
  }
  function errorText(part) {
    return part?.error === 'session_not_found' ? '当前会话尚未关联 SIC。' : '暂时无法读取 SIC 状态。';
  }
  function Ref({ name, value }) {
    return h('div', { style: { margin: '7px 0' } }, h('span', { style: subdued }, `${name} `),
      h('code', { style: code }, value || '本次请求未记录'));
  }
  function Hierarchy({ sessionId }) {
    const [level, setLevel] = useState(1);
    const [kind, setKind] = useState('cb');
    const [timeline, setTimeline] = useState(null);
    const [expanded, setExpanded] = useState(null);
    const [page, setPage] = useState(null);
    const [error, setError] = useState('');
    const [busy, setBusy] = useState(false);
    const generation = useRef(0);
    useEffect(() => { generation.current++; setTimeline(null); setExpanded(null); setPage(null); setError(''); }, [sessionId]);
    async function query(action, params) {
      const current = generation.current;
      setBusy(true); setError('');
      try {
        const response = await fetch('api/sic-core/context', { method: 'POST', headers: { 'content-type': 'application/json' },
          body: JSON.stringify({ sessionId, action, params }) });
        const body = await response.json();
        if (!response.ok || body?.ok !== true) throw new Error('unavailable');
        return current === generation.current ? body.value.result : null;
      } catch { if (current === generation.current) setError('读取失败：请检查层级、权限及 SIC 连接。'); return null; }
      finally { if (current === generation.current) setBusy(false); }
    }
    async function load(offset = 0) {
      const value = await query('timeline', { level: Number(level), kind, offset, limit: 20 });
      if (value) { setTimeline({ ...value, offset }); setExpanded(null); setPage(null); }
    }
    async function open(row) {
      const id = row.cb_id ? { cb_id: row.cb_id } : { wb_id: row.wb_id };
      const value = await query('expand', id);
      if (value) { setExpanded(value); setPage(null); }
    }
    async function readPage(which, offset = 0) {
      const objectId = which === 'wb' ? expanded?.wb_id : expanded?.cb_id;
      if (!objectId) return;
      const value = await query('page', { kind: which, object_id: objectId, offset, limit: 8192 });
      if (value) setPage({ ...value, which });
    }
    return h('section', { style: box, 'aria-label': 'WB 与 CB 时间线' },
      h('h3', { style: { marginTop: 0 } }, 'WB / CB 时间线'),
      h('p', { style: subdued }, '按需读取主上下文中的同阶对应、前后顺序与下层成员。已发布 CB 只读。'),
      h('div', { style: { display: 'flex', gap: 8, flexWrap: 'wrap', alignItems: 'center' } },
        h('label', null, '层级 ', h('input', { type: 'number', min: 1, max: 128, value: level,
          onChange: e => setLevel(Math.max(1, Math.min(128, Number(e.target.value) || 1))), style: { width: 64 } })),
        h('label', null, '链 ', h('select', { value: kind, onChange: e => setKind(e.target.value) },
          h('option', { value: 'cb' }, 'CB'), h('option', { value: 'wb' }, 'WB'))),
        h('button', { type: 'button', disabled: busy, onClick: () => void load(0) }, '读取时间线')),
      error ? h('p', { role: 'alert' }, error) : null,
      timeline ? h(React.Fragment, null,
        h('p', { style: subdued }, `共 ${timeline.total} 项，从 ${timeline.offset + 1} 开始显示。`),
        ...(timeline.rows || []).map(row => h('div', { key: row.id, style: { borderTop: '1px solid var(--border, #d7dce2)', padding: '9px 0' } },
          h('button', { type: 'button', disabled: busy, onClick: () => void open(row) }, `展开 #${row.membership_index + 1}`),
          h(Ref, { name: 'WB', value: row.wb_id }), h(Ref, { name: 'CB', value: row.cb_id }),
          h(Ref, { name: '前一项', value: row.predecessor }))),
        h('div', { style: { display: 'flex', gap: 8 } },
          timeline.offset > 0 ? h('button', { type: 'button', disabled: busy, onClick: () => void load(Math.max(0, timeline.offset - 20)) }, '上一页') : null,
          timeline.next_offset != null ? h('button', { type: 'button', disabled: busy, onClick: () => void load(timeline.next_offset) }, '下一页') : null)) : null,
      expanded ? h('div', { style: { marginTop: 14 } }, h('h4', null, `第 ${expanded.level} 层对应关系`),
        h(Ref, { name: 'WB', value: expanded.wb_id }), h(Ref, { name: 'CB', value: expanded.cb_id }),
        h('p', { style: subdued }, `下层成员 ${(expanded.members || []).length} 项 · 来源单元 ${(expanded.original_units || []).length} 项`),
        ...(expanded.members || []).map(member => h('div', { key: member.cb_id }, h('button', { type: 'button', disabled: busy,
          onClick: () => void open({ cb_id: member.cb_id }) }, `展开第 ${member.level} 层 CB`), ' ', h('code', { style: code }, member.cb_id))),
        h('details', null, h('summary', null, '查看 WB 来源单元'), h('pre', { style: { whiteSpace: 'pre-wrap', overflowWrap: 'anywhere', fontSize: 12 } }, JSON.stringify(expanded.original_units, null, 2))),
        h('div', { style: { display: 'flex', gap: 8, flexWrap: 'wrap' } },
          h('button', { type: 'button', disabled: busy, onClick: () => void readPage('wb') }, '读取 WB'),
          expanded.cb_id ? h('button', { type: 'button', disabled: busy, onClick: () => void readPage('cb') }, '读取 CB') : null,
          expanded.cb_id ? h('button', { type: 'button', disabled: busy, onClick: () => void readPage('ops') }, '读取编辑操作') : null)) : null,
      page ? h('div', null, h('h4', null, `${page.which.toUpperCase()} 内容`),
        h('pre', { style: { whiteSpace: 'pre-wrap', overflowWrap: 'anywhere', maxHeight: 360, overflow: 'auto', fontSize: 12 } }, page.value || ''),
        page.next_offset != null ? h('button', { type: 'button', disabled: busy, onClick: () => void readPage(page.which, page.next_offset) }, '下一段') : null) : null);
  }
  function Decision({ sessionId }) {
    const [decision, setDecision] = useState(null);
    const [kind, setKind] = useState('work');
    const [text, setText] = useState('');
    const [message, setMessage] = useState('');
    const [busy, setBusy] = useState(false);
    const pending = useRef(false);
    const generation = useRef(0);
    useEffect(() => { generation.current++; setDecision(null); setMessage(''); setText(''); }, [sessionId]);
    async function load() {
      if (pending.current) return;
      const current = generation.current; pending.current = true; setBusy(true); setMessage('');
      try {
        const response = await fetch('api/sic-core/context', { method: 'POST', headers: { 'content-type': 'application/json' },
          body: JSON.stringify({ sessionId, action: 'decision', params: {} }) });
        const body = await response.json();
        if (!response.ok || body?.ok !== true || !Number.isSafeInteger(body.value?.result?.decision?.revision)) throw new Error('unavailable');
        if (current === generation.current) setDecision(body.value.result.decision);
      } catch { if (current === generation.current) setMessage('决策树读取失败，请检查 SIC 连接。'); }
      finally { pending.current = false; if (current === generation.current) setBusy(false); }
    }
    async function add(event) {
      event.preventDefault();
      if (pending.current || !decision || !text.trim()) return;
      const current = generation.current; pending.current = true; setBusy(true); setMessage('');
      try {
        const response = await fetch('api/sic-core/decision-event', { method: 'POST', headers: { 'content-type': 'application/json' },
          body: JSON.stringify({ sessionId, expectedRevision: decision.revision, eventId: `user-${crypto.randomUUID()}`, kind, text }) });
        const body = await response.json();
        if (current !== generation.current) return;
        if (response.status === 409) { setDecision(null); setMessage('决策树已变化或模型仍在工作，请重新读取后再编辑。'); return; }
        if (!response.ok || body?.ok !== true || !body.value?.decision) throw new Error('unknown');
        setDecision(body.value.decision); setText(''); setMessage('条目已保存。');
      } catch {
        if (current === generation.current) { setDecision(null); setMessage('保存结果不确定，请先重新读取并核对，系统不会自动重试。'); }
      } finally { pending.current = false; if (current === generation.current) setBusy(false); }
    }
    const views = decision?.views || {};
    const labels = { 'Working on': '进行中', 'Failed Before': '曾经失败', 'Not Tried Yet': '尚未尝试', Overlooked: '可能忽略' };
    return h('section', { style: box, 'aria-label': '决策树' }, h('h3', { style: { marginTop: 0 } }, '决策树'),
      h('p', { style: subdued }, '读取当前四个视角，并以版本检查添加条目；不会改写已发布 CB。'),
      h('button', { type: 'button', disabled: busy, onClick: () => void load() }, '读取决策树'),
      message ? h('p', { role: 'status' }, message) : null,
      decision ? h(React.Fragment, null, h('p', { style: subdued }, `当前版本 ${decision.revision}`),
        ...Object.entries(labels).map(([name, label]) => h('div', { key: name }, h('h4', null, label),
          (views[name] || []).length ? h('ul', null, ...(views[name] || []).map(row => h('li', { key: row.id }, row.text))) : h('p', { style: subdued }, '暂无条目'))),
        h('form', { onSubmit: add }, h('label', null, '添加至 ', h('select', { value: kind, onChange: e => setKind(e.target.value), disabled: busy },
          h('option', { value: 'work' }, '进行中'), h('option', { value: 'candidate' }, '尚未尝试'), h('option', { value: 'overlooked' }, '可能忽略'))),
          h('textarea', { value: text, onChange: e => setText(e.target.value), maxLength: 4096, rows: 3, disabled: busy,
            'aria-label': '决策条目', placeholder: '输入决策或待考虑的事项', style: { display: 'block', width: '100%', margin: '8px 0' } }),
          h('button', { type: 'submit', disabled: busy || !text.trim() }, '保存决策条目'))) : null,
      h('p', { style: subdued }, '目前只支持新增上述三类条目；修改、删除和完成确认尚未接入此界面。'));
  }
  function Overview({ sessionId }) {
    const [state, setState] = useState({ loading: true });
    const [refresh, setRefresh] = useState(0);
    useEffect(() => {
      let live = true;
      let busy = false;
      const controller = new AbortController();
      if (!sessionId) { setState({ missing: true }); return () => controller.abort(); }
      async function load() {
        if (busy) return;
        busy = true;
        try {
          const response = await fetch('api/sic-core/overview', {
            method: 'POST', headers: { 'content-type': 'application/json' },
            body: JSON.stringify({ sessionId }), signal: controller.signal,
          });
          const data = await response.json();
          if (!response.ok || data?.ok !== true || !data.value) throw new Error('unavailable');
          if (live) setState({ value: data.value });
        } catch { if (live) setState({ error: true }); }
        finally { busy = false; }
      }
      void load();
      return () => { live = false; controller.abort(); };
    }, [sessionId, refresh]);
    if (state.missing) return h('p', null, '打开一个 DSH 会话以查看 SIC。');
    if (state.loading) return h('p', null, '正在读取 SIC 会话…');
    if (state.error) return h('div', null, h('p', null, '无法连接 SIC 宿主接口。'),
      h('button', { type: 'button', onClick: () => { setState({ loading: true }); setRefresh(n => n + 1); } }, '重试'));
    const { status: statusPart, accounting: usagePart, events: eventsPart } = state.value;
    const status = statusPart?.value;
    const latest = status?.last_turn;
    const refs = latest?.references || {};
    const published = status?.last_publication || {};
    const groups = Array.isArray(usagePart?.value?.groups) ? usagePart.value.groups : [];
    const unresolved = usagePart?.value?.unresolved_attempts;
    const pendingActions = usagePart?.value?.pending_worker_actions;
    const supersededActions = usagePart?.value?.superseded_worker_actions;
    const events = Array.isArray(eventsPart?.value?.items) ? eventsPart.value.items : [];
    return h('div', { style: { padding: 20, maxWidth: 1100, margin: 'auto', fontFamily: 'system-ui, sans-serif', lineHeight: 1.5 } },
      h('div', { style: { display: 'flex', justifyContent: 'space-between', alignItems: 'baseline', gap: 12, flexWrap: 'wrap' } },
        h('h2', { style: { margin: '0 0 6px' } }, 'SIC 会话'),
        h('button', { type: 'button', onClick: () => { setState({ loading: true }); setRefresh(n => n + 1); } }, '刷新状态')),
      h('p', { style: subdued }, '展示 SIC 桥接视图；它与 DSH 原始会话历史属于不同的数据来源。'),
      h('section', { style: box, 'aria-label': '上下文状态' }, h('h3', { style: { marginTop: 0 } }, '最近桥接请求'),
        status ? h(React.Fragment, null,
          h('p', null, `状态：${latest?.state || '尚无请求'} · 阶段：${latest?.phase || '—'} · 已发出工具：${status.issued_host_tasks ?? '未知'}`),
          h(Ref, { name: 'Agent / CB 链', value: status.chain_id }),
          h(Ref, { name: 'WB', value: refs.wb_id }), h(Ref, { name: 'Editor 任务', value: refs.editor_job_id }),
          h(Ref, { name: 'CB', value: refs.published_cb_id }), h(Ref, { name: '发布切点', value: refs.published_cut_id }),
          h('h4', null, '该 DSH 会话最近一次发布'),
          h(Ref, { name: '发布时 WB', value: published.wb_id }), h(Ref, { name: '发布的 CB', value: published.cb_id }),
          h(Ref, { name: '发布切点', value: published.cut_id }),
          h('p', { style: subdued }, '工具续接属于另一条请求，WB / CB 为空不代表整条链没有这些对象。旧记录和中断窗口也可能缺少引用。'))
          : h('p', null, errorText(statusPart))),
      h('section', { style: box, 'aria-label': '模型用量' }, h('h3', { style: { marginTop: 0 } }, 'Editor / Worker 模型用量'),
        usagePart?.error ? h('p', null, errorText(usagePart)) : groups.length === 0 ? h('p', null, '暂无已记录的模型调用。') :
          h('div', { style: { overflowX: 'auto' } }, h('table', { style: { borderCollapse: 'collapse', width: '100%', fontSize: 13 } },
            h('thead', null, h('tr', null, ...['角色 / 模型', '调用', '输入 Token', '输出 Token', '输入缓存 Token'].map(x => h('th', { key: x, style: cell }, x)))),
            h('tbody', null, ...groups.map((group, index) => h('tr', { key: `${group.role}-${group.provider_route_fingerprint}-${group.requested_model}-${group.provider_reported_model}-${index}` },
              h('td', { style: cell }, h('strong', null, group.role === 'editor' ? 'Editor' : group.role === 'worker' ? 'Worker' : '未知角色'),
                h('div', { style: code }, group.requested_model || '模型未知'),
                h('div', { style: subdued }, group.provider_origin || 'Provider 未知'),
                group.provider_reported_model ? h('div', { style: subdued }, `返回模型：${group.provider_reported_model}`) : null),
              h('td', { style: cell }, group.attempts), h('td', { style: cell }, count(group.prompt_tokens)),
              h('td', { style: cell }, count(group.completion_tokens)), h('td', { style: cell }, count(group.cached_prompt_tokens))))))),
        Number.isSafeInteger(unresolved) && unresolved > 0 ? h('p', { role: 'status' },
          `${unresolved} 次调用尚无可核对结果：可能仍在运行、未发出或已发生费用。`) : null,
        Number.isSafeInteger(pendingActions) && pendingActions > 0 ? h('p', { role: 'status' },
          `${pendingActions} 次 Worker 已返回用量，但后续操作仍在等待或需要恢复。`) : null,
        Number.isSafeInteger(supersededActions) && supersededActions > 0 ? h('p', { role: 'status' },
          `${supersededActions} 次 Worker 的旧执行权已作废，结果不会采用；旧等待的实际结束状态未知。`) : null,
        h('p', { style: subdued }, '取自 Provider 返回；缓存 Token 已包含在输入中。缺失字段显示未知，费用暂未计价。DSH 自带的单模型用量栏可能显示 0，不代表 SIC 未消耗 Token。')),
      h('section', { style: box, 'aria-label': '原生工具事件' }, h('h3', { style: { marginTop: 0 } }, 'DSH 原生工具事件'),
        eventsPart?.error ? h('p', null, errorText(eventsPart)) : events.length === 0 ? h('p', null, '当前会话暂无 SIC 宿主工具任务。') :
          h('div', { style: { overflowX: 'auto' } }, h('table', { style: { borderCollapse: 'collapse', width: '100%', fontSize: 13 } },
            h('thead', null, h('tr', null, ...['工具', '桥接任务', 'DSH 结果', 'Core 记录', '续接时状态', '事件序号'].map(x => h('th', { key: x, style: cell }, x)))),
            h('tbody', null, ...events.map((item, index) => h('tr', { key: item.call_id || index },
              h('td', { style: cell }, item.tool_name || '未知'), h('td', { style: cell }, item.task_state || '未知'),
              h('td', { style: cell }, names[item.native_outcome] || '未观察到'),
              h('td', { style: cell }, item.core_observed === true ? '已记录' : item.core_observed === false ? '待记录' : '—'),
              h('td', { style: cell }, names[item.completion_outcome] || '未记录'),
              h('td', { style: cell }, item.native_event_seq ?? '—')))))),
        h('p', { style: subdued }, '结果表示 DSH 工具调用的返回状态，不保证后台任务或外部操作最终成功。事件迟到时，续接时状态仍为未知；已记录的新观察会在后续调用的 I 区可见，已发布 CB 不会改写。')),
      h(Hierarchy, { sessionId }),
      h(Decision, { sessionId }),
      h('p', { style: subdued }, '此视图支持主上下文按需追溯与决策条目新增；其他可编辑操作和内部上下文尚未接入。'));
  }
  return {
    name: 'sic-core-rust-dsh', inject: ['slots'],
    apply(ctx) {
      ctx.effect(() => ctx.slots.inject('conversation.view', () => ctx.slots.register(
        { name: 'conversation.view', id: 'sic-core', order: 22, label: () => 'SIC' },
        props => h(Overview, props),
      )), 'sic-core: conversation view');
    },
  };
} });
