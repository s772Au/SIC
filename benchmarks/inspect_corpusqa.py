"""Inspect local CorpusQA JSONL without copying the large prompts.

Writes a small per-question index. It makes no model or gateway requests.
"""

import argparse
import collections
import hashlib
import json
from pathlib import Path
from source_paths import public_path_for_local_source, report_path


def inspect(path: Path, out: Path):
    digest = hashlib.sha256()
    counts = collections.Counter()
    lengths = []
    ids = set()
    with path.open("rb") as source, out.open("w", encoding="utf-8") as target:
        offset = 0
        for number, raw in enumerate(source, 1):
            digest.update(raw)
            size = len(raw)
            if not raw.strip():
                offset += size
                continue
            record = json.loads(raw)
            identifier = record["id"]
            if identifier in ids:
                raise ValueError(f"duplicate id {identifier} in {path.name}")
            ids.add(identifier)
            prompt = record["prompt"]
            if not isinstance(prompt, list) or not prompt:
                raise ValueError(f"missing prompt in {identifier}")
            chars = sum(len(m.get("content", "")) for m in prompt)
            if not record.get("question") or "answer" not in record:
                raise ValueError(f"missing question/answer in {identifier}")
            question_at = prompt[-1]["content"].rfind(record["question"])
            if question_at < 0:
                raise ValueError(f"question not present in prompt for {identifier}")
            shared_context = prompt[-1]["content"][:question_at] + prompt[-1]["content"][question_at + len(record["question"]):]
            item = {
                "id": identifier,
                "domain": record.get("domain"),
                "set": record.get("set"),
                "type": record.get("type"),
                "offset_bytes": offset,
                "length_bytes": size,
                "prompt_chars": chars,
                "doc_count": len(record.get("doc_files", [])),
                "doc_files_sha256": hashlib.sha256(json.dumps(record.get("doc_files", []), ensure_ascii=False).encode()).hexdigest(),
                "context_sha256_without_question": hashlib.sha256(shared_context.encode()).hexdigest(),
                "question_chars": len(record["question"]),
                "answer_type": type(record["answer"]).__name__,
            }
            target.write(json.dumps(item, ensure_ascii=False) + "\n")
            counts[(item["domain"], item["set"])] += 1
            lengths.append(chars)
            offset += size
    if not lengths:
        raise ValueError(f"empty dataset: {path.name}")
    return {
        "source": public_path_for_local_source(path),
        "sha256": digest.hexdigest(),
        "size_bytes": path.stat().st_size,
        "rows": len(lengths),
        "prompt_chars_min": min(lengths),
        "prompt_chars_max": max(lengths),
        "prompt_chars_mean": round(sum(lengths) / len(lengths)),
        "domain_set_counts": [
            {"domain": domain, "set": dataset_set, "rows": count}
            for (domain, dataset_set), count in sorted(counts.items())
        ],
        "index": report_path(out),
        "token_count_status": "unverified; char count is not tokenizer count",
    }


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("sources", nargs="+", type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    reports = []
    for source in args.sources:
        report = inspect(source, args.output / (source.stem + ".index.jsonl"))
        reports.append(report)
        print(f"{source.name}: {report['rows']} rows, {report['sha256']}", flush=True)
    (args.output / "corpusqa-local-verification.json").write_text(
        json.dumps(reports, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
    )


if __name__ == "__main__":
    main()
