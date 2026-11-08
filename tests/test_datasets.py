"""Adapter and scorer checks with small local fixtures; no provider calls."""
import json
from pathlib import Path
import sys
import tempfile
import unittest
import importlib.util
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "scripts"))
import dataset_adapters as adapters
from benchmark_scoring import babilong_score, judge_requests, parse_judgment
from dataset_runner import endpoint_check
from dataset_runner import make_plan
from types import SimpleNamespace


class DatasetTests(unittest.TestCase):
    def test_private_resolved_mount_never_enters_plan_or_locator(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "private-input.jsonl"
            record = {"id": "c", "input": "complete context", "question": "where?", "target": "office"}
            path.write_text(json.dumps(record) + "\n", encoding="utf-8")
            entry = {"id": "fixture", "path": "benchmarks/data/babilong/fixture.jsonl", "resolved_path": str(path),
                     "sha256": adapters.file_sha256(path), "bytes": path.stat().st_size}
            args = SimpleNamespace(dataset=["babilong-qa3-sample"], limit_cases=1, question_limit=2, chunk_bytes=1024,
                verify_sources=False, execute=False, arms=["baseline", "sic"], max_request_bytes=1048576)
            with patch.object(adapters, "source_entries", side_effect=lambda _: iter([dict(entry)])), \
                 patch("dataset_runner.source_entries", side_effect=lambda _: iter([dict(entry)])):
                plan = make_plan(args)
            serialized = json.dumps(plan)
            self.assertNotIn("resolved_path", serialized)
            self.assertNotIn(str(path), serialized)
            self.assertEqual(plan["cases"][0]["locator"]["path"], entry["path"])
            self.assertEqual(plan["sources"][0]["path"], entry["path"])

    @unittest.skipUnless(importlib.util.find_spec("ijson"), "install benchmarks/requirements.txt for JSON-array adapter")
    def test_official_babilong_json_array_stream(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "data.json"
            path.write_text(json.dumps([{"input": "whole history", "question": "where?", "target": "office"},
                                        {"input": "second", "question": "where?", "target": "garden"}]), encoding="utf-8")
            entry = {"id": "fixture", "path": "benchmarks/data/fixture/data.json", "resolved_path": str(path), "sha256": adapters.file_sha256(path)}
            with patch.object(adapters, "source_entries", return_value=iter([entry])):
                selected = list(adapters.cases("babilong-qa3", limit=1))
            self.assertEqual(len(selected), 1)
            self.assertEqual(selected[0]["context"], "whole history")
            self.assertEqual(selected[0]["questions"][0]["reference"], "office")

    @unittest.skipUnless(importlib.util.find_spec("pyarrow"), "install benchmarks/requirements.txt for Parquet adapter")
    def test_beam_parquet_keeps_message_order_and_rubric(self):
        import pyarrow as pa
        import pyarrow.parquet as pq
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "data.parquet"
            record = {"conversation_id": "c", "chat": [{"plan-1": [{"turns": [[
                {"role": "user", "content": "first"}, {"role": "assistant", "content": "second"}]]}]}],
                "probing_questions": repr({"abstention": [{"question": "q", "ideal_response": "r", "rubric": ["one", "two"]}]})}
            pq.write_table(pa.Table.from_pylist([record]), path)
            entry = {"id": "fixture", "path": "benchmarks/data/fixture/data.parquet", "resolved_path": str(path), "sha256": adapters.file_sha256(path)}
            with patch.object(adapters, "source_entries", return_value=iter([entry])):
                selected = next(adapters.cases("beam-10m"))
            self.assertLess(selected["context"].index("first"), selected["context"].index("second"))
            self.assertEqual(selected["questions"][0]["rubric"], ["one", "two"])

    def test_chunking_preserves_all_utf8_bytes(self):
        text = "中文事实\nαβγ\n" * 30
        chunks = list(adapters.source_units(text, 25))
        self.assertEqual("".join(chunks), text)
        self.assertTrue(all(len(c.encode()) <= 25 for c in chunks))

    def test_corpus_original_prompt_can_be_reconstructed(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "data.jsonl"
            source = "documents\nQuestion: precise question?\nReturn a list."
            record = {"id": "id1", "question": "precise question?", "answer": ["a", "b"], "prompt": [
                {"role": "system", "content": "protected"}, {"role": "user", "content": source}]}
            path.write_text(json.dumps(record) + "\n", encoding="utf-8")
            entry = {"id": "fixture", "path": "benchmarks/data/fixture/data.jsonl", "resolved_path": str(path), "sha256": adapters.file_sha256(path)}
            with patch.object(adapters, "source_entries", return_value=iter([entry])):
                case = next(adapters.cases("corpusqa-4m"))
            self.assertEqual(case["context"] + case["questions"][0]["tail"], source)
            self.assertEqual(case["questions"][0]["reference"], ["a", "b"])

    def test_official_babilong_metric_is_distinct_from_raw_exact(self):
        result = babilong_score("office", "The milk was in the office.", "Where was the milk before the garden?", "qa3")
        self.assertEqual((result["score"], result["strict_exact"]), (1, 0))
        ambiguous = babilong_score("office", "office or kitchen", "Where was the milk before the garden?", "qa3")
        self.assertEqual(ambiguous["score"], 0)

    def test_judge_is_strict_and_rubric_items_are_independent(self):
        self.assertEqual(len(list(judge_requests("q", "a", "r", ["one", "two"]))), 2)
        for text in ('{"score": true,"reason":"x"}', '{"score":0.5,"reason":"x"}', 'not json'):
            with self.assertRaises((ValueError, json.JSONDecodeError)):
                parse_judgment(text)

    def test_maintenance_endpoint_refused(self):
        with self.assertRaisesRegex(ValueError, "8341"):
            endpoint_check("http://127.0.0.1:8341/v1/chat/completions")


if __name__ == "__main__":
    unittest.main()
