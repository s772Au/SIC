"""Typed official Studio forms for decisions, peers, runtime and host policy.

No generic operation or native-request JSON is accepted by a resume. The
selected route remains bound to the original thread transaction.
"""
from typing import Any, Literal, TypedDict
from pydantic import BaseModel, Field, ConfigDict, model_validator
from langgraph.graph import StateGraph, START, END
from langgraph.types import interrupt
from sic_workflow.v2_client import CoreClient
from sic_workflow.prefix_forms import prefix_patch
from sic_workflow.frontier_forms import frontier_payload, frontier_rows
from sic_workflow.editor_views import editor_postprocess


class PermissionForm(BaseModel):
    allow_write: bool = False
    allow_model: bool = False


class DecisionForm(PermissionForm):
    action: Literal['read', 'maintain', 'work', 'failure', 'candidate', 'overlooked', 'complete'] = 'read'
    expected_revision: int = Field(default=0, ge=0)
    text: str = ''
    evidence_source: str = ''
    evidence_unit: str = ''
    completion_target: str = ''
    event_id: str = ''


class RouteForm(PermissionForm):
    action: Literal['routes', 'bridge_sessions', 'history', 'checkpoint', 'fork', 'socket', 'dispatch', 'status', 'run_peer_work', 'board', 'claim', 'message', 'signal'] = 'routes'
    session_key: str = ''
    checkpoint_id: str = ''
    cut_id: str = ''
    socket_id: str = ''
    socket_ids: list[str] = Field(default_factory=list)
    request_id: str = ''
    question: str = ''
    message_text: str = ''
    message_id: str = ''
    lifecycle: Literal['P', 'V'] = 'V'
    return_lifecycle: Literal['P', 'V'] = 'V'
    message_acl: list[str] = Field(default_factory=list)
    spawn_acl: list[str] = Field(default_factory=list)
    signal_acl: list[str] = Field(default_factory=list)
    signal: Literal['run', 'wait', 'wake', 'stop'] = 'wait'
    max_steps: int = Field(default=8, ge=1, le=100)
    editor_mode: Literal['program', 'model'] = 'program'
    board_id: str = ''
    expected_revision: int = Field(default=0, ge=0)
    task_id: str = ''
    description: str = ''
    task_status: Literal['claimed', 'working', 'blocked', 'completed'] = 'claimed'


class RuntimeForm(PermissionForm):
    model_config = ConfigDict(extra='forbid')
    action: Literal['read', 'configure', 'run', 'schedule', 'frontier', 'recover'] = 'read'
    enabled: bool = False
    editor_mode: Literal['program', 'model'] = 'program'
    worker_mode: Literal['host', 'automatic'] = 'host'
    editor_workers: int = Field(default=2, ge=1)
    seal_chars: int = Field(default=8192, ge=1)
    max_pending_seconds: float = Field(default=1.0, gt=0, le=86400, allow_inf_nan=False)
    editor_chars: int = Field(default=4096, ge=1)
    worker_chars: int = Field(default=8192, ge=1)
    worker_turns: int = Field(default=64, ge=1)
    branch_budget: int = Field(default=4, ge=1)
    max_steps: int = Field(default=8, ge=1, le=100)
    continue_worker: bool = False
    job_id: str = ''
    expected_cut: int = Field(default=0, ge=0)
    schedule_policy: Literal['wait', 'interleave'] = 'wait'
    interleave_budget: int = Field(default=1, ge=0)
    position_budget: int = Field(default=0, ge=0)
    length_budget: int = Field(default=4096, ge=0)
    comparison_role: Literal['worker', 'editor'] = 'worker'
    frontier_editor: str = 'editor-1'
    next_source_id: str = ''
    next_unit_ids: list[str] | None = Field(default=None, description='None selects the entire next WB; [] is an explicit empty scope')

    @model_validator(mode='after')
    def validate_frontier_scope(self):
        if self.action == 'frontier':
            frontier_payload(self.model_dump())
        return self

    recovery_action: Literal['resume_queue', 'abandon_editor', 'mark_worker_indeterminate', 'cancel_collaboration'] = 'resume_queue'
    request_id: str = ''
    confirm: bool = False
    acknowledge_possible_duplicate: bool = False


