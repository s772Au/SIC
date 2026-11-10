import json
import hashlib
import sqlite3
import sys
from pathlib import Path

db_path = Path(sys.argv[1]).resolve()
parent_id, child_id = sys.argv[2:4]
db = sqlite3.connect(db_path.as_uri() + '?mode=ro', uri=True)

def chain_summary(chain_id):
    row = db.execute('SELECT state FROM chains WHERE id=?', (chain_id,)).fetchone()
    if not row:
        return {'exists': False}
    state = json.loads(row[0])
    jobs = state.get('jobs') if isinstance(state.get('jobs'), dict) else {}
    return {
        'exists': True,
        'run_state': state.get('run_state'),
        'cut_seq': state.get('cut_seq'),
        'worker_claim_present': bool(state.get('worker_claim')),
        'job_statuses': sorted(job.get('status', 'unknown') for job in jobs.values() if isinstance(job, dict)),
        'protected_count': len(state.get('protected', [])) if isinstance(state.get('protected'), list) else None,
        'protected_digest': hashlib.sha256(json.dumps(state.get('protected', []), ensure_ascii=False, sort_keys=True, separators=(',', ':')).encode('utf-8')).hexdigest()[:12],
    }

turn = db.execute('SELECT state,phase,error_code FROM dsh_bridge_turns ORDER BY created DESC,rowid DESC LIMIT 1').fetchone()
objects = {}
for kind, count in db.execute('SELECT kind,COUNT(*) FROM objects GROUP BY kind'):
    if kind in ('editor_attempt', 'worker_attempt', 'edit_job', 'source', 'wb', 'cb', 'cut'):
        objects[kind] = count

print(json.dumps({
    'turn': None if turn is None else {'state': turn[0], 'phase': turn[1], 'error_code': turn[2]},
    'parent': chain_summary(parent_id),
    'child': chain_summary(child_id),
    'relevant_object_counts': objects,
}))
db.close()
