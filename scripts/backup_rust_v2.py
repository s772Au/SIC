"""Create and verify a separate Rust Core snapshot, including committed WAL data.

This backs up the Core database, not the Harness Session/projection files or
external processes. Quiesce the Harness and include those files for a deployment
restore point. Existing destinations are never overwritten.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import sqlite3
import time
import uuid
from contextlib import closing


def verify(connection):
    marker = connection.execute("SELECT value FROM sic_rust_meta WHERE key='schema'").fetchone()
    if marker != ("sic-rust-core-v1",):
        raise ValueError("source is not a Rust Core database")
    if connection.execute("PRAGMA integrity_check").fetchall() != [("ok",)]:
        raise ValueError("snapshot failed SQLite integrity check")
    objects = 0
    for data, expected in connection.execute("SELECT data,digest FROM objects"):
        if hashlib.sha256(data.encode("utf-8")).hexdigest() != expected:
            raise ValueError("snapshot contains an invalid immutable object digest")
        json.loads(data)
        objects += 1
    return objects


def backup(source, destination, timeout=120):
    source, destination = Path(source).resolve(), Path(destination).resolve()
    if not source.is_file():
        raise ValueError("source database must exist")
    if source == destination or destination.exists():
        raise FileExistsError("backup destination already exists")
    if timeout <= 0:
        raise ValueError("backup timeout must be positive")
    if not destination.parent.is_dir():
        raise ValueError("backup destination directory must exist")
    temporary = destination.with_name(destination.name + "." + uuid.uuid4().hex + ".partial")
    started = time.monotonic()
    # O_EXCL avoids changing an existing file even before SQLite opens it.
    fd = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    os.close(fd)
    try:
        with closing(sqlite3.connect(source.as_uri() + "?mode=ro", uri=True, timeout=10)) as original:
            marker = original.execute("SELECT value FROM sic_rust_meta WHERE key='schema'").fetchone()
            if marker != ("sic-rust-core-v1",):
                raise ValueError("source is not a Rust Core database")
            with closing(sqlite3.connect(temporary, timeout=10)) as target:
                def progress(_status, _remaining, _total):
                    if time.monotonic() - started > timeout:
                        raise TimeoutError("backup exceeded its deadline")
                original.backup(target, pages=128, progress=progress, sleep=0.05)
                objects = verify(target)
        # Close connections before publishing (also required on Windows).
        checksum = hashlib.sha256()
        with temporary.open("rb+") as stream:
            os.fsync(stream.fileno())
            for chunk in iter(lambda: stream.read(1024 * 1024), b""):
                checksum.update(chunk)
        # Publish only a complete, verified snapshot, without replacement.
        os.link(temporary, destination)
        return {"ok": True, "schema": "sic-rust-core-v1", "objects_verified": objects,
                "bytes": destination.stat().st_size,
                "sha256": checksum.hexdigest(),
                "duration_seconds": round(time.monotonic() - started, 3),
                "scope": "Core SQLite only; Harness files and external processes are separate"}
    finally:
        # A partial snapshot is never exposed under the requested final name.
        temporary.unlink(missing_ok=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=Path)
    parser.add_argument("destination", type=Path)
    parser.add_argument("--timeout", type=float, default=120)
    args = parser.parse_args()
    try:
        print(json.dumps(backup(args.source, args.destination, args.timeout), indent=2))
    except (ValueError, OSError, sqlite3.Error, TimeoutError) as error:
        # Do not echo database contents or configured paths from driver errors.
        parser.exit(1, "Core backup failed: " + type(error).__name__ + "\n")


if __name__ == "__main__":
    main()
