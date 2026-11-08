"""Independent scorer entry. Owned-fixture answers are always not_evaluated."""
import argparse
import json
from pathlib import Path
from verify_v2_experiment import validate_run, read


def score_run(directory):
    verified = validate_run(directory)
    manifest = read(Path(directory) / "manifest.json")
    if manifest.get("mode") == "owned_http_fixture":
        return {"schema": "sic-new-score-v1", "quality": {"status": "not_evaluated", "score": None,
                "reason": "Deterministic transport fixture is not a model/task quality experiment"},
                "artifact_validation": verified, "scorer": "fixture-no-quality-v1", "model_calls": 0}
    raise ValueError("No registered independent task rubric for this run mode; no fabricated default score")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("run", type=Path)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    result = score_run(args.run)
    if args.output:
        if args.output.exists():
            raise ValueError("Do not overwrite an existing score artifact")
        args.output.write_text(json.dumps(result, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(result, ensure_ascii=False))


if __name__ == "__main__":
    main()
