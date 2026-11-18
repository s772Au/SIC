"""Seven frozen compression controls running on an isolated Rust v2 Core.

This is a new Rust artifact format, not a relabelled Python Core result. Model
calls are opt-in, capped, and made only by the isolated Core. Hierarchical
grouping uses deterministic program editing; media inputs fail closed.
"""
from contextlib import closing, nullcontext
from datetime import datetime, timezone
from decimal import Decimal, InvalidOperation
import hashlib
import importlib.util
import json
import os
from pathlib import Path, PurePosixPath
import sqlite3
from urllib.parse import urlsplit
import zipfile

from dataset_adapters import file_sha256, source_entries
from rust_v2_dataset_runner import Budget, save, validate_profile_config
from rust_v2_test_runtime import MockProvider, OwnedRustCore, maintain_hierarchy, mock_profile, run_bounded_worker

ROOT = Path(__file__).resolve().parents[1]
PACK = ROOT / "research/compression/pack"
ENGINES = {"sic", "batch", "structured_summary", "program_exact", "langchain_trim", "unchanged", "llmlingua"}


def validate_prices(prices):
    if not isinstance(prices, list):
        raise ValueError("price table must be an array")
    for row in prices:
        if not isinstance(row, dict) or not all(isinstance(row.get(k), str) and row[k] for k in ("category", "currency", "price_version")):
            raise ValueError("each price requires category, currency and price_version")
        try:
            rate = Decimal(str(row["unit_price"]))
            denominator = Decimal(str(row.get("per_quantity", 1)))
        except (KeyError, InvalidOperation, TypeError, ValueError):
            raise ValueError("price and per_quantity must be finite decimals") from None
        if not rate.is_finite() or rate < 0 or not denominator.is_finite() or denominator <= 0:
            raise ValueError("price must be nonnegative and per_quantity positive")
    for index, first in enumerate(prices):
        for second in prices[index + 1:]:
            if (first["category"] == second["category"] and first.get("unit", "token") == second.get("unit", "token") and
                all(first.get(key) is None or second.get(key) is None or first[key] == second[key] for key in ("role", "model"))):
                raise ValueError("ambiguous price bindings; freeze one rate per applicable category/model/role")
    return prices


def price_estimate(accounting, prices):
    from sic_core.accounting import normalize_usage, summarize_records
    calls = []
    for attempt in accounting.get("attempts", []):
        usage = normalize_usage(attempt.get("provider_reported_usage"), "openai-chat")
        calls.append({"call_id": attempt["record_id"], "role": attempt["role"],
                      "model": attempt.get("model_identity", {}).get("requested_model"), **usage})
    return {"basis": "host-declared prices applied to provider-reported usage; not a provider bill",
            "summary": summarize_records(calls, prices=prices)}


def planned_calls(block_count, engines, worker_call_limit=1):
    if block_count < 1 or worker_call_limit < 1 or not engines or len(set(engines)) != len(engines) or set(engines) - ENGINES:
        raise ValueError("nonempty blocks and distinct supported engines are required")
    worker = (block_count + 1) * len(engines) * worker_call_limit
    editor = sum(block_count + 1 if e == "sic" else 1 if e in {"batch", "structured_summary"} else 0 for e in engines)
    return {"worker": worker, "editor": editor, "total": worker + editor}


