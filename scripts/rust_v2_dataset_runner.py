"""BEAM/BABILong/CorpusQA execution against an isolated Rust v2 Core.

The baseline sends the complete, unedited history as pending sources through
the same Rust Worker. It is a Rust-adapted baseline, not an official raw-prompt
reproduction. Each question/arm has its own chain to prevent answer leakage.
"""
from contextlib import closing, nullcontext
from datetime import datetime, timezone
import json
import os
from pathlib import Path
import sqlite3
from urllib.parse import urlsplit
import zipfile

from benchmark_scoring import aggregate, babilong_score, judge_requests, parse_judgment
from dataset_adapters import case_metadata, cases, file_sha256, source_units
from rust_v2_test_runtime import MockProvider, OwnedRustCore, maintain_hierarchy, mock_profile, run_bounded_worker

ROOT = Path(__file__).resolve().parents[1]
TASK_GOAL = "Preserve the history so a later question can be answered from its evidence."
EDITOR_PROMPT = "Compress the assigned material while preserving useful facts, chronology, uncertainty, and provenance. Do not invent facts."


def save(path, value):
    Path(path).write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")


def add_rust_plan(plan, editor_mode, worker_call_limit=1, hierarchy_fanout=0):
    if type(worker_call_limit) is not int or worker_call_limit < 1:
        raise ValueError("Worker call limit must be a positive integer")
    calls = {"editor": 0, "worker": 0, "judge": 0}
    for case in plan["cases"]:
        for question in case["questions"]:
            if "sic" in plan["arms"] and editor_mode == "model":
                calls["editor"] += case["wb_chunks"]
            calls["worker"] += len(plan["arms"]) * worker_call_limit
            if not case["dataset"].startswith("babilong"):
                calls["judge"] += (len(question["rubric"]) if question["rubric"] is not None else 1) * len(plan["arms"])
    calls["total"] = sum(calls.values())
    plan["backend"] = "rust-v2"
    plan["editor_mode"] = editor_mode
    plan["worker_call_limit"] = worker_call_limit
    plan["hierarchy_fanout"] = hierarchy_fanout
    plan["hierarchy_policy"] = "deterministic higher-level deduplication after each SIC publication" if hierarchy_fanout else "disabled"
    plan["call_estimate_kind"] = "maximum host-dispatched model steps"
    plan["call_estimate"] = calls
    plan["baseline_semantics"] = "Rust Worker with complete unedited pending history; message boundaries are adapted"
    plan["question_isolation"] = "a new chain for each case, question and arm"
    plan["task_goal"] = TASK_GOAL
    plan["editor_prompt"] = EDITOR_PROMPT
    plan["scoring"] = "pinned BABILong metric; local BEAM/CorpusQA judge, not official benchmark scores"
    return plan


def role_profile(config, role):
    selected = config.get("roles", {}).get(role)
    if isinstance(selected, dict) and set(selected) == {"profile"}:
        selected = selected["profile"]
    if isinstance(selected, str):
        selected = config.get("profiles", {}).get(selected)
    if not isinstance(selected, dict):
        raise ValueError("model configuration must resolve roles." + role)
    return selected


def validate_profile_config(config):
    if not isinstance(config, dict) or not isinstance(config.get("profiles", {}), dict):
        raise ValueError("Rust model config and profiles must be JSON objects")
    def inspect(value):
        if isinstance(value, dict):
            for key, item in value.items():
                if key.lower() in {"api_key", "api_key_value", "authorization", "access_token", "secret"}:
                    raise ValueError("model config must reference credentials by environment variable")
                if key == "retry_editor_http_5xx" and item is True:
                    raise ValueError("disable Editor HTTP retries for a hard per-run model-call budget")
                inspect(item)
        elif isinstance(value, list):
            for item in value:
                inspect(item)

    inspect(config)
    worker = role_profile(config, "worker")
    editor = role_profile(config, "editor")
    for profile in (worker, editor, *config.get("profiles", {}).values()):
        if not isinstance(profile, dict):
            raise ValueError("each Rust model profile must be an object")
        url = urlsplit(str(profile.get("api_base", "")))
        if url.username or url.password or url.query or url.fragment:
            raise ValueError("provider URL cannot contain credentials, query or fragment")
    return worker, editor


def validate_real_config(args):
    if not args.model_config:
        raise ValueError("Rust v2 real-provider execution requires --model-config")
    config = json.loads(Path(args.model_config).read_text(encoding="utf-8-sig"))
    worker, _editor = validate_profile_config(config)
    expected_model = worker.get("model", "")
    if args.model not in {expected_model, expected_model.removeprefix("openai/")}:
        raise ValueError("--model must match the frozen Rust Worker profile")
    expected_endpoint = str(worker.get("api_base", "")).rstrip("/") + "/chat/completions"
    if args.endpoint != expected_endpoint:
        raise ValueError("--endpoint must match the frozen Rust Worker profile")
    return config


