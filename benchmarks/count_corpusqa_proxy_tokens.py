"""Count one shared context per domain with cl100k_base as a screening proxy.

This is NOT the gateway model's tokenizer. It makes no model requests.
"""

import json
from pathlib import Path
from source_paths import resolve_source, PROJECT_ROOT

import tiktoken


ROOT = Path(__file__).resolve().parent
REPORTS = json.loads((ROOT / "prepared" / "corpusqa" / "corpusqa-local-verification.json").read_text(encoding="utf-8"))
LOCKED = json.loads((ROOT / "sources-lock.json").read_text(encoding="utf-8"))["files"]
TOKENIZER = tiktoken.get_encoding("cl100k_base")


def main():
    results = []
    for report in REPORTS:
        index = [json.loads(line) for line in (PROJECT_ROOT / report["index"]).open(encoding="utf-8")]
        selected = {}
        for item in index:
            selected.setdefault(item["domain"], item)
        entry = next(item for item in LOCKED if item["path"] == report["source"])
        with resolve_source(entry).open("rb") as source:
            for domain, item in selected.items():
                source.seek(item["offset_bytes"])
                record = json.loads(source.read(item["length_bytes"]))
                count = sum(len(TOKENIZER.encode(message["content"], disallowed_special=())) for message in record["prompt"])
                results.append({"file": Path(report["source"]).name, "domain": domain, "representativeId": item["id"], "cl100kTokens": count, "actualGatewayTokens": None, "sameContextQuestions": sum(x["context_sha256_without_question"] == item["context_sha256_without_question"] for x in index)})
                print(f"{Path(report['source']).name} {domain}: {count:,} cl100k tokens", flush=True)
    (ROOT / "prepared" / "corpusqa" / "proxy-token-counts.json").write_text(json.dumps(results, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
