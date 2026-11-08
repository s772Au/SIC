"""Resolve private local mounts while keeping public source identities portable."""
import json
import os
from pathlib import Path, PurePosixPath, PureWindowsPath

PROJECT_ROOT = Path(__file__).resolve().parent.parent


def public_source_path(entry):
    value = str(entry["path"]).replace("\\", "/")
    path = PurePosixPath(value)
    if path.is_absolute() or PureWindowsPath(value).is_absolute() or ".." in path.parts:
        raise ValueError("Public source paths must be project-relative")
    if not value.startswith("benchmarks/data/"):
        raise ValueError("Public source paths must be under benchmarks/data")
    return path.as_posix()


def _overrides(path):
    if not path.is_file():
        return {}
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
        mapping = value["sources"]
        if not isinstance(mapping, dict) or not all(isinstance(k, str) and isinstance(v, str) and v for k, v in mapping.items()):
            raise ValueError
        return {key: Path(value) if Path(value).is_absolute() else path.parent / value for key, value in mapping.items()}
    except (OSError, ValueError, KeyError, TypeError):
        raise ValueError("Invalid private source override configuration") from None


def resolve_source(entry, project_root=PROJECT_ROOT):
    """Explicit override > DATA_ROOT > automatic private override > public path.

    Return values are for local I/O only; never write them to public reports.
    """
    root = Path(project_root)
    public = public_source_path(entry)
    explicit = os.environ.get("SIC_SOURCE_OVERRIDES")
    if explicit:
        selected = _overrides(Path(explicit))
        if entry["id"] in selected:
            return selected[entry["id"]].resolve()
    data_root = os.environ.get("SIC_DATA_ROOT")
    if data_root:
        return (Path(data_root) / public.removeprefix("benchmarks/data/")).resolve()
    if not explicit:
        selected = _overrides(root / ".local/source-overrides.json")
        if entry["id"] in selected:
            return selected[entry["id"]].resolve()
    return (root / public).resolve()


def report_path(path, project_root=PROJECT_ROOT, fallback_directory="prepared"):
    """Produce a relative display path without exposing an external local mount."""
    value = Path(path)
    try:
        return value.resolve().relative_to(Path(project_root).resolve()).as_posix()
    except ValueError:
        return (PurePosixPath(fallback_directory) / value.name).as_posix()


def public_path_for_local_source(path, project_root=PROJECT_ROOT):
    root = Path(project_root)
    entries = json.loads((root / "benchmarks/sources-lock.json").read_text(encoding="utf-8"))["files"]
    target = Path(path).resolve()
    for entry in entries:
        if resolve_source(entry, root) == target:
            return public_source_path(entry)
    return report_path(path, root, fallback_directory="data")