class Budget:
    def __init__(self, limit):
        self.limit, self.used = limit, 0

    def take(self):
        if self.used >= self.limit:
            raise ValueError("model call budget exhausted before dispatch")
        self.used += 1


def run_arm(client, case, question, arm, args, budget):
    chain = client.call("chain.create")["chain_id"]
    client.call("config.update", {"editor_feedback": {"max_corrections": 0, "feedback_chars": 8192},
                                 "task_goal": TASK_GOAL, "editor_prompt": EDITOR_PROMPT}, chain)
    blocks, hierarchy_actions = [], []
    for index, unit in enumerate(source_units(case["context"], args.chunk_bytes)):
        messages = (case["protected"] if index == 0 else []) + [{"role": "user", "content": unit}]
        source = client.call("source.ingest", {"request": {"messages": messages},
                             "idempotency_key": f"context-{index}"}, chain)["source_id"]
        if arm == "sic":
            wb = client.call("wb.form", {"source_ids": [source]}, chain)
            if args.editor_mode == "model":
                job = client.call("edit.prepare", {"source_id": source}, chain)
                budget.take()
                result = client.call("edit.run", {"job_id": job["job_id"]}, chain, True)
                if result.get("status") != "ready" or result.get("attempts") != 1:
                    raise ValueError("Editor did not produce exactly one ready attempt")
            else:
                job = client.call("edit.program", {"source_id": source, "rules": []}, chain)
            cut = client.call("cut.publish", {"job_id": job["job_id"],
                              "expected_cut": client.call("chain.get", chain_id=chain)["cut_seq"],
                              "idempotency_key": f"publish-{index}"}, chain)
            blocks.append({"source_id": source, "wb_id": wb["wb_id"], "cb_id": cut["cb_id"]})
            for action in maintain_hierarchy(client, chain, getattr(args, "hierarchy_fanout", 0)):
                hierarchy_actions.append({"level": action["level"], "job_id": action["job_id"],
                    "wb_id": action["wb_id"], "members": action["members"], "cb_id": action["publication"]["cb_id"]})
        else:
            blocks.append({"source_id": source})
    if not blocks:
        raise ValueError("case has no complete input history")
    before = client.call("cut.get", chain_id=chain)
    client.call("source.ingest", {"request": {"messages": [{"role": "user", "content": question["tail"]}]},
                "idempotency_key": "question"}, chain)
    worker, worker_attempts = run_bounded_worker(client, chain, budget, getattr(args, "worker_call_limit", 1))
    answer = worker.get("content")
    pack = client.call("experiment.pack", {}, chain)
    verification = client.call("experiment.verify", {"pack": pack}, chain)
    if verification.get("ok") is not True or verification.get("cut_seq", {}).get("matches") is not True:
        raise ValueError("Rust experiment pack verification failed")
    return {"answer": answer, "chain_id": chain, "blocks": blocks, "hierarchy_actions": hierarchy_actions,
            "worker_attempt_id": worker.get("attempt_id"), "worker_attempt_ids": worker_attempts,
            "active_cb_ids_before_final": [cb["cb_id"] for cb in before["view"]["cbs"]], "pack": pack,
            "pack_verification": verification, "accounting": client.call("accounting.get", {}, chain)}