def load_task(args, task_override=None):
    if args.dataset and args.task:
        raise ValueError('choose one of --dataset and --task')
    if task_override is not None:
        task = dict(task_override)
    elif args.dataset:
        if args.data_root:
            os.environ["SIC_DATA_ROOT"] = str(args.data_root.resolve())
        # Execution verifies the selected dataset's locked sources before any
        # model process is started. It never downloads or repairs data.
        for entry in source_entries(args.dataset):
            path = Path(entry["resolved_path"])
            if not path.is_file() or path.stat().st_size != entry["bytes"] or file_sha256(path) != entry["sha256"]:
                raise ValueError("dataset source failed its locked size/SHA-256 check: " + entry["id"])
        from benchmarks.core_adapters import tasks
        task = next(tasks(args.dataset, limit=1, chunk_bytes=args.chunk_bytes, question_limit=1))
    elif args.task:
        task = json.loads(args.task.read_text(encoding="utf-8-sig"))
    else:
        task = {"schema": "sic-research-task-v2", "task_id": "synthetic-mechanical-only",
                "goal": "Preserve evidence and choose a supported next action.",
                "blocks": ["Fact: server is on port 7193.", "Suggestion: inspect the configuration."],
                "references": {"answer": "7193"}, "scoring": {"kind": "normalized_exact", "official_score": False}}
    if task.get("media_required") or task.get("question_image"):
        raise ValueError("Rust v2 media_not_implemented: do not flatten screenshot-bearing tasks")
    if not isinstance(task.get("goal"), str) or not task.get("blocks") or any(not isinstance(b, str) or not b for b in task["blocks"]):
        raise ValueError("a text goal and nonempty text blocks are required")
    task.setdefault("final_question", "SIC_RESEARCH_FINAL_QUESTION\nAnswer using retained context.\n" + task["goal"])
    if not isinstance(task["final_question"], str) or not task["final_question"]:
        raise ValueError("final question must be nonempty text")
    return task


