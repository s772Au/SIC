"""Check a synthetic DSH peer's status without disclosing session data or bearer."""
import json
import os
from pathlib import Path
import sqlite3
import sys
import urllib.error
import urllib.request


def main():
    if len(sys.argv) != 6:
        raise ValueError("expected fixture database, actor, chain, loopback port, and role")
    database = Path(sys.argv[1]).resolve(strict=True)
    actor, chain = sys.argv[2:4]
    port = int(sys.argv[4])
    role = sys.argv[5]
    if role not in {"requester", "peer"}:
        raise ValueError("invalid fixture role")
    if not 1 <= port <= 65535:
        raise ValueError("invalid loopback port")
    bearer = os.environ.get("SIC_CORE_TOKEN", "")
    if not bearer:
        raise ValueError("fixture bearer missing")
    with sqlite3.connect(database.as_uri() + "?mode=ro", uri=True) as db:
        rows = db.execute(
            "SELECT session_key FROM dsh_bridge_sessions WHERE actor=? AND chain_id=?",
            (actor, chain),
        ).fetchall()
    if len(rows) != 1 or not isinstance(rows[0][0], str):
        raise ValueError("fixture session is not unique")
    session = rows[0][0]
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    url = f"http://127.0.0.1:{port}/v1/sic/status"

    def post(body, authenticated=True, path=url):
        headers = {"Content-Type": "application/json"}
        if authenticated:
            headers["Authorization"] = "Bearer " + bearer
        request = urllib.request.Request(path, data=json.dumps(body).encode(), headers=headers, method="POST")
        try:
            with opener.open(request, timeout=5) as response:
                return response.status, json.load(response), response.headers.get("Cache-Control")
        except urllib.error.HTTPError as error:
            return error.code, None, error.headers.get("Cache-Control")

    unauthenticated, _, _ = post({"session_id": session}, authenticated=False)
    code, status, cache = post({"session_id": session})
    if unauthenticated != 401 or code != 200 or cache != "no-store":
        raise ValueError("fixture authentication or cache behavior failed")
    last = status.get("last_turn") or {}
    references = last.get("references") or {}
    if (status.get("chain_id") != chain or status.get("active") is not False
            or status.get("issued_host_tasks") != 0 or last.get("state") != "delivered"
            or last.get("worker_rounds_recorded") != 1 or not references.get("last_worker_attempt_id")):
        raise ValueError("fixture status did not reflect the completed request")
    if role == "peer":
        if last.get("tool_completions_recorded") != 1 or references.get("wb_id") is not None:
            raise ValueError("fixture status did not reflect the tool continuation")
        events_url = f"http://127.0.0.1:{port}/v1/sic/tool-events"
        event_code, event_page, _ = post({"session_id": session}, path=events_url)
        items = event_page.get("items", []) if event_page else []
        if event_code != 200 or len(items) != 1 or items[0].get("tool_name") != "read" \
                or items[0].get("native_outcome") != "ok" or items[0].get("task_state") != "completed" \
                or items[0].get("core_observed") is not True or items[0].get("completion_outcome") != "ok":
            raise ValueError("native DSH read event was not matched to its tool task")
        with sqlite3.connect(database.as_uri() + "?mode=ro", uri=True) as db:
            rows = db.execute("SELECT succeeded FROM sic_tool_dispatch_results WHERE chain_id=? AND actor=?", (chain, actor)).fetchall()
        if rows != [(1,)]:
            raise ValueError("native outcome was not received before tool continuation")
    else:
        cb_id = references.get("published_cb_id")
        wb_id = references.get("wb_id")
        if not cb_id or not wb_id or not references.get("published_cut_id"):
            raise ValueError("requester status lacks a published CB/WB pair")
        operation_url = f"http://127.0.0.1:{port}/api/v2/operations"
        op = {"operation": "hierarchy.expand", "context": {"actor": actor, "chain_id": chain,
               "allow_write": False, "allow_model": False}, "payload": {"cb_id": cb_id}}
        code, expanded, _ = post(op, path=operation_url)
        if code != 200 or expanded.get("result", {}).get("wb_id") != wb_id:
            raise ValueError("requester CB/WB mapping does not match Core")
    redacted = json.dumps(status)
    for sentinel in ("SIC_TOOL_FIXTURE.txt", "SIC_RUST_SYNTHETIC_FIXTURE_ONLY", "SIC_PEER_CHILD_TASK_SYNTHETIC"):
        if sentinel in redacted:
            raise ValueError("fixture content leaked into status")
    revision = status.get("revision")
    _, unchanged, _ = post({"session_id": session, "if_revision": revision})
    if unchanged != {"unchanged": True, "revision": revision}:
        raise ValueError("conditional fixture status failed")
    accounting_url = f"http://127.0.0.1:{port}/v1/sic/accounting"
    code, accounting, _ = post({"session_id": session}, path=accounting_url)
    groups = accounting.get("groups", []) if accounting else []
    if code != 200 or not {"editor", "worker"}.issubset({g.get("role") for g in groups}) \
            or any(g.get("requested_model") != "openai/mock-model" for g in groups) \
            or any(g.get("prompt_tokens", {}).get("unknown_attempts") != 0 for g in groups):
        raise ValueError("Editor/Worker accounting was not grouped by model")
    print(json.dumps({"status_verified": True, "condition_verified": True, "role": role,
                      "native_event_verified": role == "peer", "accounting_verified": True,
                      "sensitive_fixture_text_in_status": False}))


if __name__ == "__main__":
    try:
        main()
    except Exception:
        # Do not print paths, session identities, bearer values or response bodies.
        print(json.dumps({"status_verified": False, "error": "fixture_status_check_failed"}), file=sys.stderr)
        raise SystemExit(1)
