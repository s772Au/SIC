"""Fetch optional official benchmark data on demand; never calls the model gateway."""

import argparse
from pathlib import Path
from source_paths import report_path

from huggingface_hub import hf_hub_download, snapshot_download


ROOT = Path(__file__).resolve().parent
REVISIONS = {
    "RMT-team/babilong": "ee0d588794c7ac098062ee0d247c733d62e94fe2",
    "xiaowu0162/longmemeval-v2": "f152293e235517d504809563c833d7190b8c713b",
}


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("dataset", choices=["babilong-qa3", "babilong-full", "longmemeval-v2-full"])
    args = parser.parse_args()
    if args.dataset == "babilong-qa3":
        files = ["data/qa3/10M.json"]
        repo = "RMT-team/babilong"
        destination = ROOT / "data" / "babilong"
    elif args.dataset == "babilong-full":
        files = [f"data/qa{task}/10M.json" for task in range(1, 6)]
        repo = "RMT-team/babilong"
        destination = ROOT / "data" / "babilong"
    else:
        files = ["trajectories.jsonl", "trajectory_screenshots/*", "question_screenshots/*", "questions.jsonl", "haystacks/*.json", "SCHEMA.md", "DATA_CARD.md", "checksums.sha256", "LICENSE", "README.md"]
        repo = "xiaowu0162/longmemeval-v2"
        destination = ROOT / "data" / "longmemeval-v2"
    destination.mkdir(parents=True, exist_ok=True)
    if args.dataset == "babilong-qa3":
        hf_hub_download(repo_id=repo, repo_type="dataset", revision=REVISIONS[repo],
                        filename=files[0], local_dir=destination, token=False)
    else:
        snapshot_download(repo_id=repo, repo_type="dataset", revision=REVISIONS[repo],
                          allow_patterns=files, local_dir=destination, max_workers=2, token=False)
    print(f"Download returned for {args.dataset} under {report_path(destination)}")
    print("Download returned; verify full bytes before marking data ready: python benchmarks/verify_sources.py --hash --all-locked")


if __name__ == "__main__":
    main()
