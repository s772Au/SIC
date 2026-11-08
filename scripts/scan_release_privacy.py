"""Scan printable binary/text strings without echoing private matches or roots."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import struct
import sys


def root_variants(root):
    text = str(root).rstrip("/\\")
    if not text:
        return set()
    forward = text.replace("\\", "/")
    back = forward.replace("/", "\\")
    values = {forward, back, back.replace("\\", "\\\\")}
    if re.match(r"^[A-Za-z]:/", forward):
        values.add("/" + forward[0].lower() + forward[2:])
    return values


def scan_bytes(data, forbidden_roots=()):
    patterns = [
        ("windows-user-directory", re.compile(r"(?i)[a-z]:[\\/]+(?:users|documents and settings)[\\/]")),
        ("unix-user-directory", re.compile(r"(?i)/(?:home|users)/[a-z0-9_.-]+")),
        ("absolute-windows-directory", re.compile(r"(?i)(?<![a-z0-9])[a-z]:[\\/]+[a-z0-9_. -]{1,80}[\\/]")),
        ("github-credential", re.compile(r"(?:github_pat_[A-Za-z0-9_]{22,255}|gh[pousr]_[A-Za-z0-9]{20,255})")),
        ("api-key-credential", re.compile(r"(?<![A-Za-z0-9])sk-[A-Za-z0-9_-]{20,}")),
        ("google-api-credential", re.compile(r"AIza[A-Za-z0-9_-]{30,}")),
        ("private-key-material", re.compile(r"-----BEGIN (?:[A-Z0-9]+ )?PRIVATE KEY-----")),
        ("jwt-credential", re.compile(r"(?<![A-Za-z0-9_-])eyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}")),
        ("credential-url", re.compile(r"(?i)https?://[^\s/@:]+:[^\s/@]+@")),
    ]
    roots = {value.casefold() for root in forbidden_roots for value in root_variants(root)}
    findings = set()
    observations = {}
    # Direct root checks also cover non-ASCII path components; output is labels only.
    for encoding in ("utf-8", "utf-16-le", "utf-16-be"):
        lowered = data.lower()
        if any(root.encode(encoding).lower() in lowered for root in roots):
            findings.add(("forbidden-local-root", encoding))
    blocks = [("ascii", rb"[\x20-\x7e]{4,}", "ascii"),
              ("utf-16-le", rb"(?:[\x20-\x7e]\x00){4,}", "utf-16-le"),
              ("utf-16-be", rb"(?:\x00[\x20-\x7e]){4,}", "utf-16-be")]
    for label, expression, encoding in blocks:
        for match in re.finditer(expression, data):
            value = match.group().decode(encoding)
            for category, pattern in patterns:
                for found in pattern.finditer(value):
                    # Distributed Rust std contains these public compiler-package
                    # vendor locations. They are not this build's roots. Explicit
                    # forbidden roots and user-directory rules always still fail.
                    normalized = value.replace("\\", "/")
                    upstream = re.match(r"(?i)[a-z]:/[a-z]/[a-z]/src/rustc-\d+\.\d+\.\d+-src/vendor/", normalized[found.start():])
                    unwind = re.match(r"(?i)[a-z]:/[a-z]/[a-z]/src/llvm-project-\d+\.\d+\.\d+\.src/libunwind/src/", normalized[found.start():])
                    if category == "absolute-windows-directory" and (upstream or unwind):
                        key = ("upstream-rust-vendor-location" if upstream else "upstream-llvm-libunwind-location", label)
                        observations[key] = observations.get(key, 0) + 1
                    else:
                        findings.add((category, label))
    return {"findings": [{"category": category, "encoding": encoding} for category, encoding in sorted(findings)],
            "observations": [{"category": category, "encoding": encoding, "count": count}
                             for (category, encoding), count in sorted(observations.items())]}


def pe_identity(data):
    if len(data) < 64 or data[:2] != b"MZ":
        return None
    offset = struct.unpack_from("<I", data, 0x3c)[0]
    if offset + 24 > len(data) or data[offset:offset + 4] != b"PE\0\0":
        return None
    machine, = struct.unpack_from("<H", data, offset + 4)
    symbols_at, symbols_count = struct.unpack_from("<II", data, offset + 12)
    return {"machine": f"0x{machine:04x}", "coff_symbols_absent": symbols_at == 0 and symbols_count == 0}


def default_roots():
    roots = [Path(__file__).resolve().parents[1], Path.home()]
    for name in ("USERPROFILE", "HOME", "CARGO_HOME", "RUSTUP_HOME", "SIC_TOOLCHAIN_ROOT"):
        if os.environ.get(name):
            roots.append(os.environ[name])
    return roots


def scan_file(path, forbidden_roots=(), require_stripped=False):
    data = path.read_bytes()
    scan = scan_bytes(data, list(default_roots()) + list(forbidden_roots))
    findings = scan["findings"]
    identity = pe_identity(data)
    if require_stripped and (identity is None or not identity["coff_symbols_absent"]):
        findings.append({"category": "coff-symbols-or-non-pe", "encoding": "pe-header"})
    return {"file": path.name, "bytes": len(data), "sha256": hashlib.sha256(data).hexdigest(),
            "findings": findings, "observations": scan["observations"], "pe": identity, "passed": not findings}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("files", nargs="+", type=Path)
    parser.add_argument("--forbid-root", action="append", default=[])
    parser.add_argument("--require-stripped", action="store_true")
    parser.add_argument("--out", type=Path)
    args = parser.parse_args()
    result = {"schema": "sic-privacy-scan-v1", "matched_text_disclosed": False,
              "files": [scan_file(path, args.forbid_root, args.require_stripped) for path in args.files]}
    result["passed"] = all(item["passed"] for item in result["files"])
    serialized = json.dumps(result, indent=2) + "\n"
    if args.out:
        args.out.write_text(serialized, encoding="utf-8")
    print(serialized, end="")
    return 0 if result["passed"] else 1


if __name__ == "__main__":
    sys.exit(main())
