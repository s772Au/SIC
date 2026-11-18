"""Rust research controls and independent evidence verification."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
from types import SimpleNamespace
from unittest import TestCase, skipUnless
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
sys.path[:0] = [str(ROOT), str(ROOT / "scripts")]
import llmlingua_adapter
from rust_v2_research import planned_calls, run, score, verify


class RustResearchPlanTests(TestCase):
    def test_counts_every_worker_and_editor_call(self):
        engines = ["sic", "batch", "structured_summary", "program_exact", "langchain_trim", "unchanged", "llmlingua"]
        self.assertEqual(planned_calls(2, engines), {"worker": 21, "editor": 5, "total": 26})
        with self.assertRaises(ValueError):
            planned_calls(2, ["sic", "sic"])


@skipUnless(os.environ.get("SIC_TEST_RUST_BINARY"), "explicit Rust v2 executable required")
class RustResearchIntegrationTests(TestCase):
    def args(self, output, engines):
        return SimpleNamespace(rust_binary=Path(os.environ["SIC_TEST_RUST_BINARY"]),
            hierarchy_fanout=0, prices=None, allow_provider=False, provider_config=None, allow_test_gateway=False,
            dataset=None, data_root=None, task=None, chunk_bytes=128, output=output, engines=engines,
            max_model_calls=40, trim_budget=4096)

    def test_all_controls_use_rust_and_fixture_quality_remains_unscored(self):
        with tempfile.TemporaryDirectory(prefix="sic-rust-research-test-") as directory:
            output = Path(directory) / "run"
            engines = ["sic", "batch", "structured_summary", "program_exact", "langchain_trim", "unchanged", "llmlingua"]
            def fake_local_compressor(units, evidence_dir):
                lock = json.loads((ROOT / "research/compression/pack/llmlingua-model.lock.json").read_text(encoding="utf-8"))
                weight = next(item["sha256"] for item in lock["files"] if item["path"] == "model.safetensors")
                evidence_dir.mkdir(parents=True, exist_ok=True)
                (evidence_dir / "llmlingua-0000.json").write_text(json.dumps({"weight_sha256": weight,
                    "classifier_tensor_matches_weight_file": True, "model_forward_calls": 1,
                    "network_connections_blocked": True}), encoding="utf-8")
                return {"ops": [{"op": "COMPRESS", "target": units[0]["unit_id"], "content": "fixture summary"}]}
            with patch.object(llmlingua_adapter, "compress_units", side_effect=fake_local_compressor):
                self.assertEqual(run(self.args(output, engines)), 0)
            result = verify(output)
            self.assertTrue(result["rust_snapshot_and_final_worker_plans_verified"])
            records = json.loads((output / "records.json").read_text(encoding="utf-8"))
            self.assertEqual([r["engine"] for r in records], engines)
            self.assertEqual(len({r["chain_id"] for r in records}), len(engines))
            self.assertEqual(len(records[0]["edits"]), 3)
            self.assertTrue(all(len(r["edits"]) == 1 for r in records[1:]))
            self.assertTrue(all(v["quality"]["status"] == "not_evaluated" for v in score(output)["arms"].values()))
            (output / "sic/result.json").write_text("{}", encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "digest mismatch"):
                verify(output)

    def test_generic_experiment_entry_runs_and_verifies_rust_fixture(self):
        with tempfile.TemporaryDirectory(prefix="sic-rust-experiment-test-") as directory:
            output = Path(directory) / "run"
            binary = os.environ["SIC_TEST_RUST_BINARY"]
            subprocess.run([sys.executable, str(ROOT / "scripts/run_v2_experiment.py"),
                "--rust-binary", binary, "--output", str(output)], check=True, capture_output=True, text=True)
            manifest = json.loads((output / "manifest.json").read_text(encoding="utf-8"))
            self.assertEqual((manifest["backend"], manifest["status"]), ("rust-v2", "complete"))
            verify_proc = subprocess.run([sys.executable, str(ROOT / "scripts/verify_v2_experiment.py"), str(output)],
                                         check=True, capture_output=True, text=True)
            self.assertEqual(json.loads(verify_proc.stdout)["status"], "passed")
            score_proc = subprocess.run([sys.executable, str(ROOT / "scripts/score_v2_experiment.py"), str(output)],
                                        check=True, capture_output=True, text=True)
            self.assertEqual(json.loads(score_proc.stdout)["quality"]["status"], "not_evaluated")

    def test_multimodal_task_is_refused_before_starting(self):
        with tempfile.TemporaryDirectory(prefix="sic-rust-research-media-") as directory:
            root = Path(directory)
            task = root / "task.json"
            task.write_text(json.dumps({"goal": "q", "blocks": ["text"], "media_required": True}), encoding="utf-8")
            args = self.args(root / "run", ["sic"])
            args.task = task
            with self.assertRaisesRegex(ValueError, "media_not_implemented"):
                run(args)
            self.assertFalse(args.output.exists())

    def test_hierarchy_uses_program_editing_without_extra_provider_calls(self):
        with tempfile.TemporaryDirectory(prefix="sic-rust-research-hierarchy-") as directory:
            root = Path(directory)
            args = self.args(root / "run", ["sic"])
            args.hierarchy_fanout = 2
            args.prices = root / "prices.json"
            args.prices.write_text(json.dumps([
                {"category": "input_cache_split_unknown", "unit_price": "1", "per_quantity": 1000000,
                 "currency": "FIXTURE", "price_version": "fixture"},
                {"category": "output_unsplit", "unit_price": "2", "per_quantity": 1000000,
                 "currency": "FIXTURE", "price_version": "fixture"}]), encoding="utf-8")
            self.assertEqual(run(args), 0)
            record = json.loads((args.output / "sic/result.json").read_text(encoding="utf-8"))
            self.assertEqual(sum(edit["level"] == 1 for edit in record["edits"]), 3)
            self.assertEqual(sum(edit["level"] == 2 for edit in record["edits"]), 1)
            self.assertEqual(json.loads((args.output / "manifest.json").read_text())["model_steps_dispatched"], 6)
            self.assertFalse(record["price_estimate"]["summary"]["complete_cost"])
            self.assertIsNone(record["price_estimate"]["summary"]["total_cost"])
            self.assertTrue(verify(args.output)["rust_snapshot_and_final_worker_plans_verified"])