class ConfigurationForm(PermissionForm):
    model_config = ConfigDict(extra='forbid')
    replace_prefix_layout: bool = False
    prefix_layout: Literal['separate','shared_i'] = 'separate'
    shared_prefix_budget: int | None = Field(default=None, ge=512)
    prefix_identity_mode: Literal['keep','shared','per_role'] = 'keep'
    shared_model: str = ''
    shared_provider_id: str = ''
    shared_cache_namespace: str = ''
    shared_prefix_identity: dict[str,dict[str,str]] = Field(default_factory=dict)
    action: Literal['read', 'update'] = 'read'
    worker_profile: str = ''
    editor_profiles: dict[str, str] = Field(default_factory=dict)
    replace_worker_prompt: bool = False
    worker_prompt: str = ''
    replace_editor_prompt: bool = False
    editor_prompt: str = ''
    deduplicate_exact: bool = False
    drop_empty: bool = False
    normalize_newlines: bool = False
    schedule_policy: Literal['wait', 'interleave'] = 'wait'
    interleave_budget: int = Field(default=1, ge=0)


class ArchiveRead(BaseModel):
    kind: Literal['source', 'cb', 'ops', 'proposal', 'view', 'tool_result', 'prefix_plan', 'provider_request']
    object_id: str
    offset: int = Field(default=0, ge=0)
    limit: int = Field(default=2048, ge=1, le=8192)


class EditorForm(PermissionForm):
    model_config = ConfigDict(extra='forbid')
    action: Literal['read', 'prepare', 'run_one', 'run_all'] = 'read'
    source_id: str = ''
    job_id: str = ''
    editor: str = 'editor-1'
    editor_one_name: str = 'editor-1'
    editor_two_name: str = 'editor-2'
    two_editors: bool = False
    editor_one_units: list[str] = Field(default_factory=list)
    editor_two_units: list[str] = Field(default_factory=list)
    readonly_reads: list[ArchiveRead] = Field(default_factory=list)
    directory_offset: int = Field(default=0, ge=0)
    directory_limit: int = Field(default=20, ge=1, le=100)


class SourceForm(PermissionForm):
    action: Literal['read', 'ingest'] = 'read'
    source_id: str = ''
    native_format: Literal['openai-chat', 'openai-responses', 'anthropic-messages'] = 'openai-chat'
    working_text: str = ''
    protected_instruction: str = ''
    idempotency_key: str = ''


class DomainState(TypedDict, total=False):
    chain_id: str
    form: dict
    current: dict
    readonly_facts: dict
    reviewed_form: dict
    result: dict
    after: dict
    status: str
    records: list
    task_mode: str
    selected_job_id: str
    task_view: dict
    task_directory: list
    task_navigation: dict
    assignment_rows: list
    scope_rows: list
    background_rows: list
    response_rows: list
    model_rows: list
    prefix_rows: list
    alias_rows: list


def decision_request(form):
    action = form['action']
    if action == 'read':
        return 'decision.get', {}
    if action == 'maintain':
        return 'decision.maintain', {}
    event = {'kind': action, 'text': form['text'], 'evidence': []}
    if form['evidence_source']:
        event['evidence'].append({'source_id': form['evidence_source'], 'unit_id': form['evidence_unit']})
    if form['event_id']:
        event['id'] = form['event_id']
    if action == 'complete':
        event['target'] = form['completion_target']
    return 'decision.update', {'expected_revision': form['expected_revision'], 'events': [event]}


