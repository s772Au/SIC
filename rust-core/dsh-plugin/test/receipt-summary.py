import json
import re
import sqlite3
import sys

connection = sqlite3.connect(sys.argv[1])

def error_category(error):
    if not isinstance(error, str):
        return None
    if "timed out" in error.lower() or "timeout" in error.lower():
        return "timeout"
    match = re.search(r"provider returned HTTP (\d{3})", error)
    if match:
        return "http_" + match.group(1)
    if "transport failed" in error.lower() or "transport" in error.lower():
        return "transport"
    if "tool call" in error.lower() and ("json" in error.lower() or "arguments" in error.lower()):
        return "invalid_tool_json"
    if "provider response" in error.lower() or "incomplete or invalid json" in error.lower():
        return "malformed_response"
    if "editor delivery" in error.lower() or "editor must" in error.lower():
        return "editor_contract"
    return "other"

turns = connection.execute(
    "SELECT state, phase, error_code FROM dsh_bridge_turns ORDER BY created"
).fetchall()
sessions = connection.execute(
    "SELECT chain_id IS NOT NULL, active_fingerprint IS NOT NULL, worker_calls, host_task_count FROM dsh_bridge_sessions"
).fetchall()
tasks = connection.execute(
    "SELECT task_state, COUNT(*) FROM dsh_bridge_tool_tasks GROUP BY task_state ORDER BY task_state"
).fetchall()
attempts = []
for (raw_data,) in connection.execute("SELECT data FROM objects WHERE kind='worker_attempt' ORDER BY rowid"):
    try:
        attempt = json.loads(raw_data)
    except Exception:
        continue
    raw = attempt.get("raw_response")
    choices = raw.get("choices") if isinstance(raw, dict) else None
    message = choices[0].get("message") if isinstance(choices, list) and choices and isinstance(choices[0], dict) else None
    error = attempt.get("error")
    error_class = error_category(error)
    calls = message.get("tool_calls") if isinstance(message, dict) else None
    attempts.append({
        "error_class": error_class,
        "raw_type": type(raw).__name__,
        "raw_keys": sorted(raw.keys()) if isinstance(raw, dict) else [],
        "choice_count": len(choices) if isinstance(choices, list) else None,
        "finish_reason": choices[0].get("finish_reason") if isinstance(choices, list) and choices and isinstance(choices[0], dict) else None,
        "message_keys": sorted(message.keys()) if isinstance(message, dict) else [],
        "function_names": [call.get("function", {}).get("name") for call in calls
                           if isinstance(call, dict) and isinstance(call.get("function"), dict)]
                          if isinstance(calls, list) else [],
        "usage_keys": sorted(attempt["usage"].keys()) if isinstance(attempt.get("usage"), dict) else [],
        "stale": attempt.get("stale"),
        "interrupted": attempt.get("interrupted"),
        "input_changed": attempt.get("input_changed"),
        "dependencies_revoked": attempt.get("dependencies_revoked"),
    })
chains = []
for (raw_state,) in connection.execute("SELECT state FROM chains"):
    try:
        state = json.loads(raw_state)
    except Exception:
        continue
    jobs = state.get("jobs") if isinstance(state.get("jobs"), dict) else {}
    chains.append({
        "run_state": state.get("run_state"),
        "worker_claim_present": bool(state.get("worker_claim")),
        "cut_seq": state.get("cut_seq"),
        "control_epoch": state.get("control_epoch"),
        "input_epoch": state.get("input_epoch"),
        "jobs": [{"status": job.get("status"), "claim_present": bool(job.get("claim_id"))}
                 for job in jobs.values() if isinstance(job, dict)],
    })
model_attempts = []
for (raw_data,) in connection.execute("SELECT data FROM objects WHERE kind='model_attempt' ORDER BY rowid"):
    try:
        attempt = json.loads(raw_data)
    except Exception:
        continue
    raw = attempt.get("raw_response")
    choices = raw.get("choices") if isinstance(raw, dict) else None
    message = choices[0].get("message") if isinstance(choices, list) and choices and isinstance(choices[0], dict) else None
    error = attempt.get("validation_error")
    error_text = error if isinstance(error, str) else ""
    error_class = error_category(error_text) if error_text.strip() else None
    calls = message.get("tool_calls") if isinstance(message, dict) else None
    model_attempts.append({
        "attempt": attempt.get("attempt"),
        "error_class": error_class,
        "raw_type": type(raw).__name__,
        "raw_keys": sorted(raw.keys()) if isinstance(raw, dict) else [],
        "choice_count": len(choices) if isinstance(choices, list) else None,
        "finish_reason": choices[0].get("finish_reason") if isinstance(choices, list) and choices and isinstance(choices[0], dict) else None,
        "message_keys": sorted(message.keys()) if isinstance(message, dict) else [],
        "function_names": [call.get("function", {}).get("name") for call in calls
                           if isinstance(call, dict) and isinstance(call.get("function"), dict)]
                          if isinstance(calls, list) else [],
        "interrupted": attempt.get("interrupted"),
    })
print(json.dumps({"turns": turns, "sessions": sessions, "task_counts": tasks, "worker_attempts": attempts, "model_attempts": model_attempts, "chains": chains}))
connection.close()
