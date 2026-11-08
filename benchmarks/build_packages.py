"""Build small control-plane ZIPs; source corpora remain mounted by path.

No gateway, model, evaluation or scoring calls are made.
"""

import json
import zipfile
from pathlib import Path
from source_paths import report_path


ROOT = Path(__file__).resolve().parent
PACKAGES = ROOT / "packages"
COMMON = [
    ROOT / "test-catalog.json",
    ROOT / "sources-lock.json",
    ROOT / "source_paths.py",
    ROOT / "SOURCE_PATHS.md",
    ROOT / "source-overrides.example.json",
    ROOT / "estimate_plan.py",
    ROOT / "inspect_corpusqa.py",
    ROOT / "inspect_beam.py",
    ROOT / "verify_sources.py",
    ROOT / "download_optional.py",
    ROOT / "download_large_http.py",
    ROOT / "count_corpusqa_proxy_tokens.py",
    ROOT / "prepare_babilong_sample.py",
    ROOT / "prepared" / "corpusqa" / "corpusqa-local-verification.json",
    ROOT / "prepared" / "corpusqa" / "proxy-token-counts.json",
    ROOT / "prepared" / "corpusqa" / "4m_4domains.index.jsonl",
    ROOT / "prepared" / "corpusqa" / "10m_4domains.index.jsonl",
    ROOT / "prepared" / "beam10m" / "inventory.json",
    ROOT / "prepared" / "babilong" / "qa3-10m-sample-inventory.json",
    ROOT / "prepared" / "babilong" / "full-inventory.json",
    ROOT / "prepared" / "longmemeval-v2" / "text-inventory.json",
    ROOT / "prepared" / "longmemeval-v2" / "runtime-validation.json",
    ROOT / "prepared" / "source-verification.json",
    ROOT / "prepared" / "data-licenses.json",
    ROOT / "vendor" / "babilong_metrics.py",
    ROOT / "vendor" / "BABILONG-LICENSE",
    ROOT / "vendor" / "sources.json",
]


def main():
    for tier in ("low-resource", "high-resource"):
        archive = PACKAGES / f"sic-{tier}.zip"
        contents = COMMON + [path for path in (PACKAGES / tier).rglob("*") if path.is_file()]
        contents.append(PACKAGES / "README.md")
        with zipfile.ZipFile(archive, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=6) as output:
            for path in contents:
                if not path.is_file():
                    raise FileNotFoundError(path)
                output.write(path, path.relative_to(ROOT))
        print(json.dumps({"package": report_path(archive), "bytes": archive.stat().st_size, "files": len(contents)}))


if __name__ == "__main__":
    main()