def route_request(f):
    a = f['action']
    if a == 'bridge_sessions':
        return 'bridge.sessions', {'session_key': f['session_key']}
    if a in {'routes', 'history', 'status', 'board'}:
        return {'routes': 'chain.list', 'history': 'chain.get', 'status': 'collaboration.status', 'board': 'board.get'}[a], ({'board_id': f['board_id']} if a == 'board' and f['board_id'] else {})
    if a == 'checkpoint':
        return 'chain.checkpoint', ({'cut_id': f['cut_id']} if f['cut_id'] else {})
    if a == 'fork':
        return 'chain.fork', {'checkpoint_id': f['checkpoint_id']}
    if a == 'socket':
        return 'socket.create', {k: f[k] for k in ('checkpoint_id', 'message_acl', 'spawn_acl', 'signal_acl')}
    if a == 'dispatch':
        return 'collaboration.dispatch', {k: f[k] for k in ('socket_ids', 'question', 'request_id', 'lifecycle', 'return_lifecycle')}
    if a == 'run_peer_work':
        return 'collaboration.run', {k: f[k] for k in ('request_id', 'max_steps', 'editor_mode')}
    if a == 'claim':
        payload = {k: f[k] for k in ('task_id', 'expected_revision', 'description')}
        payload['status'] = f['task_status']
        if f['board_id']:
            payload['board_id'] = f['board_id']
        return 'board.claim', payload
    if a == 'message':
        return 'socket.message', {'socket_id': f['socket_id'], 'text': f['message_text'], 'lifecycle': f['lifecycle'], 'message_id': f['message_id']}
    return 'socket.signal', {'socket_id': f['socket_id'], 'signal': f['signal']}


def runtime_request(f):
    a = f['action']
    if a == 'read':
        return 'runtime.get', {}
    if a == 'configure':
        return 'runtime.configure', {k: f[k] for k in ('enabled', 'editor_mode', 'worker_mode', 'editor_workers', 'seal_chars', 'max_pending_seconds', 'editor_chars', 'worker_chars', 'worker_turns', 'branch_budget', 'max_steps')}
    if a == 'run':
        return 'runtime.run', {k: f[k] for k in ('max_steps', 'continue_worker', 'editor_mode')}
    if a == 'schedule':
        return 'harness.schedule', {'job_id': f['job_id'], 'policy': f['schedule_policy'], 'interleave_budget': f['interleave_budget']}
    if a == 'frontier':
        return 'frontier.compare', frontier_payload(f)
    return 'runtime.recover', {'action': f['recovery_action'], 'job_id': f['job_id'], 'request_id': f['request_id'], 'confirm': f['confirm'], 'acknowledge_possible_duplicate': f['acknowledge_possible_duplicate']}


def configuration_request(f):
    if f['action'] == 'read':
        return 'config.get', {}
    payload = {k: f[k] for k in ('schedule_policy', 'interleave_budget')}
    payload.update(prefix_patch(f))
    if f['worker_profile']:
        payload['worker_profile'] = f['worker_profile']
    if f['editor_profiles']:
        payload['editor_profiles'] = f['editor_profiles']
    for role in ('worker', 'editor'):
        if f['replace_' + role + '_prompt']:
            payload[role + '_prompt'] = f[role + '_prompt']
    payload['program_rules'] = [rule for rule in ('deduplicate_exact', 'drop_empty', 'normalize_newlines') if f[rule]]
    return 'config.update', payload


def editor_request(f):
    a = f['action']
    if a == 'read':
        return 'chain.get', {}
    if a == 'prepare':
        payload = {'source_id': f['source_id'], 'readonly_reads': f['readonly_reads']}
        if f['editor_one_units'] or f['editor_two_units'] or f['two_editors']:
            payload['assignments'] = [{'editor': f['editor_one_name'], 'unit_ids': f['editor_one_units']}]
            if f['editor_two_units'] or f['two_editors']:
                payload['assignments'].append({'editor': f['editor_two_name'], 'unit_ids': f['editor_two_units']})
        return 'edit.prepare', payload
    if a == 'run_one':
        return 'edit.run', {'job_id': f['job_id'], 'editor': f['editor']}
    return 'edit.run_all', {'job_id': f['job_id']}


