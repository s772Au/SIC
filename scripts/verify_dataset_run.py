"""Recompute scores from a completed dataset artifact, with no network requests."""
import argparse
from contextlib import closing
import hashlib
import json
from pathlib import Path
import sqlite3
from benchmark_scoring import aggregate, babilong_score, parse_judgment
from dataset_adapters import file_sha256


def verify_rust_snapshot(directory, checksums, plan, manifest, records):
    if plan.get("backend") != "rust-v2" or manifest.get("research_evaluation") != "not_evaluated":
        raise ValueError("invalid Rust v2 evaluation scope")
    snapshot = manifest.get("database_snapshot")
    if snapshot != "rust-core-snapshot.sqlite" or snapshot not in checksums:
        raise ValueError("missing consistent Rust database snapshot")
    if "rust-model-config.json" not in checksums or file_sha256(directory / "rust-model-config.json") != manifest.get("model_config_sha256"):
        raise ValueError("Rust model configuration digest mismatch")
    if manifest.get("model_steps_dispatched", 0) > manifest.get("max_model_calls", -1):
        raise ValueError("model call limit exceeded")
    chains = set()
    uri = (directory / snapshot).resolve().as_uri() + "?mode=ro"
    with closing(sqlite3.connect(uri, uri=True)) as db:
        if db.execute("PRAGMA quick_check").fetchone()[0] != "ok":
            raise ValueError("Rust database snapshot failed integrity check")
        for record in records:
            if record.get("status") not in {"scored", "completed"}:
                continue
            chain = record.get("chain_id")
            if not chain or chain in chains:
                raise ValueError("Rust case/question/arm chains must be distinct")
            chains.add(chain)
            pack = record.get("pack", {})
            verification = record.get("pack_verification", {})
            if (pack.get("chain", {}).get("chain_id") != chain or not verification.get("ok") or
                    not verification.get("cut_seq", {}).get("matches")):
                raise ValueError("missing valid Rust Core pack verification")
            row = db.execute("SELECT state FROM chains WHERE id=?", (chain,)).fetchone()
            if not row or json.loads(row[0]).get("cut_seq") != pack["chain"]["cut_seq"]:
                raise ValueError("Rust chain is missing or changed in the database snapshot")
            for field, kind, id_key in (("sources", "source", "source_id"), ("blocks", "cb", "cb_id")):
                for entry in pack.get(field, []):
                    row = db.execute("SELECT data,digest FROM objects WHERE id=? AND kind=?", (entry[id_key], kind)).fetchone()
                    if not row or hashlib.sha256(row[0].encode("utf-8")).hexdigest() != row[1] or row[1] != entry["digest"]:
                        raise ValueError("Rust stored object digest mismatch")
            source_ids = {entry["source_id"] for entry in pack.get("sources", [])}
            cb_ids = {entry["cb_id"] for entry in pack.get("blocks", [])}
            for action in record.get("hierarchy_actions", []):
                row = db.execute("SELECT data FROM objects WHERE id=? AND kind='cb'", (action.get("cb_id"),)).fetchone()
                if not row or action["cb_id"] not in cb_ids:
                    raise ValueError("hierarchy CB is missing from the Rust pack")
                cb = json.loads(row[0])
                if (cb.get("level") != action.get("level") or cb.get("members") != action.get("members") or
                        cb.get("job_id") != action.get("job_id") or cb.get("wb_id") != action.get("wb_id")):
                    raise ValueError("hierarchy evidence does not match the stored Rust CB")
            if not all(block.get("source_id") in source_ids and
                       (record["arm"] != "sic" or block.get("cb_id") in cb_ids)
                       for block in record.get("blocks", [])):
                raise ValueError("recorded Rust source or CB is missing from its pack")
            if not record.get("blocks") or not record.get("worker_attempt_id"):
                raise ValueError("missing Rust source or Worker attempt evidence")
            attempt_ids = record.get("worker_attempt_ids", [record["worker_attempt_id"]])
            if (not attempt_ids or attempt_ids[-1] != record["worker_attempt_id"] or
                    len(attempt_ids) != len(set(attempt_ids)) or
                    len(attempt_ids) > plan.get("worker_call_limit", 1)):
                raise ValueError("invalid bounded Rust Worker attempt sequence")
            for attempt_id in attempt_ids:
                item = db.execute("SELECT data,digest FROM objects WHERE id=? AND kind='worker_attempt'", (attempt_id,)).fetchone()
                if not item or hashlib.sha256(item[0].encode("utf-8")).hexdigest() != item[1]:
                    raise ValueError("missing or changed Rust Worker attempt")
            row = db.execute("SELECT data,digest FROM objects WHERE id=? AND kind='worker_attempt'",
                             (record["worker_attempt_id"],)).fetchone()
            if not row or hashlib.sha256(row[0].encode("utf-8")).hexdigest() != row[1]:
                raise ValueError("missing or changed Rust Worker attempt")
            attempt = json.loads(row[0])
            choices = (attempt.get("raw_response") or {}).get("choices", [])
            answer = record.get("answer", record.get("final_answer"))
            if (attempt.get("error") is not None or attempt.get("stale") or attempt.get("interrupted") or
                    attempt.get("input_changed") or attempt.get("cut_seq") != pack["chain"]["cut_seq"] or
                    len(choices) != 1 or choices[0].get("message", {}).get("content") != answer):
                raise ValueError("recorded answer does not match the Rust Worker attempt")
            row = db.execute("SELECT data,digest FROM objects WHERE id=? AND kind='worker_run'",
                             (attempt.get("worker_run_id"),)).fetchone()
            if not row or hashlib.sha256(row[0].encode("utf-8")).hexdigest() != row[1]:
                raise ValueError("missing or changed Rust Worker run")
            worker_plan = json.loads(row[0])["plan"]["data"]
            if (worker_plan.get("chain_id") != chain or
                    worker_plan.get("cb_ids") != record.get("active_cb_ids_before_final")):
                raise ValueError("final Worker plan did not use the recorded CB view")


