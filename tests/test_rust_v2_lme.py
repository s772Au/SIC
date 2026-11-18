"""Rust LongMemEval adapter and image preflight; never a benchmark score."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
from unittest import TestCase, skipUnless
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from benchmarks.run_core_lme import preflight_rust_trajectories
from scripts.rust_v2_test_runtime import MockProvider, OwnedRustCore, mock_profile


class RustLMEPreflightTests(TestCase):
    def test_checks_selected_trajectories_and_question_images(self):
        with tempfile.TemporaryDirectory(prefix="sic-rust-lme-preflight-") as directory:
            root = Path(directory)
            questions, haystack, trajectories = root / "questions.json", root / "haystack.json", root / "trajectories.jsonl"
            questions.write_text(json.dumps([{"id": "q", "question": "where?"}]), encoding="utf-8")
            haystack.write_text(json.dumps({"q": ["selected"]}), encoding="utf-8")
            rows = [{"id": "selected", "states": [{"screenshot": None}]},
                    {"id": "unused", "states": [{"screenshot": "elsewhere.png"}]}]
            trajectories.write_text("\n".join(map(json.dumps, rows)) + "\n", encoding="utf-8")
            self.assertTrue(preflight_rust_trajectories(questions, haystack, trajectories)["ready"])
            rows[0]["states"][0]["screenshot"] = "selected.png"
            trajectories.write_text("\n".join(map(json.dumps, rows)) + "\n", encoding="utf-8")
            report = preflight_rust_trajectories(questions, haystack, trajectories)
            self.assertFalse(report["ready"])
            self.assertEqual(report["trajectory_image_states"], 1)
            rows[0]["states"][0]["screenshot"] = None
            trajectories.write_text("\n".join(map(json.dumps, rows)) + "\n", encoding="utf-8")
            questions.write_text(json.dumps([{"id": "q", "question": {"text": "where?", "image": "q.png"}}]), encoding="utf-8")
            self.assertFalse(preflight_rust_trajectories(questions, haystack, trajectories)["ready"])


@skipUnless(os.environ.get("SIC_TEST_RUST_BINARY") and os.environ.get("SIC_LME_OFFICIAL_ROOT"),
            "explicit Rust binary and pinned official LME code required")
class RustLMEIntegrationTests(TestCase):
    def test_official_memory_interface_uses_rust_and_refuses_images_before_ingest(self):
        sys.path.insert(0, os.environ["SIC_LME_OFFICIAL_ROOT"])
        from benchmarks.rust_v2_lme_memory import RustV2Memory
        from memory_modules.memory import load_memory
        with tempfile.TemporaryDirectory(prefix="sic-rust-lme-test-") as directory:
            root = Path(directory)
            with MockProvider() as fixture:
                config = root / "profile.json"
                config.write_text(json.dumps(mock_profile(fixture.base)), encoding="utf-8")
                with OwnedRustCore(os.environ["SIC_TEST_RUST_BINARY"], root, config,
                                   {"SIC_RUST_EVAL_MOCK_KEY": "fixture"}) as host:
                    with patch.dict(os.environ, {"SIC_RUST_CORE_URL": host.client.origin,
                         "SIC_RUST_CORE_TOKEN": host.client._token, "SIC_RUST_CORE_ACTOR": "benchmark"}):
                        memory = RustV2Memory({"editor_mode": "program", "wb_state_count": 1, "hierarchy_fanout": 2})
                        trajectory = {"id": "t1", "domain": "web", "goal": "remember", "outcome": "ok",
                                      "states": [{"state_index": 0, "accessibility_tree": "unique fact",
                                                  "action": None, "thought": None, "url": "local", "screenshot": None},
                                                 {"state_index": 1, "accessibility_tree": "second fact", "action": None,
                                                  "thought": None, "url": "local", "screenshot": None}]}
                        memory.insert(trajectory)
                        memory.insert(trajectory)
                        self.assertEqual(len(memory.inserted), 1)
                        self.assertIn("unique fact", json.dumps(memory.query("where?")))
                        before = host.client.call("chain.get", chain_id=memory.chain_id)["cut_seq"]
                        image = dict(trajectory, id="t2", states=[dict(trajectory["states"][0], screenshot="image.png")])
                        with self.assertRaisesRegex(ValueError, "media_not_implemented"):
                            memory.insert(image)
                        self.assertEqual(before, host.client.call("chain.get", chain_id=memory.chain_id)["cut_seq"])
                        with self.assertRaisesRegex(ValueError, "media_not_implemented"):
                            memory.query("where?", query_image="question.png")
                        memory.save_memory(root / "saved")
                        restored = load_memory(root / "saved")
                        self.assertEqual(memory.query("where?"), restored.query("where?"))
                        state_path = root / "saved" / "rust-v2-link.json"
                        state = json.loads(state_path.read_text(encoding="utf-8"))
                        state["pack"]["manifest_digest"] = "invalid"
                        state_path.write_text(json.dumps(state), encoding="utf-8")
                        with self.assertRaisesRegex(ValueError, "no longer matches"):
                            load_memory(root / "saved")

    def test_pinned_official_harness_builds_a_text_memory_in_rust(self):
        with tempfile.TemporaryDirectory(prefix="sic-rust-lme-harness-") as directory:
            root = Path(directory)
            questions, haystack, trajectories = root / "questions.json", root / "haystack.json", root / "trajectories.jsonl"
            questions.write_text(json.dumps([{"id": "q1", "domain": "web", "environment": "webarena",
                "question_type": "dynamic-environment", "question": "Where?", "answer": "withheld-gold",
                "eval_function": "mc_choice_match|require_non_empty=true"}]), encoding="utf-8")
            haystack.write_text(json.dumps({"q1": ["t1"]}), encoding="utf-8")
            trajectory = {"id": "t1", "domain": "web", "goal": "remember", "outcome": "ok",
                          "states": [{"state_index": 0, "accessibility_tree": "official fixture fact",
                                      "action": None, "thought": None, "url": "local", "screenshot": None}]}
            trajectories.write_text(json.dumps(trajectory) + "\n", encoding="utf-8")
            memory_config = root / "memory.json"
            memory_config.write_text(json.dumps({"memory_type": "sic_rust_v2", "memory_params": {"editor_mode": "program"}}), encoding="utf-8")
            with MockProvider() as fixture:
                profile = root / "profile.json"
                profile.write_text(json.dumps(mock_profile(fixture.base)), encoding="utf-8")
                with OwnedRustCore(os.environ["SIC_TEST_RUST_BINARY"], root, profile,
                                   {"SIC_RUST_EVAL_MOCK_KEY": "fixture"}) as host:
                    env = dict(os.environ, SIC_RUST_CORE_URL=host.client.origin,
                               SIC_RUST_CORE_TOKEN=host.client._token, SIC_RUST_CORE_ACTOR="benchmark",
                               PYTHONDONTWRITEBYTECODE="1", PYTHONIOENCODING="utf-8")
                    command = [sys.executable, str(ROOT / "benchmarks/run_core_lme.py"), "--backend", "rust-v2",
                        "--official-root", os.environ["SIC_LME_OFFICIAL_ROOT"], "harness", "--domain", "web",
                        "--questions-path", str(questions), "--haystack-path", str(haystack),
                        "--trajectories-path", str(trajectories), "--memory-config-path", str(memory_config),
                        "--output-dir", str(root / "run"), "--save-memory", "--skip-evaluation"]
                    result = subprocess.run(command, env=env, capture_output=True, text=True, encoding="utf-8", timeout=120)
                    self.assertEqual(result.returncode, 0, result.stdout + "\n" + result.stderr)
                    saved = json.loads((root / "run/memory_state/rust-v2-link.json").read_text(encoding="utf-8"))
                    self.assertEqual(len(saved["inserted"]), 1)
                    view = host.client.call("cut.get", chain_id=saved["chain_id"])["view"]
                    self.assertIn("official fixture fact", json.dumps(view))
                    self.assertNotIn("withheld-gold", json.dumps(view))
