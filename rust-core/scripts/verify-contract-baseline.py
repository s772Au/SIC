"""Check local SIC source hashes and feature IDs, not implementation acceptance.

The author's draft may be an ignored/private local file. Missing source material
is reported as an error instead of silently substituting an older document.
"""

import hashlib
import json
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
CONTRACT = ROOT / "paper/review/sic_implementation_contract.md"
MATRIX = ROOT / "paper/review/sic_feature_acceptance_matrix.json"


def main():
    errors = []
    contract = CONTRACT.read_text(encoding="utf-8-sig")
    matrix = json.loads(MATRIX.read_text(encoding="utf-8-sig"))
    for source in matrix["sources"]:
        if source.get("id") not in {"D", "P"}:
            continue
        path = ROOT / source["path"]
        if not path.is_file():
            errors.append(f"missing source {source['id']}: {source['path']}")
            continue
        digest = hashlib.sha256(path.read_bytes()).hexdigest()
        if digest != source["sha256"] or digest not in contract:
            errors.append(f"source hash mismatch: {source['id']}")

    feature_ids = [feature["id"] for feature in matrix["features"]]
    headings = re.findall(r"^### (F\d+) ", contract, re.MULTILINE)
    group_ids = [feature for group in matrix["groups"] for feature in group["features"]]
    expected = {f"F{i:02}" for i in range(1, 74)}
    for label, ids in (("matrix", feature_ids), ("contract", headings), ("groups", group_ids)):
        if len(ids) != len(set(ids)) or set(ids) != expected:
            errors.append(f"{label} feature IDs are incomplete, duplicated, or unexpected")
    if "shared_prefix_requirements" not in matrix or "T-COST-SHARED-PREFIX" not in contract:
        errors.append("shared-prefix requirements or fixture reference missing")

    print(json.dumps({"ok": not errors, "feature_count": len(feature_ids), "errors": errors}, ensure_ascii=False))
    return int(bool(errors))


if __name__ == "__main__":
    raise SystemExit(main())
