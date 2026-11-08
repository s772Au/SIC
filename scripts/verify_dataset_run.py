"""Recompute scores from a completed dataset artifact, with no network requests."""
import argparse
import json
from pathlib import Path
from benchmark_scoring import aggregate, babilong_score, parse_judgment
from dataset_adapters import file_sha256


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
