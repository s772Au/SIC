"""Official Studio ledger/independently verified comparison workbench."""
import json
from pathlib import Path
import sys
from typing import Literal, TypedDict
from pydantic import BaseModel, Field,ConfigDict
from langgraph.graph import StateGraph, START, END
from langgraph.types import interrupt
from sic_workflow.v2_client import CoreClient
from sic_workflow.accounting_scope import Price,LedgerScope,ledger_payload,ledger_tables


class AccountingInput(LedgerScope):
    model_config=ConfigDict(extra='forbid')
    chain_id: str = ''
    action: Literal['ledger', 'compare_routes', 'verified_experiment', 'record_resource'] = 'ledger'
    baseline_chain: str = ''
    sic_chain: str = ''
    run_directory: str = ''
    prices: list[Price] = Field(default_factory=list)
    record_id: str = ''
    resource: Literal['program', 'storage', 'other-service'] = 'program'
    quantity: str = '0'
    unit: str = 'cpu-second'
    measurement_method: str = ''
    allow_write: bool = False


class AccountingIngress(TypedDict):
    # Preserve the entire input until strict validation. Flat input channels
    # otherwise drop an unknown alias before the node can reject it.
    __root__:AccountingInput


def inspect_ledger(state):
    f = AccountingInput.model_validate(state.get('__root__',state)).model_dump(exclude_none=True)
    client = CoreClient()
    context = {'chain_id': f['chain_id']}
    verification = {}
    if f['action'] == 'verified_experiment':
        root = Path(__file__).resolve().parents[2]
        relative = Path(f['run_directory'])
        if not f['run_directory'] or relative.is_absolute() or '..' in relative.parts:
            raise ValueError('Choose a project-relative completed experiment directory')
        directory = (root / relative).resolve()
        directory.relative_to(root)
        if str(root / 'scripts') not in sys.path:
            sys.path.insert(0, str(root / 'scripts'))
        from verify_v2_experiment import validate_run
        verification = validate_run(directory)
        if verification['status'] != 'passed':
            raise ValueError('Experiment is incomplete or invalid')
        result = {'ok': True, 'result': json.loads((directory / 'comparison.json').read_text(encoding='utf-8')),
                  'evidence': {'source': 'independent-run-validator', 'quality': 'not_evaluated'}}
    else:
        if f['action'] == 'record_resource':
            record = {k: f[k] for k in ('record_id', 'resource', 'quantity', 'unit', 'measurement_method')}
            answer = interrupt({'readonly_context': context, 'resource_measurement': record,
                                'allow_write': f['allow_write'], 'notice': 'Confirm an actual measured record. Resume {confirm:true,allow_write:true}. Model calls remain off.'})
            if not isinstance(answer, dict) or set(answer) - {'confirm', 'allow_write'} or answer.get('confirm') is not True:
                raise ValueError('Explicit resource measurement confirmation required')
            result = client.execute('resource.record', context, record, allow_write=answer.get('allow_write') is True)
            if not result.get('ok'):
                return {'result': result, 'status': 'failed'}
        payload = ledger_payload(f)
        if f['action'] == 'compare_routes':
            if f['scope']!='chain':raise ValueError('Route comparison uses two single-chain ledgers. Read each task root separately to retain its explicit scope.')
            from sic_core.accounting import compare_workflows
            baseline = client.execute('ledger.export', {'chain_id': f['baseline_chain']}, payload)
            sic = client.execute('ledger.export', {'chain_id': f['sic_chain']}, payload)
            if not baseline.get('ok') or not sic.get('ok'):
                return {'result': baseline if not baseline.get('ok') else sic, 'status': 'failed'}
            result = {'ok': True, 'result': compare_workflows(baseline['result']['summary'], sic['result']['summary']),
                      'evidence': {'paired_task_verified': False, 'quality': 'not_scored'}}
        else:
            result = client.execute('ledger.export', context, payload)
    body = result.get('result', {})
    summaries = [(name, body[name]) for name in ('baseline', 'sic') if name in body] or [('current', body.get('summary', body))]
    rows, unknown, timing = [], [], {}
    for name, summary in summaries:
        rows.extend({'workflow': name, **row} for row in summary.get('rows', []) + summary.get('resource_rows', []))
        unknown.extend({'workflow': name, 'item': item} for item in summary.get('unknown', []))
        timing[name] = summary.get('timing', {})
    return {'result': result, 'charge_rows': rows, 'unknown': unknown, 'timing': timing, 'verification': verification,**ledger_tables(body),
            'status': 'readonly' if result.get('ok') else 'failed'}


# A root channel must not be mixed with named state channels: Studio requests
# the state schema as well as the input schema. Keep the entire transaction in
# the supported dict root channel so strict validation sees unknown fields.
builder = StateGraph(dict, input_schema=AccountingIngress)
builder.add_node('read_actual_ledger_or_verify_paired_artifact', inspect_ledger)
builder.add_edge(START, 'read_actual_ledger_or_verify_paired_artifact')
builder.add_edge('read_actual_ledger_or_verify_paired_artifact', END)
graph = builder.compile()
