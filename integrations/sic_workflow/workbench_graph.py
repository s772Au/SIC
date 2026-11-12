"""Official Studio workbenches. Core records, not Studio state, are authority.

Each editable transaction pauses after fetching the current Core revision.
Resume accepts only the declared form; route/job/provenance are frozen in the
checkpoint. Published content has no edit operation in these graphs.
"""
import copy
from typing import Any, Literal, TypedDict

from pydantic import BaseModel, Field, field_validator
from langgraph.graph import StateGraph, START, END
from langgraph.types import interrupt
from sic_workflow.v2_client import CoreClient


class CandidateForm(BaseModel):
    action: Literal['read', 'revise', 'validate', 'publish'] = 'read'
    edit_mode: Literal['replace', 'append', 'remove', 'pvi_only'] = Field(default='replace', description='Candidate operation editing; pvi_only works even when the delivered operation list is empty (implicit KEEP).')
    operation_number: int = Field(default=1, ge=1)
    op: Literal['KEEP', 'DELETE', 'COMPRESS', 'REPLACE', 'FLAG', 'REORDER', 'ADD'] = 'REPLACE'
    target: str = 'u1'
    content: str = ''
    reason: str = ''
    lifecycle: Literal['P', 'V', 'I'] = 'P'
    view_key: str = 'operator_note'
    risk: Literal['none', 'low', 'medium', 'high', 'unknown'] = 'unknown'
    order: int = 0
    anchor_target: str = 'u1'
    anchor_position: Literal['before', 'after'] = 'after'
    persistent: list[str] = Field(default_factory=list)
    volatile: list[str] = Field(default_factory=list)
    instant: dict[str, Any] = Field(default_factory=dict)
    expected_revision: int = Field(default=0, ge=0)
    expected_cut: int = Field(default=0, ge=-1, description='-1 uses the job base-cut shown before this review; never forces the latest cut')
    allow_write: bool = False
    direct: bool = Field(default=False, description='Apply the typed form in one run without the review interrupt; for GUI save/reload checks.')


class CandidateInput(BaseModel):
    chain_id: str
    job_id: str
    form: CandidateForm = Field(default_factory=CandidateForm)


class CandidateState(TypedDict, total=False):
    chain_id: str
    job_id: str
    form: dict
    current: dict
    original_responses: list
    editable_operations: list
    lifecycles: dict
    reviewed_form: dict
    result: dict
    after: dict
    status: str
    reviewed_cut: int


def _form(state):
    value = state.get('form', {})
    return CandidateForm.model_validate(value).model_dump()


def candidate_load(state):
    current = CoreClient().execute('edit.candidate.get', {'chain_id': state['chain_id']}, {'job_id': state['job_id']})
    body = current.get('result', {})
    snapshot = CoreClient().execute('chain.get', {'chain_id':state['chain_id']}) if current.get('ok') else {}
    job = snapshot.get('result',{}).get('jobs',{}).get(state['job_id'],{})
    return {'current': current, 'original_responses': body.get('original_responses', []),
            'editable_operations': body.get('ops', []), 'lifecycles': body.get('pvi', {}),
            'reviewed_cut': job.get('base_cut',-1),
            'status': 'loaded' if current.get('ok') else 'failed'}


def candidate_review(state):
    form = _form(state)
    if form['action'] == 'read' or not state['current'].get('ok'):
        return {'reviewed_form': form}
    if form.get('direct'):
        return {'reviewed_form': form}
    answer = interrupt({'workbench': 'candidate', 'form_schema': CandidateForm.model_json_schema(),
                        'form': form, 'current_revision': state['current']['result']['revision'],
                        'editable_operations': state['editable_operations'], 'lifecycles': state['lifecycles'],
                        'job_base_cut':state['reviewed_cut'],
                        'readonly_original_responses': state['original_responses'],
                        'readonly_context': {'chain_id': state['chain_id'], 'job_id': state['job_id']},
                        'notice': 'Resume with form fields only. Published CB/op and original responses stay immutable. P/V/I are communication, not protocol controls.'})
    # No arbitrary context/payload passthrough: a resume cannot retarget archives.
    if not isinstance(answer, dict) or set(answer) - set(CandidateForm.model_fields):
        raise ValueError('Resume accepts CandidateForm fields only')
    return {'reviewed_form': CandidateForm.model_validate(answer).model_dump()}


