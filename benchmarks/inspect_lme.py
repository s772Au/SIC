"""Verify LongMemEval-V2 text questions and trajectory IDs without images."""

import collections
import json
from pathlib import Path


ROOT = Path(__file__).resolve().parent
DATA = ROOT / "data" / "longmemeval-v2"


def main():
    report = {}
    for name in ("questions", "trajectories"):
        path = DATA / f"{name}.jsonl"
        domains = collections.Counter()
        ids = set()
        rows = 0
        images = 0
        with path.open("rb") as source:
            for line in source:
                item = json.loads(line)
                rows += 1
                if item["id"] in ids:
                    raise ValueError(f"duplicate {name} id {item['id']}")
                ids.add(item["id"])
                domains[item["domain"]] += 1
                images += bool(item.get("image"))
        report[name] = {"rows": rows, "uniqueIds": len(ids), "domains": dict(domains), "questionImages": images if name == "questions" else None}
    output = ROOT / "prepared" / "longmemeval-v2"
    output.mkdir(parents=True, exist_ok=True)
    (output / "text-inventory.json").write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(report, ensure_ascii=False))


if __name__ == "__main__":
    main()
