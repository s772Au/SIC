"""Build an isolated, remapped Rust Core using an already provisioned toolchain."""
import argparse
import json
import os
from pathlib import Path
import shlex
import subprocess

from package_rust_v2 import PLATFORMS, sha256, source_git
from scan_release_privacy import default_roots, scan_file


def build(source, build_dir, platform, toolchain):
    rust_target, binary_format, machine, interpreter = PLATFORMS[platform]
    git = source_git(source)
    revision = subprocess.check_output(git + ["rev-parse", "HEAD"], text=True).strip()
    if subprocess.check_output(git + ["status", "--porcelain"]):
        raise ValueError("release source must be a clean checkout")
    lock_hash = sha256(source / "Cargo.lock")
    rustup = ["rustup", "run", toolchain]
    sysroot = subprocess.check_output(rustup + ["rustc", "--print", "sysroot"], text=True).strip()
    roots = {str(Path(root).resolve()) for root in [*default_roots(), source, build_dir, sysroot]}
    remaps = {root: f"/build/sic-root-{index}" for index, root in
              enumerate(sorted(roots, key=lambda root: (len(root), root)), 1)}
    env = os.environ.copy()
    rust_flags = ["-Cstrip=symbols", "-Cdebuginfo=0"]
    if platform == "windows-x86_64":
        rust_flags.append("-Ctarget-feature=+crt-static")
    rust_flags += [f"--remap-path-prefix={root}={label}" for root, label in remaps.items()]
    env["CARGO_ENCODED_RUSTFLAGS"] = "\x1f".join(rust_flags)
    c_flags = [f"-f{kind}-prefix-map={root}={label}" for root, label in remaps.items()
               for kind in ("file", "debug")]
    for name in ("CFLAGS", "CXXFLAGS"):
        env[name] = f"{env.get(name, '')} {shlex.join(c_flags)}".strip()
    env["CC_SHELL_ESCAPED_FLAGS"] = "1"
    subprocess.run(rustup + ["cargo", "build", "--locked", "--offline", "--release", "--package",
                              "sic-core-rust", "--bin", "sic-core-rust", "--target", rust_target,
                              "--target-dir", str(build_dir)], cwd=source, env=env, check=True)
    binary = build_dir / rust_target / "release" / ("sic-core-rust.exe" if binary_format == "pe" else "sic-core-rust")
    review = scan_file(binary, roots, require_stripped=True)
    header = review[binary_format] or {}
    if not review["passed"] or header.get("machine") != machine or header.get("interpreter") != interpreter:
        raise ValueError(f"release binary rejected: {header}, {review['findings']}")
    if (subprocess.check_output(git + ["rev-parse", "HEAD"], text=True).strip() != revision
            or subprocess.check_output(git + ["status", "--porcelain"])
            or sha256(source / "Cargo.lock") != lock_hash):
        raise ValueError("source or lock changed during build")
    report = {"schema": "sic-privacy-scan-v1", "passed": True, "matched_text_disclosed": False,
              "files": [review], "build_policy": {"kind": "release", "source_commit": revision,
              "cargo_lock_sha256": lock_hash, "remap_path_prefix": True, "rust_target": rust_target,
              "c_and_cpp_prefix_maps": True, "symbol_strip": "symbols", "debug_info": 0}}
    (binary.parent / "privacy-build.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(report, indent=2))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target-dir", required=True, type=Path)
    parser.add_argument("--platform", choices=PLATFORMS, required=True)
    parser.add_argument("--toolchain", required=True)
    args = parser.parse_args()
    build(Path(__file__).resolve().parents[1], args.target_dir.resolve(), args.platform, args.toolchain)
