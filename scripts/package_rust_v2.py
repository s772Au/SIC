"""Package a verified Rust Core and the transparent DSH integration.

Includes no Provider configuration, credentials, sessions, databases, test
fixtures, node_modules or target trees. The output directory must be new.
"""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import tarfile
import tomllib
import zipfile

from scan_release_privacy import scan_file

PLATFORMS = {
    "windows-arm64": ("aarch64-pc-windows-gnullvm", "pe", "0xaa64", None),
    "windows-x86_64": ("x86_64-pc-windows-gnu", "pe", "0x8664", None),
    "linux-arm64": ("aarch64-unknown-linux-gnu", "elf", "0x00b7", "/lib/ld-linux-aarch64.so.1"),
    "linux-x86_64": ("x86_64-unknown-linux-musl", "elf", "0x003e", None),
}

HOST_FILES = {
    "Pi": ("docs/pi-native-compaction.md", "integrations/pi/sic-archive.ts", "integrations/pi/sic-compaction.ts",
           "integrations/pi/sic-visual.ts"),
    "OpenCode": ("docs/opencode-native-compaction.md", "integrations/opencode/index.mjs", "integrations/opencode/package.json"),
    "Hermes": ("docs/hermes-native-compaction.md", "integrations/hermes/sic_context.py",
               *(f"integrations/hermes/hermes-v0.23.2-{kind}.patch" for kind in (
                   "required-selection", "required-binding", "compaction-authority", "native-editor"))),
    "Claude Code": ("docs/claude-code-native-compaction.md", "integrations/claude/sic_messages.py",
                    "integrations/claude/gateway.py", "integrations/claude/launch.py"),
    "Codex": ("docs/codex-native-compaction.md", "integrations/codex/__init__.py", "integrations/codex/loopback.py", "integrations/codex/sic_responses.py",
              "integrations/codex/native_span.py", "integrations/codex/native_history.py", "integrations/codex/native_launch.py"),
}


def sha256(path):
    checksum = hashlib.sha256()
    with Path(path).open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            checksum.update(chunk)
    return checksum.hexdigest()


def source_git(source):
    nested = source / "git/.git"
    if nested.is_dir():
        return ["git", "--git-dir=" + str(nested), "--work-tree=" + str(source)]
    return ["git", "-C", str(source)]


def public_tar_entry(member):
    member.uid = member.gid = 0
    member.uname = member.gname = ""
    return member