def _controls():
    spec = importlib.util.spec_from_file_location("sic_research_control_pack", PACK / "controls.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def run_engine(client, task, engine, pack, budget, trim_budget, evidence_dir, worker_call_limit=1, hierarchy_fanout=0):
    chain = client.call("chain.create")["chain_id"]
    prompt = pack["prompts"]["structured_summary" if engine == "structured_summary" else "sic_editor"]
    # Rust's Editor requires the operation envelope as one tool call.
    prompt += "\nSubmit the ops and pvi object using exactly one sic_edit_blocks tool call."
    client.call("config.update", {"task_goal": task["goal"], "editor_prompt": prompt,
                "schedule_policy": "wait", "editor_feedback": {"max_corrections": 0, "feedback_chars": 8192}}, chain)
    edits, blocks, worker_steps = [], [], []

    def seal():
        pending = client.call("chain.get", chain_id=chain)["contexts"]["main"]["levels"]["1"]["pending"]
        if not pending:
            return None
        wb = client.call("wb.form", {"source_ids": pending, "reason": "rust-research-" + engine}, chain)
        source = wb["source_id"]
        if engine in {"sic", "batch", "structured_summary"}:
            job = client.call("edit.prepare", {"source_id": source}, chain)
            budget.take()
            outcome = client.call("edit.run", {"job_id": job["job_id"]}, chain, True)
            if outcome.get("status") != "ready" or outcome.get("attempts") != 1:
                raise ValueError("Editor did not complete in one attempt")
        elif engine in {"program_exact", "unchanged"}:
            rules = ["deduplicate_exact", "drop_empty", "normalize_newlines"] if engine == "program_exact" else []
            job = client.call("edit.program", {"source_id": source, "rules": rules}, chain)
        else:
            job = client.call("edit.prepare", {"source_id": source}, chain)
            units = job["editable_units"]
            if engine == "llmlingua":
                from llmlingua_adapter import compress_units
                ops = compress_units(units, evidence_dir / engine / "compressor-evidence")["ops"]
            else:
                ops = _controls().unit_ops(units, engine, trim_budget)
            assignments = job["job"]["assignments"]
            if len(assignments) != 1:
                raise ValueError("research control requires one explicit Editor assignment")
            client.call("edit.submit", {"job_id": job["job_id"], "editor": assignments[0]["editor"],
                        "ops": ops, "pvi": {}}, chain)
        material = client.call("edit.validate", {"job_id": job["job_id"]}, chain)["material"]
        cut = client.call("cut.publish", {"job_id": job["job_id"],
                          "expected_cut": client.call("chain.get", chain_id=chain)["cut_seq"],
                          "idempotency_key": "research-cut-" + str(len(edits))}, chain)
        edits.append({"job_id": job["job_id"], "wb_id": wb["wb_id"], "level": cut["level"],
                      "source_text": "\n".join(unit["text"] for unit in job["editable_units"]),
                      "candidate": material, "publication": {"cb_id": cut["cb_id"]}})
        edits.extend(maintain_hierarchy(client, chain, hierarchy_fanout))
        return cut["cb_id"]

    for index, text in enumerate(task["blocks"]):
        messages = (task.get("protected", []) if index == 0 else []) + [{"role": "user", "content": text}]
        source = client.call("source.ingest", {"request": {"messages": messages},
                             "idempotency_key": f"research-source-{index}"}, chain)["source_id"]
        item = {"source_id": source}
        if engine == "sic":
            item["cb_id"] = seal()
        blocks.append(item)
        worker, attempts = run_bounded_worker(client, chain, budget, worker_call_limit)
        worker_steps.append({"answer": worker["content"], "attempt_id": worker["attempt_id"], "attempt_ids": attempts})
    seal()
    before = client.call("cut.get", chain_id=chain)
    client.call("source.ingest", {"request": {"messages": [{"role": "user", "content": task["final_question"]}]},
                "idempotency_key": "research-final-question"}, chain)
    final, final_attempts = run_bounded_worker(client, chain, budget, worker_call_limit)
    evidence = client.call("experiment.pack", {}, chain)
    checked = client.call("experiment.verify", {"pack": evidence}, chain)
    if not checked.get("ok") or not checked.get("cut_seq", {}).get("matches"):
        raise ValueError("Rust experiment pack verification failed")
    return {"schema": "sic-rust-v2-research-arm-v1", "engine": engine, "arm": engine, "status": "completed",
            "chain_id": chain, "blocks": blocks, "edits": edits, "worker_steps": worker_steps,
            "final_answer": final["content"], "worker_attempt_id": final["attempt_id"], "worker_attempt_ids": final_attempts,
            "active_cb_ids_before_final": [cb["cb_id"] for cb in before["view"]["cbs"]],
            "endpoint_policy": "retain-final-question-and-final-answer-unedited; no post-answer compression",
            "pack": evidence, "pack_verification": checked,
            "accounting": client.call("accounting.get", {}, chain),
            "quality": {"status": "not_evaluated", "score": None}}


def run(args, task_override=None):
    if not args.rust_binary and not getattr(args,'plan_only',False):
        raise ValueError("Rust research requires --rust-binary")
    if args.hierarchy_fanout != 0 and not 2 <= args.hierarchy_fanout <= 64:
        raise ValueError("hierarchy fanout must be 0 or an integer from 2 to 64")
    prices = validate_prices(json.loads(args.prices.read_text(encoding="utf-8-sig"))) if args.prices else None
    if args.allow_provider != bool(args.provider_config):
        raise ValueError("real Provider execution requires both --provider-config and --allow-provider")
    if args.allow_test_gateway and not args.allow_provider:
        raise ValueError("test gateway opt-in requires a Provider configuration")
    task = load_task(args, task_override)
    worker_call_limit = getattr(args, "worker_call_limit", 1)
    estimate = planned_calls(len(task["blocks"]), args.engines, worker_call_limit)
    if getattr(args,'plan_only',False):
        if args.output.exists():raise ValueError('output must be a new directory')
        args.output.mkdir(parents=True)
        plan={'backend':'rust-v2','task_id':task.get('task_id'),'blocks':len(task['blocks']),
              'source_bytes':sum(len(block.encode('utf-8')) for block in task['blocks']),
              'engines':args.engines,'model_calls':estimate,'call_estimate_kind':'maximum host-dispatched model steps',
              'worker_call_limit':worker_call_limit,'hierarchy_fanout':args.hierarchy_fanout,
              'executed':False,'research_evaluation':'not_evaluated'}
        save(args.output/'plan.json',plan)
        print(json.dumps(plan,ensure_ascii=False))
        return 0
    if estimate["total"] > args.max_model_calls:
        raise ValueError("planned model calls exceed --max-model-calls")
    config = None
    if args.provider_config:
        config = json.loads(args.provider_config.read_text(encoding="utf-8-sig"))
        worker, editor = validate_profile_config(config)
        for profile in (worker, editor, *config.get("profiles", {}).values()):
            key_env = profile.get("api_key_env")
            if not isinstance(key_env, str) or not os.environ.get(key_env):
                raise ValueError("each Rust profile requires its credential environment variable")
            if urlsplit(profile.get("api_base", "")).port == 8341 and not args.allow_test_gateway:
                raise ValueError("8341 requires explicit --allow-test-gateway")
    if args.output.exists():
        raise ValueError("output must be a new directory")
    args.output.mkdir(parents=True)
    save(args.output / "task.json", task)
    plan = {"backend": "rust-v2", "engines": args.engines, "model_calls": estimate,
            "worker_call_limit": worker_call_limit, "hierarchy_fanout": args.hierarchy_fanout,
            "call_estimate_kind": "maximum host-dispatched model steps",
            "per_question_chain": True, "research_evaluation": "not_evaluated"}
    save(args.output / "plan.json", plan)
    pack = json.loads((PACK / "control-pack.json").read_text(encoding="utf-8"))
    fixture_context = MockProvider() if config is None else nullcontext(None)
    records, budget = [], Budget(args.max_model_calls)
    model_path = args.output / "rust-model-config.json"
    snapshot = args.output / "code-snapshot.zip"
    files = ["scripts/run_compression_research.py", "scripts/rust_v2_research.py", "scripts/rust_v2_dataset_runner.py",
             "scripts/rust_v2_test_runtime.py", "scripts/score_compression_research.py", "scripts/research_scoring.py",
             "scripts/run_v2_experiment.py", "scripts/verify_v2_experiment.py", "scripts/score_v2_experiment.py",
             "scripts/llmlingua_adapter.py", "scripts/llmlingua_worker.py", "scripts/dataset_adapters.py",
             "scripts/verify_dataset_run.py", "scripts/benchmark_scoring.py", "benchmarks/core_adapters.py", "benchmarks/source_paths.py",
             "benchmarks/sources-lock.json", "sic_core/accounting.py", "research/compression/pack/control-pack.json",
             "research/compression/pack/controls.py", "research/compression/pack/rubric.json",
             "research/compression/pack/llmlingua-model.lock.json"]
    with zipfile.ZipFile(snapshot, "w", zipfile.ZIP_DEFLATED) as archive:
        for name in files:
            archive.write(ROOT / name, name)
    manifest = {"schema": "sic-rust-v2-research-run-v1", "backend": "rust-v2", "status": "starting",
                "mode": "fixed_provider" if config else "owned_fixture", "research_evaluation": "not_evaluated",
                "quality_claim": False, "binary_sha256": file_sha256(args.rust_binary),
                "code_snapshot_sha256": file_sha256(snapshot), "max_model_calls": args.max_model_calls,
                "created_at_utc": datetime.now(timezone.utc).isoformat(),
                "route_status": "nonfixed-exploratory" if args.allow_test_gateway else "declared-profile" if config else "owned-fixture"}
    try:
        with fixture_context as fixture:
            if fixture:
                config = mock_profile(fixture.base)
            save(model_path, config)
            manifest["model_config_sha256"] = file_sha256(model_path)
            manifest["status"] = "running"
            save(args.output / "manifest.json", manifest)
            save(args.output / "configuration.json", {"backend": "rust-v2", "engines": args.engines,
                 "mode": manifest["mode"], "task_goal_visible_during_edit": True,
                 "pack_sha256": file_sha256(PACK / "control-pack.json"), "model_calls": estimate,
                 "hierarchy_fanout": args.hierarchy_fanout, "hierarchy_editing": "deterministic deduplicate_exact",
                 "pricing": "host-declared estimate" if prices is not None else "not configured",
                 "quality": "not_evaluated"})
            if prices is not None:
                save(args.output / "prices.json", prices)
            extra_env = {"SIC_RUST_EVAL_MOCK_KEY": "fixture"} if fixture else {}
            with OwnedRustCore(args.rust_binary, args.output, model_path, extra_env) as host:
                for engine in args.engines:
                    try:
                        record = run_engine(host.client, task, engine, pack, budget, args.trim_budget, args.output, worker_call_limit, args.hierarchy_fanout)
                        if prices is not None:
                            record["price_estimate"] = price_estimate(record["accounting"], prices)
                    except Exception as error:
                        message = type(error).__name__ + ": " + str(error)
                        for profile in (*config.get("profiles", {}).values(), *[value for value in config.get("roles", {}).values() if isinstance(value,dict)]):
                            secret = os.environ.get(profile.get("api_key_env", ""))
                            if secret:message = message.replace(secret, "[redacted]")
                        record = {"engine": engine, "arm": engine, "status": "failed", "error": message}
                    records.append(record)
                    folder = args.output / engine
                    folder.mkdir(exist_ok=True)
                    save(folder / "result.json", record)
                    save(args.output / "records.json", records)
                    if record["status"] != "completed":
                        break
                snapshot_db = args.output / "rust-core-snapshot.sqlite"
                with closing(sqlite3.connect((args.output / "rust-core.sqlite").resolve().as_uri() + "?mode=ro", uri=True)) as source:
                    with closing(sqlite3.connect(str(snapshot_db))) as destination:
                        source.backup(destination)
                        destination.execute("PRAGMA journal_mode=DELETE")
            manifest["database_snapshot"] = "rust-core-snapshot.sqlite"
            manifest["status"] = "complete" if len(records) == len(args.engines) and all(r["status"] == "completed" for r in records) else "completed-with-failures"
            manifest["model_steps_dispatched"] = budget.used
            manifest["finished_at_utc"] = datetime.now(timezone.utc).isoformat()
            manifest["files"] = {p.relative_to(args.output).as_posix(): {"sha256": file_sha256(p), "bytes": p.stat().st_size}
                                 for p in sorted(args.output.rglob("*")) if p.is_file() and p.name not in {"manifest.json", "checksums.json"}
                                 and not p.name.startswith("rust-core.sqlite")}
            save(args.output / "manifest.json", manifest)
    finally:
        checksums = {p.name: file_sha256(p) for p in sorted(args.output.iterdir())
                     if p.is_file() and p.name != "checksums.json" and not p.name.startswith("rust-core.sqlite")
                     and p.suffix in (".json", ".log", ".sqlite", ".zip")}
        save(args.output / "checksums.json", checksums)
    print(json.dumps({"backend": "rust-v2", "status": manifest["status"], "engines": [r["engine"] for r in records],
                      "model_steps_dispatched": budget.used, "quality": "not_evaluated"}))
    return 0 if manifest["status"] == "complete" else 1


def verify(directory):
    """Independently verify artifacts and the final Rust Worker CB plans."""
    from verify_dataset_run import verify_rust_snapshot
    root = Path(directory).resolve()
    read = lambda name: json.loads((root / name).read_text(encoding="utf-8"))
    manifest, checksums = read("manifest.json"), read("checksums.json")
    if manifest.get("schema") != "sic-rust-v2-research-run-v1" or manifest.get("backend") != "rust-v2":
        raise ValueError("unsupported Rust research artifact")
    for name, expected in checksums.items():
        if Path(name).name != name or file_sha256(root / name) != expected:
            raise ValueError("root artifact digest mismatch: " + name)
    for name, expected in manifest.get("files", {}).items():
        relative = PurePosixPath(name)
        if relative.is_absolute() or ".." in relative.parts or ":" in name or "\\" in name:
            raise ValueError("unsafe research artifact path")
        path = (root / name).resolve()
        path.relative_to(root)
        if not path.is_file() or path.stat().st_size != expected["bytes"] or file_sha256(path) != expected["sha256"]:
            raise ValueError("research artifact digest mismatch: " + name)
    required = {"task.json", "configuration.json", "records.json", "code-snapshot.zip", "rust-core-snapshot.sqlite"}
    if not required.issubset(manifest.get("files", {})):
        raise ValueError("incomplete Rust research evidence")
    if file_sha256(root / "code-snapshot.zip") != manifest.get("code_snapshot_sha256"):
        raise ValueError("research code snapshot mismatch")
    config, task, plan, records = read("configuration.json"), read("task.json"), read("plan.json"), read("records.json")
    with zipfile.ZipFile(root / "code-snapshot.zip") as archive:
        if archive.testzip() is not None:
            raise ValueError("research code snapshot CRC failure")
        if hashlib.sha256(archive.read("research/compression/pack/control-pack.json")).hexdigest() != config["pack_sha256"]:
            raise ValueError("research control pack mismatch")
        model_lock = json.loads(archive.read("research/compression/pack/llmlingua-model.lock.json"))
    verify_rust_snapshot(root, checksums, plan, manifest, records)
    if manifest.get("status") != "complete":
        return {"status": "valid-failure-artifact", "backend": "rust-v2", "quality": "not_evaluated"}
    if [r["engine"] for r in records] != config["engines"] or any(r["status"] != "completed" for r in records):
        raise ValueError("not every planned Rust control completed")
    if manifest["model_steps_dispatched"] > plan["model_calls"]["total"]:
        raise ValueError("actual Rust model steps exceed the planned maximum")
    actual_steps = 0
    for record in records:
        if read(record["engine"] + "/result.json") != record:
            raise ValueError("per-engine result differs from the run record")
        if len(record["worker_steps"]) != len(task["blocks"]):
            raise ValueError("missing intermediate Rust Worker calls")
        if any(not step.get("attempt_ids") or len(step["attempt_ids"]) > plan.get("worker_call_limit",1)
               or step["attempt_ids"][-1] != step.get("attempt_id") for step in record["worker_steps"]):
            raise ValueError("invalid intermediate Rust Worker attempt sequence")
        actual_steps += sum(len(step.get("attempt_ids", [])) for step in record["worker_steps"])
        actual_steps += len(record.get("worker_attempt_ids", []))
        if record["engine"] in {"sic", "batch", "structured_summary"}:
            actual_steps += sum(edit["level"] == 1 for edit in record["edits"])
        expected_edits = len(task["blocks"]) + 1 if record["engine"] == "sic" else 1
        if sum(edit["level"] == 1 for edit in record["edits"]) != expected_edits:
            raise ValueError("control did not use its declared editing schedule")
        if record["engine"] == "llmlingua":
            paths = [name for name in manifest["files"] if name.startswith("llmlingua/compressor-evidence/llmlingua-") and name.endswith(".json")]
            if len(paths) != expected_edits:
                raise ValueError("missing local LLMLingua inference evidence")
            expected_weight = next(item["sha256"] for item in model_lock["files"] if item["path"] == "model.safetensors")
            for path in paths:
                evidence = read(path)
                if (evidence.get("weight_sha256") != expected_weight or not evidence.get("classifier_tensor_matches_weight_file") or
                        evidence.get("model_forward_calls", 0) < 1 or not evidence.get("network_connections_blocked")):
                    raise ValueError("local LLMLingua weight or inference evidence is invalid")
        if record["quality"]["status"] != "not_evaluated":
            raise ValueError("runner cannot grade its own research quality")
        if config.get("pricing") == "host-declared estimate":
            if "prices.json" not in manifest["files"] or record.get("price_estimate") != price_estimate(record["accounting"], validate_prices(read("prices.json"))):
                raise ValueError("Rust host price estimate cannot be independently recomputed")
    if actual_steps != manifest["model_steps_dispatched"]:
        raise ValueError("recorded Rust attempts do not match the model-step count")
    with closing(sqlite3.connect((root / manifest["database_snapshot"]).resolve().as_uri() + "?mode=ro", uri=True)) as db:
        for record in records:
            state = json.loads(db.execute("SELECT state FROM chains WHERE id=?", (record["chain_id"],)).fetchone()[0])
            for edit in record["edits"]:
                job = state.get("jobs", {}).get(edit["job_id"])
                if (not job or job.get("status") != "published" or job.get("cb_id") != edit["publication"]["cb_id"] or
                        "\n".join(unit["text"] for unit in job["source_snapshot"]["units"]) != edit["source_text"]):
                    raise ValueError("research edit does not match its published Rust job")
                cb_row = db.execute("SELECT data FROM objects WHERE id=? AND kind='cb'", (job["cb_id"],)).fetchone()
                if not cb_row or json.loads(cb_row[0]).get("body") != edit["candidate"].get("body"):
                    raise ValueError("research candidate differs from the published CB")
            for step in record["worker_steps"]:
                for index, attempt_id in enumerate(step["attempt_ids"]):
                    row = db.execute("SELECT data,digest FROM objects WHERE id=? AND kind='worker_attempt'", (attempt_id,)).fetchone()
                    if not row or hashlib.sha256(row[0].encode()).hexdigest() != row[1]:
                        raise ValueError("intermediate Rust Worker attempt is missing or changed")
                    attempt = json.loads(row[0])
                    run_row = db.execute("SELECT data,digest FROM objects WHERE id=? AND kind='worker_run'", (attempt["worker_run_id"],)).fetchone()
                    if not run_row or hashlib.sha256(run_row[0].encode()).hexdigest() != run_row[1] or json.loads(run_row[0])["plan"]["data"]["chain_id"] != record["chain_id"]:
                        raise ValueError("intermediate Worker belongs to a different Rust chain")
                    if index == len(step["attempt_ids"]) - 1:
                        choices = (attempt.get("raw_response") or {}).get("choices", [])
                        if len(choices) != 1 or choices[0].get("message", {}).get("content") != step["answer"]:
                            raise ValueError("intermediate Worker answer differs from its Rust attempt")
    return {"status": "passed", "backend": "rust-v2", "files_checked": len(manifest["files"]),
            "rust_snapshot_and_final_worker_plans_verified": True, "model_quality_inferred": False}


def score(directory, annotations=None):
    from research_scoring import score_answer, score_annotations, score_task_rubric
    root = Path(directory)
    valid = verify(root)
    if valid["status"] != "passed":
        return {"schema": "sic-rust-v2-research-score-v1", "artifact_validation": valid,
                "quality": {"status": "not_evaluated"}}
    read = lambda name: json.loads((root / name).read_text(encoding="utf-8"))
    manifest, task, records = read("manifest.json"), read("task.json"), read("records.json")
    for item in (annotations or {}).values():
        for criterion in item.get("criteria", []):
            for evidence in criterion.get("evidence", []):
                if evidence.get("artifact") not in manifest["files"] or not evidence.get("locator"):
                    raise ValueError("task annotation must cite a verified artifact and locator")
    fixture = manifest["mode"] == "owned_fixture"
    results = {}
    for record in records:
        engine, answer = record["engine"], record["final_answer"]
        kind = task.get("scoring", {}).get("kind", "human_rubric")
        quality = score_answer(answer, task.get("references", {}).get("answer"), kind, fixture)
        if kind != "normalized_exact":
            quality = score_task_rubric(answer, (annotations or {}).get(engine + ":task"), fixture)
        noise = []
        for index, edit in enumerate(record["edits"]):
            candidate = edit["candidate"].get("body", "")
            noise.append({"edit": index, "level": edit["level"],
                "assessment": score_annotations(edit["source_text"], candidate,
                                                 (annotations or {}).get(engine + ":" + str(index)), fixture)})
        results[engine] = {"quality": quality, "editing_noise": noise}
    return {"schema": "sic-rust-v2-research-score-v1", "artifact_validation": valid, "arms": results,
            "research_evaluation": "not_evaluated" if fixture or all(v["quality"]["status"] == "not_evaluated" for v in results.values()) else "partial-independent-scoring",
            "model_calls": 0, "official_benchmark_score": False}
