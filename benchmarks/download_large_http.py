"""Resumable, low-memory download of official Hugging Face dataset files.

This utility never contacts the SIC model gateway.
"""

import argparse
import hashlib
import json
import os
import re
import time
import urllib.error
import urllib.request
from pathlib import Path
from source_paths import report_path


ROOT = Path(__file__).resolve().parent
BABILONG_REVISION = "ee0d588794c7ac098062ee0d247c733d62e94fe2"
LME_REVISION = "f152293e235517d504809563c833d7190b8c713b"


def official_url(repo, revision, filename):
    return f"https://huggingface.co/datasets/{repo}/resolve/{revision}/{filename}"


DATASETS = {
    "babilong-qa3-10m": (
        official_url("RMT-team/babilong", BABILONG_REVISION, "data/qa3/10M.json"),
        ROOT / "data" / "babilong" / "data" / "qa3" / "10M.json",
        4025904013,
    ),
    "longmemeval-v2-trajectories": (
        official_url("xiaowu0162/longmemeval-v2", LME_REVISION, "trajectories.jsonl"),
        ROOT / "data" / "longmemeval-v2" / "trajectories.jsonl",
        1195604539,
    ),
    "longmemeval-v2-enterprise-images": (
        official_url("xiaowu0162/longmemeval-v2", LME_REVISION, "trajectory_screenshots/enterprise_screenshots_base.tar.gz"),
        ROOT / "data" / "longmemeval-v2" / "trajectory_screenshots" / "enterprise_screenshots_base.tar.gz",
        3354163660,
    ),
    "longmemeval-v2-web-images": (
        official_url("xiaowu0162/longmemeval-v2", LME_REVISION, "trajectory_screenshots/web_screenshots.tar.gz"),
        ROOT / "data" / "longmemeval-v2" / "trajectory_screenshots" / "web_screenshots.tar.gz",
        2562302847,
    ),
}
for task, size in {1: 4018016805, 2: 4019960532, 4: 4016999567, 5: 4017418339}.items():
    DATASETS[f"babilong-qa{task}-10m"] = (
        official_url("RMT-team/babilong", BABILONG_REVISION, f"data/qa{task}/10M.json"),
        ROOT / "data" / "babilong" / "data" / f"qa{task}" / "10M.json",
        size,
    )


def sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(8 * 1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def verify_range(value, start, end, total):
    match = re.fullmatch(r"bytes (\d+)-(\d+)/(\d+)", value or "")
    if match is None or tuple(map(int, match.groups())) != (start, end, total):
        raise ValueError(f"unexpected Content-Range {value!r}; expected bytes {start}-{end}/{total}; no bytes appended")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("dataset", choices=DATASETS)
    args = parser.parse_args()
    url, target, expected = DATASETS[args.dataset]
    locked = next(item for item in json.loads((ROOT / "sources-lock.json").read_text(encoding="utf-8"))["files"]
                  if item["id"] == args.dataset)
    if locked["bytes"] != expected:
        raise ValueError("source size disagrees with sources-lock.json")
    expected_sha256 = locked["sha256"]
    target.parent.mkdir(parents=True, exist_ok=True)
    if target.is_file() and target.stat().st_size == expected:
        actual = sha256(target)
        if actual != expected_sha256:
            raise ValueError(f"existing file SHA-256 mismatch: {report_path(target)}; expected {expected_sha256}, got {actual}")
        print(f"already complete and SHA-256 verified: {report_path(target)}")
        return
    partial = target.with_suffix(target.suffix + ".part")
    current = partial.stat().st_size if partial.exists() else 0
    if current > expected:
        raise ValueError("partial file exceeds expected size")
    segment_bytes = 16 * 1024 * 1024
    started = time.monotonic()
    initial = current
    with partial.open("ab") as output:
        failures = 0
        while current < expected:
            end = min(current + segment_bytes, expected) - 1
            # Distinct official resolve URLs avoid intermediaries reusing a stale
            # signed redirect for a different Range. The pinned revision and
            # strict Content-Range/hash checks still define byte identity.
            request_url = f"{url}?download=true&sic_range_start={current}"
            request = urllib.request.Request(request_url, headers={"Range": f"bytes={current}-{end}"})
            try:
                with urllib.request.urlopen(request, timeout=45) as response:
                    if response.status != 206:
                        raise RuntimeError("server did not honor Range; refusing to append duplicate bytes")
                    verify_range(response.headers.get("Content-Range"), current, end, expected)
                    remaining = end + 1 - current
                    while remaining:
                        chunk = response.read(min(1024 * 1024, remaining))
                        if not chunk:
                            raise OSError(f"short download at {current}")
                        output.write(chunk)
                        current += len(chunk)
                        remaining -= len(chunk)
                failures = 0
                elapsed = time.monotonic() - started
                rate = (current - initial) / max(elapsed, 0.001)
                print(f"{args.dataset}: {current:,}/{expected:,} bytes; {rate / 1024 / 1024:.2f} MiB/s average", flush=True)
            except (OSError, urllib.error.URLError) as exc:
                output.flush()
                failures += 1
                if failures > 8:
                    raise RuntimeError(f"download stalled at {current}; rerun to resume") from exc
                print(f"{args.dataset}: retry {failures} from byte {current}: {type(exc).__name__}", flush=True)
                time.sleep(min(2 * failures, 10))
    if current != expected:
        raise RuntimeError(f"incomplete: {current} of {expected} bytes; rerun to resume")
    actual = sha256(partial)
    if actual != expected_sha256:
        raise ValueError(f"downloaded SHA-256 mismatch; retained {report_path(partial)}; expected {expected_sha256}, got {actual}")
    os.replace(partial, target)
    print(f"complete; SHA-256 {actual}: {report_path(target)}")


if __name__ == "__main__":
    main()