def candidate_apply(state):
    form = state['reviewed_form']
    if not state['current'].get('ok'):
        return {'result': state['current'], 'status': 'failed'}
    action = form['action']
    if action == 'read':
        return {'result': state['current'], 'status': 'readonly'}
    client, context = CoreClient(), {'chain_id': state['chain_id']}
    payload = {'job_id': state['job_id']}
    if action == 'revise':
        ops = copy.deepcopy(state['editable_operations'])
        n = form['operation_number'] - 1
        if form['edit_mode'] in {'replace', 'remove'} and n >= len(ops):
            raise ValueError('Select an existing explicit candidate operation')
        op = {'op': form['op'], 'target': form['target'], 'reason': form['reason']}
        if form['op'] in {'COMPRESS', 'REPLACE', 'FLAG', 'ADD'}:
            op.update(content=form['content'], lifecycle=form['lifecycle'], risk=form['risk'])
            if form['lifecycle'] == 'I':
                op['view_key'] = form['view_key']
        if form['op'] == 'REORDER':
            op['order'] = form['order']
        if form['op'] == 'ADD':
            op.pop('target', None)
            op['anchor'] = {'target': form['anchor_target'], 'position': form['anchor_position']}
        if form['edit_mode'] == 'append':
            ops.append(op)
        elif form['edit_mode'] == 'remove':
            ops.pop(n)
        elif form['edit_mode'] == 'replace':
            ops[n] = op
        payload.update(expected_revision=form['expected_revision'], ops=ops,
                       pvi={'P': form['persistent'], 'V': form['volatile'], 'I': form['instant']})
        operation = 'edit.candidate.update'
    elif action == 'validate':
        operation = 'edit.validate'
        # Core validates the delivered candidate; require the inspected version.
        if form['expected_revision'] != state['current']['result']['revision']:
            return {'result': {'ok': False, 'error': {'code': 'candidate_stale', 'message': 'Reload the current candidate revision'}}, 'status': 'failed'}
    else:
        operation = 'cut.publish'
        expected_cut = state['reviewed_cut'] if form['expected_cut'] == -1 else form['expected_cut']
        if expected_cut < 0: raise ValueError('Reviewed job base-cut is required before publication')
        payload.update(expected_candidate_revision=form['expected_revision'], expected_cut=expected_cut,
                       idempotency_key=f"studio:{state['job_id']}:{form['expected_revision']}")
    result = client.execute(operation, context, payload, allow_write=form['allow_write'])
    after = client.execute('cut.get' if action == 'publish' and result.get('ok') else 'edit.candidate.get', context,
                           {} if action == 'publish' and result.get('ok') else {'job_id': state['job_id']})
    return {'result': result, 'after': after, 'status': 'applied' if result.get('ok') else 'failed'}


builder = StateGraph(CandidateState, input_schema=CandidateInput)
builder.add_node('load_candidate_and_readonly_provenance', candidate_load)
builder.add_node('review_versioned_material', candidate_review)
builder.add_node('apply_authorized_candidate_action', candidate_apply)
builder.add_edge(START, 'load_candidate_and_readonly_provenance')
builder.add_edge('load_candidate_and_readonly_provenance', 'review_versioned_material')
builder.add_edge('review_versioned_material', 'apply_authorized_candidate_action')
builder.add_edge('apply_authorized_candidate_action', END)
candidate_graph = builder.compile()


class MemoryInput(BaseModel):
    chain_id: str
    action: Literal['timeline', 'source_mapping', 'cb_mapping', 'source', 'media', 'communication', 'cb', 'ops', 'proposal', 'view', 'tool_result', 'prefix_plan', 'provider_request', 'projection', 'decision', 'routes', 'board', 'ledger'] = 'timeline'
    object_id: str = ''
    offset: int = Field(default=0, ge=0)
    limit: int = Field(default=2048, ge=1, le=10000)
    query: str = ''
    role: str = 'worker'
    view_scope: Literal['default', 'context_level', 'role'] = 'default'
    context_id: str = 'main'
    level: int = Field(default=1, ge=1)

    @field_validator('role')
    @classmethod
    def legal_role(cls, value):
        if value in ('worker', 'editor'):
            return value
        if not value.startswith('editor:'):
            raise ValueError('Role must be worker, editor, or editor:<identity>')
        name = value[len('editor:'):]
        if not name or name.startswith('editor:') or any(c.isspace() or not c.isprintable() for c in name):
            raise ValueError('Editor identity must be nonempty, printable, and contain no whitespace or repeated role prefix')
        return value


class MemoryState(TypedDict, total=False):
    chain_id: str
    action: str
    object_id: str
    offset: int
    limit: int
    query: str
    role: str
    view_scope: str
    context_id: str
    level: int
    result: dict
    rows: list
    page_text: str
    navigation: dict
    status: str


def navigate(state):
    form = MemoryInput.model_validate(state).model_dump()
    action = form['action']
    payload = {}
    if action == 'timeline':
        operation, payload = 'memory.timeline', {'offset': form['offset'], 'limit': min(form['limit'], 100)}
    elif action in {'source_mapping', 'cb_mapping'}:
        operation, payload = 'memory.locate', {('source_id' if action == 'source_mapping' else 'cb_id'): form['object_id']}
    elif action in {'source', 'media', 'communication', 'cb', 'ops', 'proposal', 'view', 'tool_result', 'prefix_plan', 'provider_request'}:
        operation, payload = 'memory.page', {'kind': action, 'object_id': form['object_id'], 'offset': form['offset'], 'limit': form['limit']}
        if action == 'view' and form['view_scope'] == 'context_level':
            payload.update(context_id=form['context_id'], level=form['level'])
        elif action == 'view' and form['view_scope'] == 'role':
            payload['role'] = form['role']
    elif action == 'projection':
        operation, payload = 'memory.project', {'role': form['role'], 'max_chars': form['limit'], 'query': form['query']}
    else:
        operation = {'decision': 'decision.get', 'routes': 'chain.list', 'board': 'board.get', 'ledger': 'ledger.export'}[action]
    result = CoreClient().execute(operation, {'chain_id': form['chain_id']}, payload)
    body = result.get('result', {})
    return {'result': result, 'rows': body.get('rows', body.get('routes', [])), 'page_text': body.get('value', ''),
            'navigation': {key: body[key] for key in ('offset', 'next_offset', 'total', 'unit', 'full_digest', 'manifest', 'views', 'summary') if key in body},
            'status': 'readonly' if result.get('ok') else 'failed'}


nav = StateGraph(MemoryState, input_schema=MemoryInput)
nav.add_node('navigate_immutable_history_and_current_views', navigate)
nav.add_edge(START, 'navigate_immutable_history_and_current_views')
nav.add_edge('navigate_immutable_history_and_current_views', END)
memory_graph = nav.compile()