def source_request(f):
    if f['action'] == 'read':
        return 'source.read', {'source_id': f['source_id']}
    if f['native_format'] == 'openai-responses':
        native = {'input': [{'role': 'user', 'content': f['working_text']}]}
        if f['protected_instruction']:
            native['instructions'] = f['protected_instruction']
    else:
        native = {'messages': [{'role': 'user', 'content': f['working_text']}]}
        if f['protected_instruction']:
            if f['native_format'] == 'anthropic-messages':
                native['system'] = f['protected_instruction']
            else:
                native['messages'].insert(0, {'role': 'system', 'content': f['protected_instruction']})
    payload = {'format': f['native_format'], 'request': native}
    if f['idempotency_key']:
        payload['idempotency_key'] = f['idempotency_key']
    return 'source.ingest', payload


def make_graph(name, model, loader, request, checkpointer=None, presenter=None, postprocess=None):
    class Input(BaseModel):
        chain_id: str = ''
        form: model = Field(default_factory=model)

    def load(state):
        value = CoreClient().execute(loader, {'chain_id': state.get('chain_id', '')})
        if postprocess:
            form = model.model_validate(state.get('form', {})).model_dump()
            value = postprocess(state, form, value, value).get('result', value)
        return {'current': value, 'readonly_facts': value.get('result', {}), 'status': 'loaded' if value.get('ok') else 'failed'}

    def review(state):
        form = model.model_validate(state.get('form', {})).model_dump()
        operation, _ = request(form)
        # Read-only and compare operations never demand a fake write grant.
        caps = CoreClient().capabilities()
        spec = next((x for x in caps.get('operations', []) if x['operation'] == operation), {})
        if not spec.get('write'):
            return {'reviewed_form': form}
        answer = interrupt({'workbench': name, 'form_schema': model.model_json_schema(), 'form': form,
                            'readonly_current': state['readonly_facts'], 'operation': operation,
                            'notice': 'Only form fields may change. Route, original archives and published CB/op are read-only. Model permission is independent.'})
        if not isinstance(answer, dict) or set(answer) - set(model.model_fields):
            raise ValueError('Resume accepts declared domain form fields only')
        return {'reviewed_form': model.model_validate(answer).model_dump()}

    def apply(state):
        f = state['reviewed_form']
        operation, payload = request(f)
        if operation == 'bridge.sessions':
            from sic_workflow.bridge_client import session_bindings
            result = session_bindings({'chain_id': state.get('chain_id', '')}, payload['session_key'])
        else:
            result = CoreClient().execute(operation, {'chain_id': state.get('chain_id', '')}, payload,
                                          allow_write=f['allow_write'], allow_model=f['allow_model'])
        # Fork creates another route; refreshing it uses the Core-returned identity.
        after = CoreClient().execute(loader, result.get('context', {'chain_id': state['chain_id']}))
        if operation == 'source.ingest' and result.get('ok'):
            after = CoreClient().execute('source.read', result['context'], {'source_id': result['result'].get('part_source_id', result['result']['source_id'])})
        output = {'result': result, 'after': after, 'records': presenter(result.get('result',{})) if presenter and result.get('ok') else [],
                  'status': 'applied' if result.get('ok') else 'failed'}
        if postprocess:
            output.update(postprocess(state, f, result, after))
        return output

    b = StateGraph(DomainState, input_schema=Input)
    b.add_node('load_' + name, load)
    b.add_node('review_authorized_' + name, review)
    b.add_node('apply_' + name, apply)
    b.add_edge(START, 'load_' + name)
    b.add_edge('load_' + name, 'review_authorized_' + name)
    b.add_edge('review_authorized_' + name, 'apply_' + name)
    b.add_edge('apply_' + name, END)
    return b.compile(checkpointer=checkpointer)


decision_graph = make_graph('four_view_decision', DecisionForm, 'decision.get', decision_request)
routes_graph = make_graph('routes_and_peer_work', RouteForm, 'chain.get', route_request)
runtime_graph = make_graph('runtime_and_edit_window', RuntimeForm, 'runtime.get', runtime_request, presenter=frontier_rows)
configuration_graph = make_graph('host_profiles_and_teaching', ConfigurationForm, 'config.get', configuration_request)
editor_graph = make_graph('editor_scope_and_readonly_navigation', EditorForm, 'chain.get', editor_request, postprocess=editor_postprocess)
source_graph = make_graph('native_text_and_protocol_sidecar', SourceForm, 'chain.get', source_request)
