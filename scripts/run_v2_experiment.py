"""Execute new-core baseline batch vs SIC incremental on an owned HTTP fixture.

This is a reproducible engineering experiment, not a research quality result.
No flag or configuration can redirect this command to a real provider.
"""
import argparse
import hashlib
import json
from pathlib import Path
import sys
import time
import uuid
import zipfile

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from sic_core.accounting import summarize_records, compare_workflows
from v2_experiment_fixture import OwnedFixture

DEFAULT_TASK = {"task_id": "owned-continuation", "goal": "Retain constraints, attempted paths and evidence while continuing the task.",
                "blocks": ["Constraint: preserve source evidence. First attempt checked the wrong file and failed.",
                           "New evidence: inspect the configuration before trying another route.",
                           "Pending work: verify the candidate and report what remains untested."]}
PROMPTS = {"worker": "Use the SIC workspace and source-navigation tools. Distinguish performed actions, suggestions and untested paths.",
           "editor": "Edit assigned text units only. Keep constraints, facts, failures and source meaning. Return one JSON object {ops:[],pvi:{P:[],V:[],I:{}}}. No repair rounds."}
PRICES = [{"category": category, "unit": "token", "unit_price": value, "currency": "FIXTURE", "price_version": "owned-fictional-v1"}
          for category, value in [("input_uncached", "0.000002"), ("cache_read", "0.0000002"), ("output_nonreasoning", "0.000006"), ("output_reasoning", "0.000006")]]


