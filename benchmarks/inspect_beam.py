"""Create a compact BEAM-10M inventory; requires pyarrow."""

import ast
import hashlib
import json
from pathlib import Path
from source_paths import report_path

import pyarrow.parquet as pq


ROOT = Path(__file__).resolve().parent
DATA = ROOT / "data" / "beam10m" / "data"
OUTPUT = ROOT / "prepared" / "beam10m"


def main():
    OUTPUT.mkdir(parents=True, exist_ok=True)
    inventory = []
    sources = []
    for file in sorted(DATA.glob("*.parquet")):
        digest = hashlib.sha256()
        with file.open("rb") as handle:
            for chunk in iter(lambda: handle.read(8 * 1024 * 1024), b""):
                digest.update(chunk)
        parquet = pq.ParquetFile(file)
        sources.append({"file": report_path(file), "size_bytes": file.stat().st_size, "sha256": digest.hexdigest(), "rows": parquet.metadata.num_rows})
        for group in range(parquet.num_row_groups):
            for row_index, record in enumerate(parquet.read_row_group(group).to_pylist()):
                messages = 0
                chars = 0
                for plan in record["chat"]:
                    for batches in plan.values():
                        if not batches:
                            continue
                        for batch in batches:
                            for turn in batch["turns"]:
                                for item in turn:
                                    messages += 1
                                    chars += len(item["content"])
                questions = ast.literal_eval(record["probing_questions"])
                categories = {key: len(value) for key, value in questions.items()}
                inventory.append({
                    "conversation_id": str(record["conversation_id"]),
                    "source_file": report_path(file),
                    "row_group": group,
                    "row_in_group": row_index,
                    "messages": messages,
                    "content_chars": chars,
                    "question_count": sum(categories.values()),
                    "question_categories": categories,
                    "token_count_status": "official ~10M; exact gateway tokenizer unverified",
                })
    (OUTPUT / "inventory.json").write_text(json.dumps({"sources": sources, "conversations": inventory}, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(f"{len(inventory)} conversations, {sum(row['question_count'] for row in inventory)} questions")


if __name__ == "__main__":
    main()
