"""Read-only independent validation of a new-core run. No model or network calls."""
import argparse
import hashlib
import json
from pathlib import Path, PurePosixPath
import sys
import zipfile

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from sic_core.accounting import summarize_records, compare_workflows


def read(path):
    return json.loads(path.read_text(encoding="utf-8"))


def validate_run(directory):
    directory = Path(directory).resolve()
    manifest = read(directory / "manifest.json")
    if manifest.get("schema") != "sic-new-experiment-v1":
        raise ValueError("Unsupported run schema")
    checked = 0
    for name, entry in manifest["files"].items():
        rel = PurePosixPath(name)
        if rel.is_absolute() or ".." in rel.parts or "\\" in name or ":" in name:
            raise ValueError("Manifest paths must be portable relative paths")
        path = (directory / name).resolve()
        path.relative_to(directory)
        if not path.is_file() or path.stat().st_size != entry["bytes"] or hashlib.sha256(path.read_bytes()).hexdigest() != entry["sha256"]:
            raise ValueError("Artifact integrity mismatch: " + name)
        checked += 1
    required = {"task.json", "frozen-configuration.json", "code-snapshot.zip"}
    if not required <= manifest["files"].keys():
        raise ValueError("Missing frozen inputs/code snapshot")
    with zipfile.ZipFile(directory / "code-snapshot.zip") as archive:
        if archive.testzip() is not None:
            raise ValueError("Code snapshot CRC mismatch")
        if not {"scripts/run_v2_experiment.py", "scripts/verify_v2_experiment.py", "scripts/score_v2_experiment.py", "sic_core/accounting.py"} <= set(archive.namelist()):
            raise ValueError("Incomplete reproducibility snapshot")
    if manifest["status"] != "passed":
        return {"status": "valid-failure-artifact", "files_checked": checked, "quality": "not_evaluated"}
    task = read(directory / "task.json")
    configuration = read(directory / "frozen-configuration.json")
    if manifest["mode"] != "owned_http_fixture" or configuration["mode"] != "owned_http_fixture" or manifest["real_provider_calls"] != 0:
        raise ValueError("This validator accepts only this runner's owned fixture mode")
    summaries = {}
    role_counts = {}
    for arm in ("baseline", "sic"):
        for filename in ("ledger.json", "summary.json", "trace.json", "model-http.json", "result.json"):
            if arm + "/" + filename not in manifest["files"]:
                raise ValueError("Incomplete arm evidence")
        ledger, stored, result = [read(directory / arm / name) for name in ("ledger.json", "summary.json", "result.json")]
        summary = summarize_records(ledger["calls"], ledger["resources"], ledger["task_interval"], ledger["prices"], ledger["events"])
        if summary != stored:
            raise ValueError("Independent accounting differs: " + arm)
        if ledger["prices"] != configuration["prices"] or result["task_id"] != task["task_id"]:
            raise ValueError("Frozen task/price mismatch")
        workers = sum(call["role"] == "worker" for call in ledger["calls"])
        editors = sum(call["role"] == "editor" for call in ledger["calls"])
        if workers != len(task["blocks"]) or editors != (1 if arm == "baseline" else workers + 1):
            raise ValueError("Workflows did not execute the prescribed Worker/editor counts")
        if len(result["fixed_cbs"]) != editors or result["pending"]:
            raise ValueError("Final published/pending state mismatch")
        if result["quality"] != {"status": "not_evaluated", "score": None}:
            raise ValueError("Fixture quality must remain unscored")
        publications = [item["response"]["result"] for item in read(directory / arm / "trace.json") if item["operation"] == "cut.get"]
        for publication in publications:
            prefix = publication["view"]["cbs"]
            if prefix != result["fixed_cbs"][:len(prefix)]:
                raise ValueError("Published CB/operation history was rewritten")
        model_http = read(directory / arm / "model-http.json")
        if len(model_http) != workers + editors:
            raise ValueError("Owned HTTP call evidence count mismatch")
        summaries[arm] = summary
        role_counts[arm] = {"worker": workers, "editor": editors}
    stored_comparison = read(directory / "comparison.json")
    independent = compare_workflows(summaries["baseline"], summaries["sic"])
    for key in ("baseline", "sic", "delta_cost", "delta_wall_seconds"):
        if stored_comparison.get(key) != independent.get(key):
            raise ValueError("Independent paired comparison differs: " + key)
    if stored_comparison["quality"]["status"] != "not_evaluated":
        raise ValueError("No quality score may be attached to this fixture")
    return {"status": "passed", "files_checked": checked, "role_counts": role_counts,
            "accounting_recomputed": True, "published_history_unchanged": True, "real_provider_calls": 0, "quality": "not_evaluated"}


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("run", type=Path)
    args = parser.parse_args()
    print(json.dumps(validate_run(args.run), ensure_ascii=False))


if __name__ == "__main__":
    main()