def save(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, ensure_ascii=False, indent=2, allow_nan=False) + "\n", encoding="utf-8")


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def run_arm(folder, task, arm, shared_model):
    start, cpu_start = time.time(), time.process_time()
    snapshots, steps = [], []
    with OwnedFixture(folder / "core.db", shared_model=shared_model) as fixture:
        client = fixture.client
        client.call("chain.create", {"creators": []})
        client.call("config.update", {"worker_prompt": PROMPTS["worker"] + "\nTask: " + task["goal"],
                                      "editor_prompt": PROMPTS["editor"] + "\nTask: " + task["goal"], "program_rules": [], "schedule_policy": "wait"})
        client.call("runtime.configure", {"enabled": False, "editor_mode": "model", "worker_mode": "host", "seal_chars": 1,
                                          "editor_workers": 1, "editor_chars": 1000000, "worker_chars": 1000000})

        def seal(reason):
            state = client.call("chain.get")
            if not state["pending"]:
                return
            client.call("wb.form", {"source_ids": state["pending"], "reason": reason})
            work = client.call("runtime.run", {"max_steps": 128, "continue_worker": False, "editor_mode": "model"})
            assert any(step.get("action") == "published" for step in work["steps"]), work
            view = client.call("cut.get")
            for old, current in zip(snapshots, view["view"]["cbs"]):
                assert old == current, "Old CB/operations changed"
            snapshots[:] = view["view"]["cbs"]

        for index, block in enumerate(task["blocks"]):
            client.call("source.ingest", {"format": "openai-chat", "request": {"messages": [{"role": "user", "content": block}]}})
            if arm == "sic":
                seal("incremental-task-boundary")
            worker = client.call("provider.continue", {"idempotency_key": f"{task['task_id']}:{arm}:worker:{index}"})
            steps.append({"block": index, "worker_source_id": worker["source_id"], "response": worker["response"]})
        seal("baseline-compress-once-at-end" if arm == "baseline" else "final-worker-tail")
        final = client.call("chain.get")
        usage = client.call("usage.report")
        calls = usage["calls"]
        expected_editors = 1 if arm == "baseline" else len(task["blocks"]) + 1
        assert sum(call["role"] == "worker" for call in calls) == len(task["blocks"])
        assert sum(call["role"] == "editor" for call in calls) == expected_editors
        assert len(final["cbs"]) == expected_editors
        assert not final["pending"]
        # Account fixture interval and measurements explicitly; do not call them production prices.
        end = time.time()
        resources = [{"record_id": "program-cpu", "resource": "program", "quantity": time.process_time() - cpu_start,
                      "unit": "process_cpu_second", "measurement_method": "host process_time delta including SDK/owned fixture overhead",
                      "unit_price": "0.001", "currency": "FIXTURE", "price_version": "owned-fictional-v1"}]
        fixture.core.store.db.execute("PRAGMA wal_checkpoint(FULL)")
        stored_bytes = sum(path.stat().st_size for path in folder.glob("core.db*") if path.is_file())
        resources.append({"record_id": "storage-snapshot", "resource": "storage", "quantity": stored_bytes, "unit": "byte_at_snapshot",
                          "measurement_method": "SQLite database/WAL/SHM file sizes at end of arm; not byte-hours",
                          "unit_price": "0.000000001", "currency": "FIXTURE", "price_version": "owned-fictional-v1"})
        timing_events = []
        tool_starts = {}
        for event in final["events"]:
            if event.get("type") == "tool_start":
                tool_starts[event["call_id"]] = event["time"]
            elif event.get("type") == "tool_end" and event.get("call_id") in tool_starts:
                timing_events.append({"kind": "tool", "start": tool_starts.pop(event["call_id"]), "end": event["time"]})
        ledger = {"calls": calls, "resources": resources, "task_interval": [start, end], "events": timing_events,
                  "source_events": final["events"], "prices": PRICES,
                  "event_scope": "Only observed matched tool intervals. Fixture has no external tools or deliberate serial/interleave waits."}
        save(folder / "ledger.json", ledger)
        save(folder / "trace.json", client.trace)
        save(folder / "model-http.json", fixture.model_requests[fixture.request_start:])
        save(folder / "result.json", {"arm": arm, "task_id": task["task_id"], "chain_id": final["chain_id"], "worker_steps": steps,
                                     "fixed_cbs": snapshots, "pending": final["pending"], "editor_calls": expected_editors,
                                     "worker_calls": len(task["blocks"]), "quality": {"status": "not_evaluated", "score": None}})
        summary = summarize_records(calls, resources, [start, end], PRICES, timing_events)
        save(folder / "summary.json", summary)
        return summary


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--task", type=Path, help="JSON {task_id,goal,blocks:[text,...]}; copied into the frozen run")
    parser.add_argument("--output", type=Path, help="New empty run directory; default .local/experiments-v2/<unique>")
    args = parser.parse_args()
    task = json.loads(args.task.read_text(encoding="utf-8-sig")) if args.task else DEFAULT_TASK
    if not isinstance(task.get("blocks"), list) or not task["blocks"] or any(not isinstance(block, str) for block in task["blocks"]):
        raise ValueError("Task must contain nonempty text blocks")
    output = args.output or ROOT / ".local/experiments-v2" / (time.strftime("%Y%m%d-%H%M%S-") + uuid.uuid4().hex[:8])
    if output.exists() and any(output.iterdir()):
        raise ValueError("Output must be new/empty; old answers are never reused")
    output.mkdir(parents=True, exist_ok=True)
    save(output / "task.json", task)
    save(output / "frozen-configuration.json", {"mode": "owned_http_fixture", "prompts": PROMPTS, "program_rules": [], "prices": PRICES,
         "model": "openai/sic-experiment-fixture", "tokenizer": "gpt-4o", "tokenizer_status": "fixture-tokenizer-not-provider-verified",
         "usage_semantics": "synthetic serialized-character-quanta-v1; cached tokens explicitly zero", "currency": "FIXTURE",
         "baseline": "same Worker turns, all material in one WB compressed once at end", "sic": "publish each task block and final Worker tail",
         "real_provider_calls": 0, "quality": "not_evaluated"})
    source_paths = sorted((ROOT / "sic_core").glob("*.py")) + [ROOT / "sic_core/operations.json", Path(__file__).resolve(),
        ROOT / "scripts/v2_experiment_fixture.py", ROOT / "scripts/verify_v2_experiment.py", ROOT / "scripts/score_v2_experiment.py"]
    with zipfile.ZipFile(output / "code-snapshot.zip", "w", zipfile.ZIP_DEFLATED) as archive:
        for path in source_paths:
            archive.write(path, path.relative_to(ROOT).as_posix())
    status = "passed"
    try:
        with OwnedFixture(output / "fixture-anchor.db") as shared_model:
            baseline = run_arm(output / "baseline", task, "baseline", shared_model)
            sic = run_arm(output / "sic", task, "sic", shared_model)
        comparison = compare_workflows(baseline, sic)
        comparison["quality"] = {"status": "not_evaluated", "score": None, "reason": "owned deterministic endpoint, no model quality inference"}
        comparison["controlled_terms"] = {"status": "not_applied", "reason": "fixture reports zero cache reads; do not impose the F/H cache-hit assumptions"}
        save(output / "comparison.json", comparison)
    except Exception as error:
        status = "failed"
        save(output / "failure.json", {"type": type(error).__name__, "message": str(error), "quality": "not_evaluated"})
    files = {path.relative_to(output).as_posix(): {"sha256": sha(path), "bytes": path.stat().st_size}
             for path in sorted(output.rglob("*")) if path.is_file()}
    manifest = {"schema": "sic-new-experiment-v1", "status": status, "mode": "owned_http_fixture", "real_provider_calls": 0,
                "task_id": task["task_id"], "contract_sha256": sha(ROOT / "paper/review/sic_implementation_contract.md"),
                "files": files, "quality": {"status": "not_evaluated", "score": None}}
    save(output / "manifest.json", manifest)
    print(json.dumps({"status": status, "output": str(output), "real_provider_calls": 0, "quality": "not_evaluated"}))
    if status != "passed":
        raise SystemExit(1)


if __name__ == "__main__":
    main()