def package(source, binary, output, runtime=None, strip=None, platform="windows-arm64",
            runtime_licenses=None):
    source, binary, output = (Path(p).resolve() for p in (source, binary, output))
    target, binary_format, machine, interpreter = PLATFORMS[platform]
    windows = binary_format == "pe"
    if output.exists():
        raise FileExistsError("package output must not exist")
    identity = json.loads((binary.parent / "privacy-build.json").read_text(encoding="utf-8-sig"))
    if not (identity.get("passed") and identity.get("build_policy",{}).get("kind") == "release"
            and identity.get("build_policy",{}).get("remap_path_prefix")
            and len(identity.get("files",[])) == 1
            and identity["files"][0]["sha256"] == sha256(binary)):
        raise ValueError("release privacy report does not match binary")
    policy = identity["build_policy"]
    if policy.get("rust_target", "aarch64-pc-windows-gnullvm") != target:
        raise ValueError("build report does not certify the requested target")
    git = source_git(source)
    revision = subprocess.check_output(git+["rev-parse","HEAD"],text=True).strip()
    if identity["build_policy"].get("source_commit") != revision:
        raise ValueError("binary was not certified from this source commit")
    if identity["build_policy"].get("cargo_lock_sha256") != sha256(source / "Cargo.lock"):
        raise ValueError("binary dependency lock does not match source")
    if subprocess.check_output(git+["status","--porcelain"],text=True).strip():
        raise ValueError("package source must be a clean committed tree")
    core_report = scan_file(binary, require_stripped=True)
    header = core_report[binary_format]
    if (not core_report["passed"] or (header or {}).get("machine") != machine
            or header.get("interpreter") != interpreter):
        raise ValueError("Core format, machine, interpreter or privacy does not match target")
    if platform == "windows-arm64" and (runtime is None or strip is None):
        raise ValueError("Windows ARM64 requires --runtime and --strip for libunwind")
    if platform in {"windows-x86_64", "linux-x86_64"}:
        if runtime_licenses is None or not any(p.is_file() for p in Path(runtime_licenses).iterdir()):
            raise ValueError("statically linked runtime requires its redistribution notices")
    output.mkdir(parents=True)
    stage = output / f"sic-rust-v2-{platform}"
    (stage / "bin").mkdir(parents=True)
    shutil.copy2(source / "LICENSE", stage / "LICENSE")
    executable_name = "sic-core-rust.exe" if windows else "sic-core-rust"
    executable = stage / "bin" / executable_name
    shutil.copy2(binary, executable)
    core_report["file"] = executable_name
    scans = [core_report]
    if not windows:
        executable.chmod(0o755)
    if platform == "windows-arm64":
        runtime = Path(runtime).resolve()
        unwind = stage / "bin/libunwind.dll"
        shutil.copy2(runtime / "bin/libunwind.dll", unwind)
        subprocess.run([str(strip), "--strip-all", str(unwind)], check=True, capture_output=True)
        report = scan_file(unwind, require_stripped=True)
        if not report["passed"] or (report["pe"] or {}).get("machine") != machine:
            raise ValueError("libunwind machine or privacy does not match target")
        scans.append(report)
        shutil.copytree(runtime / "share/licenses/libunwind", stage / "licenses/libunwind")
    shutil.copytree(source / "rust-core/third-party-licenses",stage / "licenses/rust-dependencies")
    if runtime_licenses is not None:
        shutil.copytree(runtime_licenses, stage / "licenses/toolchain-runtime")
    plugin=stage / "dsh-plugin"
    shutil.copytree(source / "rust-core/dsh-plugin/transparent",plugin,
                    ignore=shutil.ignore_patterns("node_modules","__pycache__","*.tmp"))
    for name in ("backup_rust_v2.py","scan_release_privacy.py"):
        (stage / "scripts").mkdir(exist_ok=True)
        shutil.copy2(source / "scripts" / name,stage / "scripts" / name)
    (stage / "docs").mkdir()
    shutil.copy2(source / "docs/rust-v2-production-operations.md",stage / "docs")
    components = ["Rust Core", "opt-in transparent DSH plugin and source patches", "Core WAL backup"]
    host_files = ["integrations/mcp/archive_server.py", "docs/portability-and-hosts.md"]
    for host, paths in HOST_FILES.items():
        host_files.extend(paths)
        components.append(f"{host} native compaction integration")
    for relative in host_files:
        destination = stage / relative
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source / relative, destination)
    with (source / "rust-core/Cargo.toml").open("rb") as manifest_file:
        version = tomllib.load(manifest_file)["package"]["version"]
    manifest={"schema":"sic-rust-v2-release-v1","version":version,"source_commit":revision,
              "platform":platform,"rust_target":target,"binary_privacy":scans,
              "components":components,
              "runtime_dependencies":{"node":">=22.19.0 for DSH plugin","python":">=3.10 for backup and >=3.11 for Python host launchers",
                                      "image_compression":{"optional":True,"requires":["DSH native attachment service","Sharp available to the DSH loader; see plugin peer dependency"]}},
              "deployment":"No running application, database or profile is changed by this package"}
    (stage / "release.json").write_text(json.dumps(manifest,indent=2)+"\n",encoding="utf-8")
    checksums={p.relative_to(stage).as_posix():sha256(p) for p in sorted(stage.rglob("*")) if p.is_file()}
    (stage / "checksums.json").write_text(json.dumps(checksums,indent=2)+"\n",encoding="utf-8")
    archive = output / (stage.name + (".zip" if windows else ".tar.gz"))
    with (zipfile.ZipFile(archive, "x", compression=zipfile.ZIP_DEFLATED) if windows
          else tarfile.open(archive, "x:gz")) as bundle:
        for path in sorted(stage.rglob("*")):
            if not path.is_file():
                continue
            relative = stage.name + "/" + path.relative_to(stage).as_posix()
            if windows:
                bundle.write(path, relative)
            else:
                bundle.add(path, relative, filter=public_tar_entry)
    return {"source_commit":revision,"archive":archive.name,"sha256":sha256(archive),
            "bytes":archive.stat().st_size,"files":len(checksums)}


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source",type=Path,default=Path(__file__).resolve().parents[1])
    parser.add_argument("--binary",type=Path,required=True)
    parser.add_argument("--output",type=Path,required=True)
    parser.add_argument("--platform", choices=PLATFORMS, default="windows-arm64")
    parser.add_argument("--runtime",type=Path)
    parser.add_argument("--strip",type=Path)
    parser.add_argument("--runtime-licenses", type=Path, help="redistribution notices for static runtime libraries")
    args=parser.parse_args()
    print(json.dumps(package(args.source, args.binary, args.output, args.runtime, args.strip,
                             platform=args.platform, runtime_licenses=args.runtime_licenses), indent=2))


if __name__=="__main__":
    main()