def run(args, plan, provider_type):
    """Run a frozen plan; provider_type is the existing pinned judge client."""
    if not args.rust_binary:
        raise ValueError("Rust v2 execution requires --rust-binary")
    if args.provider == "chat-completions" and (not getattr(args, "allow_network", False) or
            not args.model or not args.endpoint or not os.environ.get(args.api_key_env)):
        raise ValueError("real provider execution requires explicit network opt-in, model, endpoint and credential environment")
    if plan["call_estimate"]["total"] > args.max_model_calls:
        raise ValueError("planned model calls exceed --max-model-calls")
    config = validate_real_config(args) if args.provider == "chat-completions" else None
    args.out.mkdir(parents=True, exist_ok=False)
    save(args.out / "plan.json", plan)
    snapshot = args.out / "code-snapshot.zip"
    source_files = ["scripts/dataset_runner.py", "scripts/rust_v2_dataset_runner.py", "scripts/rust_v2_test_runtime.py",
                    "scripts/dataset_adapters.py", "scripts/benchmark_scoring.py", "scripts/verify_dataset_run.py",
                    "scripts/run_experiment.py", "scripts/sic_demo.py", "benchmarks/requirements.txt",
                    "benchmarks/source_paths.py", "benchmarks/sources-lock.json", "benchmarks/vendor/sources.json",
                    "benchmarks/vendor/babilong_metrics.py", "benchmarks/vendor/BABILONG-LICENSE"]
    with zipfile.ZipFile(snapshot, "w", zipfile.ZIP_DEFLATED) as archive:
        for name in source_files:
            archive.write(ROOT / name, name)
    model_path = args.out / "rust-model-config.json"
    records, budget = [], Budget(args.max_model_calls)
    manifest = {"schema": "sic-rust-v2-dataset-run-v1", "backend": "rust-v2", "provider": args.provider,
                "binary_sha256": file_sha256(Path(args.rust_binary)), "editor_mode": args.editor_mode,
                "hierarchy_fanout": getattr(args, "hierarchy_fanout", 0),
                "code_snapshot_sha256": file_sha256(snapshot),
                "quality_claim": False, "official_benchmark_score": False,
                "research_evaluation": "not_evaluated", "status": "starting",
                "created_at_utc": datetime.now(timezone.utc).isoformat(),
                "baseline_semantics": plan["baseline_semantics"], "max_model_calls": args.max_model_calls}
    save(args.out / "manifest.json", manifest)
    fixture_context = MockProvider() if args.provider == "mock" else nullcontext(None)
    try:
        with fixture_context as fixture:
            if fixture:
                config = mock_profile(fixture.base)
                judge = provider_type(fixture.base + "/chat/completions", "sic-benchmark-fixture", "fixture", args.max_output_tokens, args.out, args.max_request_bytes)
                extra_env = {"SIC_RUST_EVAL_MOCK_KEY": "fixture"}
            else:
                judge = provider_type(args.endpoint, args.judge_model or args.model, os.environ[args.api_key_env], args.max_output_tokens, args.out, args.max_request_bytes)
                extra_env = {}
            save(model_path, config)
            manifest["model_config_sha256"] = file_sha256(model_path)
            manifest["status"] = "running"
            save(args.out / "manifest.json", manifest)
            with OwnedRustCore(args.rust_binary, args.out, model_path, extra_env) as host:
                case_index = 0
                for dataset in args.dataset:
                    for case in cases(dataset, args.limit_cases, args.question_limit):
                        if case_metadata(case, args.chunk_bytes) != plan["cases"][case_index]:
                            raise ValueError("source selection changed since the frozen plan")
                        for question in case["questions"]:
                            for arm in args.arms:
                                record = {"dataset": dataset, "case_id": case["id"], "question_id": question["id"],
                                          "arm": arm, "reference": question["reference"],
                                          "scorer": question["scorer"], "status": "failed"}
                                try:
                                    evidence = run_arm(host.client, case, question, arm, args, budget)
                                    record.update(evidence)
                                    answer = record["answer"]
                                    record["status"] = "answered-unscored"
                                    if dataset.startswith("babilong"):
                                        record["score"] = babilong_score(question["reference"], answer, question["question"], question["category"])
                                    else:
                                        judgments = []
                                        for messages in judge_requests(question["question"], answer, question["reference"], question["rubric"]):
                                            budget.take()
                                            judgments.append(parse_judgment(judge.call(messages, "judge")))
                                        record["score"] = {"metric": question["scorer"], "score": sum(j["score"] for j in judgments) / len(judgments),
                                                           "judgments": judgments, "official_benchmark_score": False}
                                    record["status"] = "scored"
                                except Exception as error:
                                    message = type(error).__name__ + ": " + str(error)
                                    for env_name in (args.api_key_env, "SIC_RUST_EVAL_MOCK_KEY"):
                                        secret = os.environ.get(env_name)
                                        if secret:
                                            message = message.replace(secret, "[redacted]")
                                    record["error"] = message
                                records.append(record)
                                save(args.out / "records.json", records)
                        case_index += 1
                if case_index != len(plan["cases"]):
                    raise ValueError("not all frozen plan cases were read")
                # Use SQLite's online backup, not a file copy of a live WAL
                # database. This is the portable database evidence for the run.
                snapshot = args.out / "rust-core-snapshot.sqlite"
                with closing(sqlite3.connect((args.out / "rust-core.sqlite").resolve().as_uri() + "?mode=ro", uri=True)) as source:
                    with closing(sqlite3.connect(str(snapshot))) as destination:
                        source.backup(destination)
                        destination.execute("PRAGMA journal_mode=DELETE")
            manifest["status"] = "complete" if records and all(r["status"] == "scored" for r in records) else "completed-with-failures"
            manifest["database_snapshot"] = "rust-core-snapshot.sqlite"
            manifest["model_steps_dispatched"] = budget.used
            manifest["finished_at_utc"] = datetime.now(timezone.utc).isoformat()
            save(args.out / "manifest.json", manifest)
            save(args.out / "scores.json", aggregate(records))
            save(args.out / "completion.json", {"status": manifest["status"], "attempted_model_steps": budget.used,
                 "quality_claim": False, "official_benchmark_score": False, "research_evaluation": "not_evaluated"})
    finally:
        checksums = {p.name: file_sha256(p) for p in sorted(args.out.iterdir())
                     if p.is_file() and p.name != "checksums.json" and not p.name.startswith("rust-core.sqlite")
                     and p.suffix in (".json", ".log", ".sqlite", ".zip")}
        save(args.out / "checksums.json", checksums)
    return 0 if manifest["status"] == "complete" else 1
