"""Runner integrity checks; no network access or model calls."""
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "scripts"))
from run_experiment import score
from run_experiment import digest, write_json
from score_experiment import verify


class RunnerTests(unittest.TestCase):
    def test_tampered_artifact_is_rejected(self):
        with tempfile.TemporaryDirectory() as tmp:
            directory = Path(tmp)
            artifact = directory / "records.json"
            write_json(artifact, [])
            write_json(directory / "checksums.json", {"records.json": digest(artifact.read_bytes())})
            artifact.write_text("tampered", encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "digest mismatch"):
                verify(directory)

    def test_failed_answer_is_missing_not_zero_scored(self):
        result = score([{"status": "failed"}, {"status": "answered", "answer": "wrong", "expected": "yes"}])
        self.assertEqual((result["answered"], result["missing_or_failed"], result["correct"]), (1, 1, 0))

    def test_existing_output_is_never_reused(self):
        with tempfile.TemporaryDirectory() as tmp:
            marker = Path(tmp) / "keep.txt"
            marker.write_text("untouched")
            result = subprocess.run([sys.executable, str(ROOT / "scripts/run_experiment.py"), "--out", tmp], capture_output=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertEqual(list(Path(tmp).iterdir()), [marker])

    def test_maintenance_gateway_rejected_before_output_creation(self):
        with tempfile.TemporaryDirectory() as tmp:
            output = Path(tmp) / "must-not-exist"
            result = subprocess.run([sys.executable, str(ROOT / "scripts/run_experiment.py"), "--out", str(output),
                "--provider", "chat-completions", "--endpoint", "http://127.0.0.1:8341/v1/chat/completions", "--model", "fixed", "--allow-network"], capture_output=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertFalse(output.exists())
            self.assertIn(b"8341", result.stderr)


if __name__ == "__main__":
    unittest.main()
