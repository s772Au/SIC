"""Extract a reproducible qa3 subset from the official 10M JSON stream.

Requires ijson. The official source may be a complete file or a downloaded
prefix containing the requested number of complete items.
"""

import argparse
import hashlib
import json
import os
from pathlib import Path
from source_paths import report_path

import ijson


ROOT = Path(__file__).resolve().parent


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--count", type=int, default=20)
    args = parser.parse_args()
    if not 1 <= args.count <= 100:
        parser.error("count must be 1..100")
    full = ROOT / "data" / "babilong" / "data" / "qa3" / "10M.json"
    partial = full.with_suffix(".json.part")
    source = full if full.is_file() else partial
    if not source.is_file():
        raise FileNotFoundError(source)
    output_dir = ROOT / "data" / "babilong"
    output_dir.mkdir(parents=True, exist_ok=True)
    output = output_dir / f"qa3-10m-first{args.count}.jsonl"
    staging = output.with_suffix(output.suffix + ".preparing")
    inventory = []
    digest = hashlib.sha256()
    with source.open("rb") as raw, staging.open("wb") as prepared:
        # Large source strings make the small default parser buffer expensive.
        for number, item in enumerate(ijson.items(raw, "item", buf_size=16 * 1024 * 1024), 1):
            if not all(isinstance(item.get(key), str) for key in ("input", "question", "target")):
                raise ValueError(f"invalid fields in qa3 item {number}")
            record = {"id": f"qa3-10M-{number:03d}", **item}
            line = (json.dumps(record, ensure_ascii=False) + "\n").encode("utf-8")
            prepared.write(line)
            digest.update(line)
            inventory.append({"id": record["id"], "inputChars": len(item["input"]), "questionChars": len(item["question"]), "targetChars": len(item["target"]), "lineBytes": len(line)})
            if number % 5 == 0:
                print(f"prepared {number}/{args.count} qa3 items", flush=True)
            if number == args.count:
                break
    if len(inventory) != args.count:
        staging.unlink(missing_ok=True)
        raise ValueError(f"source contains only {len(inventory)} complete items")
    os.replace(staging, output)
    prepared_dir = ROOT / "prepared" / "babilong"
    prepared_dir.mkdir(parents=True, exist_ok=True)
    report = {
        "source": report_path(source),
        "sourceIsCompleteOfficialFile": source == full,
        "officialSourceUrl": "https://huggingface.co/datasets/RMT-team/babilong/resolve/ee0d588794c7ac098062ee0d247c733d62e94fe2/data/qa3/10M.json",
        "officialFullSourceSha256": "6a3d703f32ba18fa8d562b8581f025ddc49699817a3a6633bbeb9cc356c7d434",
        "output": report_path(output),
        "outputBytes": output.stat().st_size,
        "outputSha256": digest.hexdigest(),
        "items": inventory,
        "tokenCountStatus": "official 10M length; exact gateway tokenizer unverified",
    }
    (prepared_dir / "qa3-10m-sample-inventory.json").write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(f"{report['output']}: {report['outputBytes']:,} bytes; sha256={report['outputSha256']}")


if __name__ == "__main__":
    main()
