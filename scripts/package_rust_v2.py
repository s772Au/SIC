"""Package the verified Windows ARM64 Rust Core and transparent DSH integration.

Includes no Provider configuration, credentials, sessions, databases, test
fixtures, node_modules or target trees. The output directory must be new.
"""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import tomllib
import zipfile

from scan_release_privacy import scan_file


def sha256(path):
    checksum = hashlib.sha256()
    with Path(path).open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            checksum.update(chunk)
    return checksum.hexdigest()


def package(source, binary, output, runtime, strip):
    source, binary, output, runtime = (Path(p).resolve() for p in (source,binary,output,runtime))
    if output.exists():
        raise FileExistsError("package output must not exist")
    identity = json.loads((binary.parent / "privacy-build.json").read_text(encoding="utf-8-sig"))
    if not (identity.get("passed") and identity.get("build_policy",{}).get("kind") == "release"
            and identity.get("build_policy",{}).get("remap_path_prefix")
            and len(identity.get("files",[])) == 1
            and identity["files"][0]["sha256"] == sha256(binary)):
        raise ValueError("release privacy report does not match binary")
    git=["git","-C",str(source)]
    if (source / "git/.git").is_dir():
        git=["git","--git-dir="+str(source / "git/.git"),"--work-tree="+str(source)]
    revision = subprocess.check_output(git+["rev-parse","HEAD"],text=True).strip()
    if identity["build_policy"].get("source_commit") != revision:
        raise ValueError("binary was not certified from this source commit")
    if identity["build_policy"].get("cargo_lock_sha256") != sha256(source / "Cargo.lock"):
        raise ValueError("binary dependency lock does not match source")
    if subprocess.check_output(git+["status","--porcelain"],text=True).strip():
        raise ValueError("package source must be a clean committed tree")
    output.mkdir(parents=True)
    stage = output / "sic-rust-v2-windows-arm64"
    (stage / "bin").mkdir(parents=True)
    shutil.copy2(source / "LICENSE", stage / "LICENSE")
    shutil.copy2(binary,stage / "bin/sic-core-rust.exe")
    shutil.copy2(runtime / "bin/libunwind.dll",stage / "bin/libunwind.dll")
    subprocess.run([str(strip),"--strip-all",str(stage / "bin/libunwind.dll")],check=True,capture_output=True)
    scans = [scan_file(p,require_stripped=True) for p in (stage / "bin").iterdir()]
    if any(not report["passed"] for report in scans):
        raise ValueError("package binary privacy validation failed")
    if scans[0]["pe"]["machine"] != "0xaa64":
        raise ValueError("package binary is not Windows ARM64")
    shutil.copytree(runtime / "share/licenses/libunwind",stage / "licenses/libunwind")
    shutil.copytree(source / "rust-core/third-party-licenses",stage / "licenses/rust-dependencies")
    plugin=stage / "dsh-plugin"
    shutil.copytree(source / "rust-core/dsh-plugin/transparent",plugin,
                    ignore=shutil.ignore_patterns("node_modules","__pycache__","*.tmp"))
    for name in ("backup_rust_v2.py","scan_release_privacy.py"):
        (stage / "scripts").mkdir(exist_ok=True)
        shutil.copy2(source / "scripts" / name,stage / "scripts" / name)
    (stage / "docs").mkdir()
    shutil.copy2(source / "docs/rust-v2-production-operations.md",stage / "docs")
    with (source / "rust-core/Cargo.toml").open("rb") as manifest_file:
        version = tomllib.load(manifest_file)["package"]["version"]
    manifest={"schema":"sic-rust-v2-release-v1","version":version,"source_commit":revision,
              "platform":"windows-arm64","binary_privacy":scans,
              "components":["Rust Core","opt-in transparent DSH plugin and source patches","Core WAL backup"],
              "runtime_dependencies":{"node":">=22.19.0 for DSH plugin","python":">=3.10 for backup only",
                                      "image_compression":{"optional":True,"requires":["DSH native attachment service","Sharp available to the DSH loader; see plugin peer dependency"]}},
              "deployment":"No running application, database or profile is changed by this package"}
    (stage / "release.json").write_text(json.dumps(manifest,indent=2)+"\n",encoding="utf-8")
    checksums={p.relative_to(stage).as_posix():sha256(p) for p in sorted(stage.rglob("*")) if p.is_file()}
    (stage / "checksums.json").write_text(json.dumps(checksums,indent=2)+"\n",encoding="utf-8")
    archive=output / (stage.name+".zip")
    with zipfile.ZipFile(archive,"x",compression=zipfile.ZIP_DEFLATED) as bundle:
        for path in sorted(stage.rglob("*")):
            if path.is_file():bundle.write(path,stage.name+"/"+path.relative_to(stage).as_posix())
    return {"source_commit":revision,"archive":archive.name,"sha256":sha256(archive),
            "bytes":archive.stat().st_size,"files":len(checksums)}


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source",type=Path,default=Path(__file__).resolve().parents[1])
    parser.add_argument("--binary",type=Path,required=True)
    parser.add_argument("--output",type=Path,required=True)
    parser.add_argument("--runtime",type=Path,required=True)
    parser.add_argument("--strip",type=Path,required=True)
    args=parser.parse_args()
    print(json.dumps(package(args.source,args.binary,args.output,args.runtime,args.strip),indent=2))


if __name__=="__main__":
    main()
