"""Synthetic privacy/strip regression checks; generated matches are never echoed."""
import json
from pathlib import Path
import struct
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "scripts"))
from scan_release_privacy import scan_bytes, scan_file, pe_identity


class PrivacyTests(unittest.TestCase):
    def test_forbidden_roots_cover_both_slashes_and_utf16(self):
        root = "Q:/private/example-project"
        for variant in (root, root.replace("/", "\\"), root.replace("/", "\\\\")):
            for encoding in ("utf-8", "utf-16-le", "utf-16-be"):
                result = scan_bytes((variant + "/source.rs").encode(encoding), [root])
                self.assertTrue(result["findings"])
                self.assertNotIn(root, json.dumps(result))

    def test_user_directories_fail_without_disclosing_contents(self):
        value = "Q:" + "/Users/" + "example_user/private.txt"
        for encoding in ("ascii", "utf-16-le", "utf-16-be"):
            result = scan_bytes(value.encode(encoding))
            self.assertTrue(any(f["category"] == "windows-user-directory" for f in result["findings"]))
            self.assertNotIn("example_user", json.dumps(result))

    def test_generic_credential_formats_are_blocked_in_all_encodings(self):
        values = ["github_" + "pat_" + "A" * 32, "gh" + "p_" + "B" * 32,
                  "s" + "k-" + "C" * 32, "AI" + "za" + "D" * 35,
                  "-----BEGIN " + "PRIVATE KEY-----", "eyJ" + "A" * 10 + "." + "B" * 20 + "." + "C" * 20,
                  "https://" + "example_user:example_password@127.0.0.1"]
        for value in values:
            for encoding in ("ascii", "utf-16-le", "utf-16-be"):
                result = scan_bytes(value.encode(encoding))
                self.assertTrue(result["findings"])
                self.assertNotIn(value, json.dumps(result))

    def test_public_upstream_layout_cannot_hide_an_explicit_private_root(self):
        for root in ("Q:/a/b/src/rustc-1.0.0-src/vendor", "Q:/a/b/src/llvm-project-1.0.0.src/libunwind/src"):
            value = (root + "/crate/src/file.rs").encode()
            allowed = scan_bytes(value)
            self.assertFalse(allowed["findings"])
            self.assertTrue(allowed["observations"])
            denied = scan_bytes(value, [root])
            self.assertTrue(denied["findings"])

    def test_pe_symbol_table_must_be_absent(self):
        data = bytearray(256)
        data[:2] = b"MZ"
        struct.pack_into("<I", data, 0x3c, 64)
        data[64:68] = b"PE\0\0"
        struct.pack_into("<H", data, 68, 0xaa64)
        self.assertTrue(pe_identity(data)["coff_symbols_absent"])
        struct.pack_into("<II", data, 76, 100, 1)
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "synthetic.exe"
            path.write_bytes(data)
            self.assertFalse(scan_file(path, require_stripped=True)["passed"])
