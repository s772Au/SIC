"""Verify artifact digests and recompute synthetic exact-match scores without APIs."""
import argparse
import json
from pathlib import Path
from run_experiment import digest, score


def verify(directory):
    checksums = json.loads((directory / "checksums.json").read_text(encoding="utf-8"))
    for name, expected in checksums.items():
        if Path(name).name != name or digest((directory / name).read_bytes()) != expected:
            raise ValueError("artifact digest mismatch: " + name)
    for required in ("manifest.json", "cases.json", "records.json", "scores.json", "completion.json"):
        if required not in checksums:
            raise ValueError("missing checksummed artifact: " + required)
    cases = json.loads((directory / "cases.json").read_text(encoding="utf-8"))["cases"]
    records = json.loads((directory / "records.json").read_text(encoding="utf-8"))
    expected = {(c["id"], arm): c["expected"] for c in cases for arm in ("baseline", "sic")}
    seen = set()
    for item in records:
        key = item["case_id"], item["arm"]
        if key in seen or key not in expected or item["expected"] != expected[key]:
            raise ValueError("duplicate, unknown or altered case/arm")
        seen.add(key)
    if seen != set(expected):
        raise ValueError("missing case/arm records")
    computed = {arm: score([r for r in records if r["arm"] == arm]) for arm in ("baseline", "sic")}
    stored = json.loads((directory / "scores.json").read_text(encoding="utf-8"))
    if computed != stored:
        raise ValueError("stored score differs from recomputed score")
    return computed


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("run", type=Path)
    args = parser.parse_args()
    print(json.dumps(verify(args.run), ensure_ascii=False, indent=2))
