"""Check mounted benchmark source files without contacting the model gateway."""

import argparse
import hashlib
import json
from pathlib import Path
from source_paths import resolve_source, public_source_path


ROOT = Path(__file__).resolve().parent


def sources():
    wanted = {"corpusqa-4m", "corpusqa-10m", "beam-10m-0", "beam-10m-1",
              "babilong-qa3-10m-first20-derived", "longmemeval-v2-trajectories"}
    return [entry for entry in json.loads((ROOT / "sources-lock.json").read_text(encoding="utf-8"))["files"]
            if entry["id"] in wanted]


def sha256(path):
    value = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(8 * 1024 * 1024), b""):
            value.update(chunk)
    return value.hexdigest()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--hash", action="store_true", help="read all bytes to verify SHA-256")
    parser.add_argument("--include-longmemeval", action="store_true", help="require the LME-V2 trajectory body")
    parser.add_argument("--all-locked", action="store_true", help="require every file in sources-lock.json, including full BABILong and LME images")
    args = parser.parse_args()
    results = []
    if args.all_locked:
        locked = json.loads((ROOT / "sources-lock.json").read_text(encoding="utf-8"))["files"]
        selected = locked
    else:
        selected = sources()
    for item in selected:
        if not args.all_locked and item["id"] == "longmemeval-v2-trajectories" and not args.include_longmemeval:
            continue
        path = resolve_source(item)
        exists = path.is_file()
        size_ok = exists and path.stat().st_size == item["bytes"]
        digest = sha256(path) if args.hash and size_ok else None
        hash_ok = None if not args.hash or not item["sha256"] else digest == item["sha256"]
        results.append({"dataset": item["id"], "path": public_source_path(item), "exists": exists, "sizeOk": size_ok, "hashOk": hash_ok})
    print(json.dumps(results, ensure_ascii=False, indent=2))
    if any(not x["sizeOk"] or x["hashOk"] is False for x in results):
        raise SystemExit(1)


if __name__ == "__main__":
    main()
