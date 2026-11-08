"""One-record-at-a-time text adapters. Never truncate histories or load whole datasets."""
import ast
import hashlib
import json
import importlib.util
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
VERSION = "sic-text-adapters-v1"
_resolver_spec = importlib.util.spec_from_file_location("sic_public_source_paths", ROOT / "benchmarks/source_paths.py")
_resolver = importlib.util.module_from_spec(_resolver_spec)
_resolver_spec.loader.exec_module(_resolver)
resolve_source = _resolver.resolve_source
public_source_path = _resolver.public_source_path


def file_sha256(path):
    value = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(8 * 1024 * 1024), b""):
            value.update(chunk)
    return value.hexdigest()


def source_entries(dataset):
    entries = json.loads((ROOT / "benchmarks/sources-lock.json").read_text(encoding="utf-8"))["files"]
    identifiers = {
        "corpusqa-4m": ["corpusqa-4m"], "corpusqa-10m": ["corpusqa-10m"],
        "beam-10m": ["beam-10m-0", "beam-10m-1"],
        "babilong-qa3-sample": ["babilong-qa3-10m-first20-derived"],
        **{f"babilong-qa{i}": [f"babilong-qa{i}-10m"] for i in range(1, 6)},
    }
    if dataset not in identifiers:
        raise ValueError("unknown dataset: " + dataset)
    selected = [entry for entry in entries if entry["id"] in identifiers[dataset]]
    if len(selected) != len(identifiers[dataset]):
        raise ValueError("source lock missing dataset")
    for entry in selected:
        entry = dict(entry)
        entry["path"] = public_source_path(entry)
        entry["resolved_path"] = str(resolve_source(entry, project_root=ROOT))
        yield entry


def source_records(path):
    if path.suffix == ".jsonl":
        with path.open("rb") as source:
            offset = 0
            for index, line in enumerate(source):
                start = offset
                offset += len(line)
                if line.strip():
                    yield json.loads(line), {"record_index": index, "offset_bytes": start, "length_bytes": len(line)}
    else:
        import ijson
        with path.open("rb") as source:
            for index, record in enumerate(ijson.items(source, "item", use_float=True, buf_size=16 * 1024 * 1024)):
                yield record, {"record_index": index, "format": "json-array-ijson"}


def cases(dataset, limit=1, question_limit=2):
    """Yield complete histories. At most one decoded source record is retained per iterator.

    BEAM uses parquet batches of one row; Corpus JSONL uses one line; official
    BABILong JSON arrays use ijson. Peak memory is still at least one full record.
    """
    count = 0
    for entry in source_entries(dataset):
        path = Path(entry["resolved_path"])
        if dataset == "beam-10m":
            import pyarrow.parquet as pq
            iterator = ((record, {"record_index": index, "format": "parquet-batch-one"})
                        for index, batch in enumerate(pq.ParquetFile(path).iter_batches(batch_size=1))
                        for record in batch.to_pylist())
        else:
            iterator = source_records(path)
        for record, locator in iterator:
            locator.update(source_id=entry["id"], path=public_source_path(entry), locked_sha256=entry["sha256"])
            if dataset.startswith("corpusqa"):
                prompt = record["prompt"]
                if not isinstance(prompt, list) or len(prompt) != 2 or prompt[0].get("role") != "system" or prompt[1].get("role") != "user":
                    raise ValueError("unsupported CorpusQA prompt shape; refusing to guess boundaries")
                question = record["question"]
                text = prompt[1]["content"]
                at = text.rfind(question)
                if at < 0:
                    raise ValueError("CorpusQA question not found in prompt")
                # Remove only the exact question; preserve original prefix and suffix.
                context = text[:at]
                tail = text[at:]
                case = {"id": str(record["id"]), "context": context,
                        "protected": [prompt[0]], "questions": [{"id": "q0", "question": question,
                        "tail": tail, "reference": record["answer"], "rubric": None,
                        "category": "corpusqa", "scorer": "corpusqa-fixed-judge-v1"}]}
            elif dataset == "beam-10m":
                messages = []
                for plan in record["chat"]:
                    for batches in plan.values():
                        for batch in batches or []:
                            for turn in batch["turns"]:
                                for message in turn:
                                    messages.append(json.dumps(message, ensure_ascii=False, sort_keys=True) + "\n")
                parsed = ast.literal_eval(record["probing_questions"])
                questions = []
                for category, probes in parsed.items():
                    for index, probe in enumerate(probes):
                        questions.append({"id": f"{category}-{index}", "question": probe["question"],
                            "tail": "\n\nQuestion: " + probe["question"], "reference": probe.get("ideal_response"),
                            "rubric": probe.get("rubric"), "category": category,
                            "scorer": "beam-rubric-fixed-judge-v1"})
                case = {"id": str(record["conversation_id"]), "context": "".join(messages), "protected": [],
                        "questions": questions[:question_limit] if question_limit else questions}
            else:
                case = {"id": str(record.get("id", locator["record_index"])), "context": record["input"],
                        "protected": [{"role": "system", "content": "Answer the question from the context. Return only the short answer, without explanation."}],
                        "questions": [{"id": "q0", "question": record["question"], "tail": "\n\nQuestion: " + record["question"],
                            "reference": record["target"], "rubric": None, "category": dataset.split("-")[1],
                            "scorer": "babilong-official-label-match-7a6efee"}]}
            case.update(dataset=dataset, locator=locator)
            yield case
            count += 1
            if count >= limit:
                return


def source_units(text, max_bytes):
    """UTF-8 safe lossless chunks, bounded by max_bytes; never cut a codepoint."""
    if max_bytes < 4:
        raise ValueError("chunk bytes must be at least four")
    # Slicing characters first avoids a full additional UTF-8 copy of the record.
    position = 0
    while position < len(text):
        piece = text[position:position + max_bytes]
        while len(piece.encode("utf-8")) > max_bytes:
            piece = piece[:max(1, len(piece) // 2)]
        position += len(piece)
        yield piece


def case_metadata(case, chunk_bytes):
    text = case["context"]
    return {"dataset": case["dataset"], "id": case["id"], "locator": case["locator"],
            "context_bytes": len(text.encode()), "context_sha256": hashlib.sha256(text.encode()).hexdigest(),
            "protected": case["protected"], "wb_chunks": sum(1 for _ in source_units(text, chunk_bytes)),
            "questions": case["questions"]}
