import hashlib
import json
import sqlite3
import sys
from pathlib import Path

path = Path(sys.argv[1]).resolve()
uri = path.as_uri() + '?mode=ro'
db = sqlite3.connect(uri, uri=True)

def digest(value):
    if not isinstance(value, str):
        return None
    return hashlib.sha256(value.encode('utf-8')).hexdigest()[:12]

requested_chain = sys.argv[2] if len(sys.argv) > 2 else None
if requested_chain is None:
    row = db.execute('SELECT chain_id FROM dsh_bridge_sessions WHERE chain_id IS NOT NULL ORDER BY rowid LIMIT 1').fetchone()
    requested_chain = row[0] if row else None

sessions = []
session_keys = []
if requested_chain is not None:
    for session_key, chain_id, model_alias, worker_calls, host_task_count, active in db.execute(
        'SELECT session_key,chain_id,model_alias,worker_calls,host_task_count,active_fingerprint FROM dsh_bridge_sessions WHERE chain_id=? ORDER BY rowid',
        (requested_chain,),
    ):
        session_keys.append(session_key)
        sessions.append({
            'session_digest': digest(session_key),
            'chain_digest': digest(chain_id),
            'model_alias': model_alias,
            'worker_calls': worker_calls,
            'host_task_count': host_task_count,
            'active': active is not None,
        })

turns = []
if session_keys:
    placeholders = ','.join('?' for _ in session_keys)
    for session_key, state, phase, error_code in db.execute(
        f'SELECT session_key,state,phase,error_code FROM dsh_bridge_turns WHERE session_key IN ({placeholders}) ORDER BY created,rowid',
        session_keys,
    ):
        turns.append({'session_digest': digest(session_key), 'state': state, 'phase': phase, 'error_code': error_code})

task_counts = []
if session_keys:
    placeholders = ','.join('?' for _ in session_keys)
    task_counts = [
        {'state': state, 'count': count}
        for state, count in db.execute(
            f'SELECT task_state,COUNT(*) FROM dsh_bridge_tool_tasks WHERE session_key IN ({placeholders}) GROUP BY task_state ORDER BY task_state',
            session_keys,
        )
    ]

host_tool_tasks = []
if requested_chain is not None and 'host_tool_tasks' in {row[0] for row in db.execute("SELECT name FROM sqlite_master WHERE type='table'")}:
    for (raw_state,) in db.execute('SELECT state FROM host_tool_tasks'):
        try:
            task = json.loads(raw_state)
        except Exception:
            continue
        if task.get('chain_id') != requested_chain:
            continue
        host_tool_tasks.append({
            'task_digest': digest(task.get('task_id')),
            'status': task.get('status'),
            'claim_present': isinstance(task.get('claim_id'), str) and bool(task['claim_id']),
            'claim_digest': digest(task.get('claim_id')),
            'claimed_by_owner': isinstance(task.get('actor'), str) and task.get('claimed_by') == task.get('actor'),
            'completion_present': isinstance(task.get('completion'), dict),
            'result_fingerprint_present': isinstance(task.get('result_fingerprint'), str),
        })

chain = None
legacy_queue_counts = []
if requested_chain is not None:
    if 'runtime_queue' in {row[0] for row in db.execute("SELECT name FROM sqlite_master WHERE type='table'")}:
        legacy_queue_counts = [
            {'status': status, 'count': count}
            for status, count in db.execute(
                'SELECT status,COUNT(*) FROM runtime_queue WHERE chain_id=? GROUP BY status ORDER BY status',
                (requested_chain,),
            )
        ]
    row = db.execute('SELECT state FROM chains WHERE id=?', (requested_chain,)).fetchone()
    if row:
        try:
            state = json.loads(row[0])
            jobs = state.get('jobs') if isinstance(state.get('jobs'), dict) else {}
            chain = {
                'chain_digest': digest(state.get('chain_id')),
                'run_state': state.get('run_state'),
                'cut_seq': state.get('cut_seq'),
                'worker_claim_present': bool(state.get('worker_claim')),
                'jobs': [
                    {'status': job.get('status'), 'claim_present': bool(job.get('claim_id'))}
                    for job in jobs.values() if isinstance(job, dict)
                ],
            }
        except Exception:
            chain = {'parse_error': True}

print(json.dumps({'sessions': sessions, 'turns': turns, 'task_counts': task_counts,
                  'host_tool_tasks': host_tool_tasks, 'legacy_queue_status_counts': legacy_queue_counts, 'chain': chain}))
db.close()
