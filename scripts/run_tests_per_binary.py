#!/usr/bin/env python3
"""Run every test binary individually and report an honest total.

Why this exists: `cargo test --workspace` stops at the first binary that fails
to *execute*, so a Windows path that blocks one binary silently truncates the
run. On this machine `signal_core_dispatch` is refused by image-load policy
(os error 740), and the aggregate run stops there: 18 rust-core binaries and 4
experiments binaries never ran, and the reported total looked complete.

This script runs each test target on its own, records the verdict per binary, and
reports what actually executed. A blocked binary is reported as blocked, not as
a pass and not as a skip.
"""

import json
import os
import re
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


def targets(directory):
    if not os.path.isdir(directory):
        return []
    return sorted(f[:-3] for f in os.listdir(directory) if f.endswith(".rs"))


def run(cargo_args, name):
    proc = subprocess.run(
        ["cargo", "test", *cargo_args, "--test", name],
        cwd=ROOT, capture_output=True, text=True, encoding="utf-8", errors="replace",
    )
    output = proc.stdout + proc.stderr
    match = re.search(r"test result: (\w+)\. (\d+) passed; (\d+) failed", output)
    if match:
        return {"verdict": match.group(1),
                "passed": int(match.group(2)),
                "failed": int(match.group(3))}
    if "os error 740" in output or "never executed" in output:
        return {"blocked": "the OS refused to execute this binary at its own path"}
    tail = [line for line in output.splitlines() if line.strip()][-3:]
    return {"error": " / ".join(tail)}


def main():
    env_target = os.environ.get("CARGO_TARGET_DIR")
    if not env_target:
        print("warning: CARGO_TARGET_DIR is unset; builds may land in the source tree",
              file=sys.stderr)

    results = {}
    for name in targets(os.path.join(ROOT, "rust-core", "tests")):
        results[name] = run([], name)
    for name in targets(os.path.join(ROOT, "experiments", "tests")):
        results[name] = run(["-p", "sic-experiments"], name)

    ok = [k for k, v in results.items() if v.get("verdict") == "ok"]
    blocked = [k for k, v in results.items() if v.get("blocked")]
    errored = [k for k, v in results.items() if v.get("error")]
    passed = sum(v.get("passed", 0) for v in results.values())
    failed = sum(v.get("failed", 0) for v in results.values())

    print(f"binaries green   : {len(ok)}")
    print(f"binaries blocked : {len(blocked)} {blocked}")
    print(f"binaries errored : {len(errored)} {errored}")
    print(f"tests passed     : {passed}")
    print(f"tests failed     : {failed}")

    out = os.path.join(ROOT, "..", "06-evidence", "per-binary-results.json")
    try:
        with open(out, "w", encoding="utf-8") as handle:
            json.dump(results, handle, indent=1)
        print(f"results written to {out}")
    except OSError as error:
        print(f"could not write results: {error}", file=sys.stderr)

    return 0 if not failed and not errored else 1


if __name__ == "__main__":
    sys.exit(main())
