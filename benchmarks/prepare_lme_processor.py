"""Fetch only the pinned official context processor, never model weights."""
import argparse,json,sys
from pathlib import Path
from huggingface_hub import snapshot_download
ROOT=Path(__file__).resolve().parents[1]
sys.path.insert(0,str(ROOT))
from benchmarks.run_core_lme import verify_processor


def prepare(output):
    lock=json.loads((ROOT/'benchmarks/lme-processor-lock.json').read_text())
    snapshot_download(lock['repository'],revision=lock['revision'],allow_patterns=list(lock['files']),local_dir=output)
    return verify_processor(output)


def main():
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('--output',type=Path,required=True);args=parser.parse_args()
    path=prepare(args.output);print(json.dumps({'processor_path':str(path),'model_weights_downloaded':False}))

if __name__=='__main__':main()