def verify(directory):
    checksums = json.loads((directory / "checksums.json").read_text(encoding="utf-8"))
    required = {"plan.json", "manifest.json", "records.json", "scores.json", "completion.json"}
    if not required.issubset(checksums):
        raise ValueError("incomplete run artifact")
    for name, expected in checksums.items():
        if Path(name).name != name or file_sha256(directory / name) != expected:
            raise ValueError("artifact digest mismatch: " + name)
    plan = json.loads((directory / "plan.json").read_text(encoding="utf-8"))
    manifest = json.loads((directory / "manifest.json").read_text(encoding="utf-8"))
    if manifest.get("code_snapshot_sha256"):
        if "code-snapshot.zip" not in checksums or file_sha256(directory / "code-snapshot.zip") != manifest["code_snapshot_sha256"]:
            raise ValueError("code snapshot digest mismatch")
    records = json.loads((directory / "records.json").read_text(encoding="utf-8"))
    if manifest.get("schema") == "sic-rust-v2-dataset-run-v1":
        verify_rust_snapshot(directory, checksums, plan, manifest, records)
    questions = {(c["dataset"], c["id"], q["id"], arm): q for c in plan["cases"] for q in c["questions"] for arm in plan["arms"]}
    seen = set()
    for record in records:
        key = record["dataset"], record["case_id"], record["question_id"], record["arm"]
        if key in seen or key not in questions:
            raise ValueError("duplicate or unknown case/question/arm")
        seen.add(key)
        question = questions[key]
        if record["reference"] != question["reference"] or record["scorer"] != question["scorer"]:
            raise ValueError("reference or scorer changed")
        if record["status"] == "scored":
            if record["dataset"].startswith("babilong"):
                computed = babilong_score(question["reference"], record["answer"], question["question"], question["category"])
                if computed != record["score"]:
                    raise ValueError("BABILong score mismatch")
            else:
                judgments = record["score"]["judgments"]
                expected_count = len(question["rubric"]) if question["rubric"] is not None else 1
                if len(judgments) != expected_count:
                    raise ValueError("judge count mismatch")
                values = [parse_judgment(json.dumps(j)) for j in judgments]
                if sum(v["score"] for v in values) / len(values) != record["score"]["score"]:
                    raise ValueError("judge aggregate mismatch")
    if seen != set(questions):
        raise ValueError("missing planned case/question/arm record")
    computed = aggregate(records)
    if computed != json.loads((directory / "scores.json").read_text(encoding="utf-8")):
        raise ValueError("aggregate score mismatch")
    return computed


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("run", type=Path)
    args = parser.parse_args()
    print(json.dumps(verify(args.run), ensure_ascii=False, indent=2))
