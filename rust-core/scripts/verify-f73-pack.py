#!/usr/bin/env python3
"""Verify the F73 preparation pack without network access."""

import hashlib
import json
import pathlib
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
PACK = ROOT / "research" / "f73"


def sha256(path: pathlib.Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main() -> int:
    adapter = json.loads((PACK / "adapter.json").read_text(encoding="utf-8"))
    sources = json.loads((PACK / "sources.json").read_text(encoding="utf-8"))
    problems = []
    for entry in adapter.get("sic_snapshot", []):
        path = ROOT / entry["path"]
        actual = sha256(path)
        expected = entry["sha256"]
        if actual != expected:
            problems.append({
                "path": entry["path"],
                "expected": expected,
                "actual": actual,
            })
    required = ("repo", "path", "commit", "git_blob_sha", "license", "url")
    for source in sources.get("sources", []):
        missing = [key for key in required if not source.get(key)]
        if missing:
            problems.append({"source": source.get("id"), "missing": missing})
    print(json.dumps({
        "ok": not problems,
        "sic_files": len(adapter.get("sic_snapshot", [])),
        "sources": len(sources.get("sources", [])),
        "problems": problems,
    }, indent=2, sort_keys=True))
    return 0 if not problems else 1


if __name__ == "__main__":
    sys.exit(main())
