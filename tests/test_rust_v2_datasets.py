"""Dataset workflows exercise an actual Rust v2 process with a local fixture."""
import json
import hashlib
import os
from pathlib import Path
import sqlite3
import sys
import tempfile
from types import SimpleNamespace
from unittest import TestCase, skipUnless
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "scripts"))
from dataset_adapters import case_metadata
from dataset_runner import Provider
from rust_v2_dataset_runner import add_rust_plan, run
from rust_v2_test_runtime import MockProvider
from verify_dataset_run import verify


class RustPlanTests(TestCase):
    def test_budget_accounts_for_each_independent_question_and_arm(self):
        case = {"dataset": "beam-10m", "wb_chunks": 3,
                "questions": [{"rubric": ["a", "b"]}, {"rubric": ["c"]}]}
        plan = add_rust_plan({"cases": [case], "arms": ["baseline", "sic"]}, "model")
        self.assertEqual(plan["call_estimate"], {"editor": 6, "worker": 4, "judge": 6, "total": 16})


@skipUnless(os.environ.get("SIC_TEST_RUST_BINARY"), "explicit Rust v2 executable needed")
class RustDatasetIntegrationTests(TestCase):
    def test_babilong_fixture_runs_both_arms_and_independently_verifies(self):
        with tempfile.TemporaryDirectory(prefix="sic-rust-dataset-test-") as directory:
            root = Path(directory)
            case = {"dataset": "babilong-qa3-sample", "id": "case-1", "context": "alpha beta gamma delta",
                    "locator": {"source_id": "fixture", "path": "benchmarks/data/fixture.jsonl", "locked_sha256": "fixture"},
                    "protected": [{"role": "system", "content": "Answer briefly."}],
                    "questions": [{"id": "q0", "question": "Where?", "tail": "\nWhere?",
                                   "reference": "office", "rubric": None, "category": "qa3",
                                   "scorer": "babilong-official-label-match-7a6efee"}]}
            plan = {"cases": [case_metadata(case, 10)], "arms": ["baseline", "sic"]}
            add_rust_plan(plan, "model")
            args = SimpleNamespace(rust_binary=Path(os.environ["SIC_TEST_RUST_BINARY"]),
                model_config=None, out=root / "run", max_model_calls=30, provider="mock", editor_mode="model",
                chunk_bytes=10, max_output_tokens=256, max_request_bytes=1024 * 1024,
                dataset=[case["dataset"]], limit_cases=1, question_limit=1, arms=plan["arms"],
                api_key_env="SIC_TEST_UNUSED", endpoint=None, judge_model=None, model=None)
            with patch("rust_v2_dataset_runner.cases", return_value=iter([case])):
                self.assertEqual(run(args, plan, Provider), 0)
            records = json.loads((args.out / "records.json").read_text(encoding="utf-8"))
            self.assertEqual([item["status"] for item in records], ["scored", "scored"])
            self.assertNotEqual(records[0]["chain_id"], records[1]["chain_id"])
            self.assertEqual(len(records[0]["blocks"]), plan["cases"][0]["wb_chunks"])
            self.assertTrue(all("cb_id" in block for block in records[1]["blocks"]))
            self.assertTrue(all(record["pack_verification"]["ok"] for record in records))
            self.assertFalse(verify(args.out)["sic"]["quality_claim"])
            manifest = json.loads((args.out / "manifest.json").read_text(encoding="utf-8"))
            self.assertEqual(manifest["model_steps_dispatched"], plan["call_estimate"]["total"])
            self.assertEqual(manifest["research_evaluation"], "not_evaluated")
            snapshot = args.out / "rust-core-snapshot.sqlite"
            db = sqlite3.connect(str(snapshot))
            try:
                db.execute("DROP TRIGGER immutable_objects_update")
                db.execute("UPDATE objects SET data='{}' WHERE id=?", (records[1]["blocks"][0]["cb_id"],))
                db.commit()
            finally:
                db.close()
            checksums = json.loads((args.out / "checksums.json").read_text(encoding="utf-8"))
            checksums[snapshot.name] = hashlib.sha256(snapshot.read_bytes()).hexdigest()
            (args.out / "checksums.json").write_text(json.dumps(checksums), encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "stored object digest mismatch"):
                verify(args.out)

    def test_beam_questions_are_isolated_and_gold_only_reaches_the_judge(self):
        with tempfile.TemporaryDirectory(prefix="sic-rust-beam-test-") as directory:
            root = Path(directory)
            case = {"dataset": "beam-10m", "id": "beam-1", "context": "first fact and later fact",
                    "locator": {"source_id": "fixture", "path": "benchmarks/data/fixture.parquet", "locked_sha256": "fixture"},
                    "protected": [], "questions": [
                        {"id": "q1", "question": "What was first?", "tail": "\nWhat was first?", "reference": "GOLD_1",
                         "rubric": ["criterion one", "criterion two"], "category": "memory", "scorer": "beam-rubric-fixed-judge-v1"},
                        {"id": "q2", "question": "What was later?", "tail": "\nWhat was later?", "reference": "GOLD_2",
                         "rubric": ["criterion three"], "category": "memory", "scorer": "beam-rubric-fixed-judge-v1"}]}
            plan = add_rust_plan({"cases": [case_metadata(case, 100)], "arms": ["baseline", "sic"]}, "model")
            args = SimpleNamespace(rust_binary=Path(os.environ["SIC_TEST_RUST_BINARY"]),
                model_config=None, out=root / "run", max_model_calls=30, provider="mock", editor_mode="model",
                chunk_bytes=100, max_output_tokens=256, max_request_bytes=1024 * 1024,
                dataset=[case["dataset"]], limit_cases=1, question_limit=2, arms=plan["arms"],
                api_key_env="SIC_TEST_UNUSED", endpoint=None, judge_model=None, model=None)
            fixture = MockProvider(capture_bodies=True)
            with patch("rust_v2_dataset_runner.cases", return_value=iter([case])), \
                 patch("rust_v2_dataset_runner.MockProvider", return_value=fixture):
                self.assertEqual(run(args, plan, Provider), 0)
            records = json.loads((args.out / "records.json").read_text(encoding="utf-8"))
            self.assertEqual(len({record["chain_id"] for record in records}), 4)
            self.assertTrue(all(record["status"] == "scored" for record in records))
            verify(args.out)
            for request in fixture.requests:
                first = request["messages"][0].get("content", "")
                if not (isinstance(first, str) and first.startswith("SIC fixed judge v1.")):
                    self.assertNotIn("GOLD_1", json.dumps(request))
                    self.assertNotIn("GOLD_2", json.dumps(request))

    def test_budget_rejects_before_creating_run_directory(self):
        with tempfile.TemporaryDirectory(prefix="sic-rust-budget-test-") as directory:
            output = Path(directory) / "run"
            args = SimpleNamespace(rust_binary=Path(os.environ["SIC_TEST_RUST_BINARY"]),
                                   model_config=None, out=output, max_model_calls=1, provider="mock")
            with self.assertRaisesRegex(ValueError, "model calls exceed"):
                run(args, {"call_estimate": {"total": 2}}, Provider)
            self.assertFalse(output.exists())

    def test_worker_builtin_tool_continuation_respects_call_bound(self):
        with tempfile.TemporaryDirectory(prefix="sic-rust-tool-test-") as directory:
            root = Path(directory)
            case = {"dataset": "babilong-qa3-sample", "id": "case-tool", "context": "fixture fact",
                    "locator": {"source_id": "fixture", "path": "benchmarks/data/fixture.jsonl", "locked_sha256": "fixture"},
                    "protected": [], "questions": [{"id": "q", "question": "Where?", "tail": "Where?",
                    "reference": "office", "rubric": None, "category": "qa3", "scorer": "babilong-official-label-match-7a6efee"}]}
            plan = add_rust_plan({"cases": [case_metadata(case, 100)], "arms": ["baseline", "sic"]}, "model", 2)
            args = SimpleNamespace(rust_binary=Path(os.environ["SIC_TEST_RUST_BINARY"]), model_config=None,
                out=root / "run", max_model_calls=plan["call_estimate"]["total"], provider="mock", editor_mode="model",
                worker_call_limit=2, chunk_bytes=100, max_output_tokens=256, max_request_bytes=1024 * 1024,
                dataset=[case["dataset"]], limit_cases=1, question_limit=1, arms=plan["arms"],
                api_key_env="SIC_TEST_UNUSED", endpoint=None, judge_model=None, model=None)
            fixture = MockProvider(worker_tool_once=True)
            with patch("rust_v2_dataset_runner.cases", return_value=iter([case])), \
                 patch("rust_v2_dataset_runner.MockProvider", return_value=fixture):
                self.assertEqual(run(args, plan, Provider), 0)
            records = json.loads((args.out / "records.json").read_text(encoding="utf-8"))
            self.assertEqual(len(records[0]["worker_attempt_ids"]), 2)
            self.assertEqual(len(records[1]["worker_attempt_ids"]), 1)
            self.assertLess(json.loads((args.out / "manifest.json").read_text())["model_steps_dispatched"], plan["call_estimate"]["total"])
            verify(args.out)

    def test_hierarchy_fanout_publishes_higher_cbs_and_preserves_source_evidence(self):
        with tempfile.TemporaryDirectory(prefix="sic-rust-hierarchy-test-") as directory:
            root = Path(directory)
            case = {"dataset": "babilong-qa3-sample", "id": "case-levels", "context": "abcdefghijklmnop",
                    "locator": {"source_id": "fixture", "path": "benchmarks/data/fixture.jsonl", "locked_sha256": "fixture"},
                    "protected": [], "questions": [{"id": "q", "question": "Where?", "tail": "Where?",
                    "reference": "office", "rubric": None, "category": "qa3", "scorer": "babilong-official-label-match-7a6efee"}]}
            plan = add_rust_plan({"cases": [case_metadata(case, 4)], "arms": ["baseline", "sic"]}, "model", 1, 2)
            args = SimpleNamespace(rust_binary=Path(os.environ["SIC_TEST_RUST_BINARY"]), model_config=None,
                out=root / "run", max_model_calls=plan["call_estimate"]["total"], provider="mock", editor_mode="model",
                hierarchy_fanout=2, chunk_bytes=4, max_output_tokens=256, max_request_bytes=1024 * 1024,
                dataset=[case["dataset"]], limit_cases=1, question_limit=1, arms=plan["arms"],
                api_key_env="SIC_TEST_UNUSED", endpoint=None, judge_model=None, model=None)
            with patch("rust_v2_dataset_runner.cases", return_value=iter([case])):
                self.assertEqual(run(args, plan, Provider), 0)
            records = json.loads((args.out / "records.json").read_text(encoding="utf-8"))
            self.assertEqual([item["level"] for item in records[1]["hierarchy_actions"]], [2, 2, 3])
            self.assertEqual(len(records[1]["active_cb_ids_before_final"]), 1)
            verify(args.out)
