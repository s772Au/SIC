import importlib.util
from pathlib import Path
import sqlite3
import hashlib

import pytest

SPEC = importlib.util.spec_from_file_location("rust_backup", Path(__file__).resolve().parents[1] / "scripts/backup_rust_v2.py")
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


def database(path):
    connection = sqlite3.connect(path)
    connection.execute("PRAGMA journal_mode=WAL")
    connection.execute("CREATE TABLE sic_rust_meta(key TEXT PRIMARY KEY,value TEXT)")
    connection.execute("INSERT INTO sic_rust_meta VALUES('schema','sic-rust-core-v1')")
    connection.execute("CREATE TABLE objects(id TEXT PRIMARY KEY,data TEXT,digest TEXT)")
    connection.commit()
    return connection


def test_live_wal_snapshot_preserves_committed_objects_and_never_replaces_destination(tmp_path):
    source, target = tmp_path / "live.db", tmp_path / "backup.db"
    connection = database(source)
    try:
        text = '{"text":"committed WAL material"}'
        connection.execute("INSERT INTO objects VALUES(?,?,?)", ("one", text, hashlib.sha256(text.encode()).hexdigest()))
        connection.commit()
        report = MODULE.backup(source, target)
        assert report["objects_verified"] == 1
        with sqlite3.connect(target) as restored:
            assert restored.execute("SELECT data FROM objects").fetchone() == (text,)
        original = target.read_bytes()
        with pytest.raises(FileExistsError):
            MODULE.backup(source, target)
        assert target.read_bytes() == original
        assert not list(tmp_path.glob("*.partial"))
    finally:
        connection.close()


def test_corrupt_object_snapshot_is_not_published_and_source_stays_unchanged(tmp_path):
    source, target = tmp_path / "live.db", tmp_path / "backup.db"
    connection = database(source)
    connection.execute("INSERT INTO objects VALUES('bad','{}','wrong')")
    connection.commit()
    try:
        with pytest.raises(ValueError, match="digest"):
            MODULE.backup(source, target)
        assert not target.exists()
        assert connection.execute("SELECT digest FROM objects").fetchone() == ("wrong",)
        assert not list(tmp_path.glob("*.partial"))
    finally:
        connection.close()
