"""Read-only binding discovery for the same authenticated Core bridge."""
import os
from sic_workflow.v2_client import CoreClient


def session_bindings(context=None, session_key=''):
    context = dict(context or {})
    reply = CoreClient(os.environ.get('SIC_BRIDGE_URL', 'http://127.0.0.1:18890'))._request('/api/v2/bridge/sessions')
    if reply.get('ok') is False or 'error' in reply:
        return {'ok': False, 'operation': 'bridge.sessions', 'context': context, 'error': reply.get('error', {})}
    sessions = reply.get('sessions', [])
    if session_key:
        selected = next((entry for entry in sessions if entry['session_key'] == session_key), None)
        if selected is None:
            return {'ok': False, 'context': context, 'error': {'code': 'session_not_visible', 'message': 'Session is not present in this authenticated identity\'s bindings'}}
        context['chain_id'] = selected['chain_id']
    return {'ok': True, 'operation': 'bridge.sessions', 'context': context, 'result': {'sessions': sessions},
            'evidence': {'read_only': True, 'source': 'authenticated-same-core-bridge', 'dsh_roundtrip_verified_by_discovery': False}}
