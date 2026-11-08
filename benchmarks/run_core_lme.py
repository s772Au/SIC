"""Run pinned official LME harness/submission tools with SIC registered.

No download or publication occurs. Reader/evaluator dispatch remains the
unmodified official harness, using explicit caller CLI model configuration.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import runpy
import importlib
import sys
ROOT=Path(__file__).resolve().parents[1]


def verify_official(root):
    root=Path(root).resolve();lock=json.loads((ROOT/'benchmarks/lme-official-source-lock.json').read_text())
    for name,expected in lock['files'].items():
        path=root/name
        if not path.is_file() or hashlib.sha256(path.read_bytes()).hexdigest()!=expected:raise ValueError('Pinned official source missing or changed: '+name)
    return root


def verify_processor(root):
    root=Path(root).resolve();lock=json.loads((ROOT/'benchmarks/lme-processor-lock.json').read_text())
    for name,expected in lock['files'].items():
        path=root/name
        if not path.is_file() or hashlib.sha256(path.read_bytes()).hexdigest()!=expected:raise ValueError('Pinned official context processor missing or changed: '+name)
    return root


def configure_processor_loader(harness,processor_path):
    """Select locked local assets; official tensor/count/truncation code is unchanged."""
    path=verify_processor(processor_path)
    def get_processor():
        processor=getattr(harness.MEMORY_CONTEXT_PROCESSOR_LOCAL,'processor',None)
        if processor is None:
            import torch
            torch.set_num_threads(2)
            from transformers import AutoProcessor
            processor=AutoProcessor.from_pretrained(path,local_files_only=True)
            harness.MEMORY_CONTEXT_PROCESSOR_LOCAL.processor=processor
        return processor
    harness.get_memory_context_processor=get_processor


def materialize_official_inputs(official,data_root,domain,tier,output_dir,question_ids=None,limit=None):
    official=verify_official(official);sys.path.insert(0,str(official))
    from data.public_data import materialize_runtime_questions,materialize_runtime_haystack
    root=Path(data_root).resolve();output=Path(output_dir)
    if output.exists():raise ValueError('Do not overwrite official runtime preparation')
    output.mkdir(parents=True)
    questions=materialize_runtime_questions(data_root=root,domain=domain,question_ids=question_ids,limit=limit,output_path=output/'questions.json')
    haystack=materialize_runtime_haystack(data_root=root,tier=tier,selected_questions=questions,output_path=output/'haystack.json')
    ordered=list(dict.fromkeys(identity for values in haystack.values() for identity in values));wanted=set(ordered);found={}
    with (root/'trajectories.jsonl').open(encoding='utf-8') as source:
        for line in source:
            row=json.loads(line)
            if row['id'] in wanted:found[row['id']]=line
            if len(found)==len(wanted):break
    if set(found)!=wanted:raise ValueError('Missing official trajectory in selected haystack')
    with (output/'trajectories.jsonl').open('w',encoding='utf-8') as destination:
        for identity in ordered:destination.write(found[identity].rstrip('\n')+'\n')
    report={'questions':str(output/'questions.json'),'haystack':str(output/'haystack.json'),'trajectories':str(output/'trajectories.jsonl'),'question_count':len(questions),'trajectory_count':len(ordered),'question_images':sum(isinstance(q['question'],dict) for q in questions),'official_revision':json.loads((ROOT/'benchmarks/lme-official-source-lock.json').read_text())['revision'],'research_evaluation':'not_evaluated'}
    (output/'preparation.json').write_text(json.dumps(report,ensure_ascii=False,indent=2),encoding='utf-8')
    return report


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--official-root',type=Path,required=True)
    parser.add_argument('--processor-path',type=Path,help='Source-locked local Qwen context processor; required for reader evaluation')
    parser.add_argument('--sic-core-config',type=Path,help='Explicit frozen Core roles/profiles JSON, only needed for model editing')
    parser.add_argument('tool',choices=['prepare-runtime','harness','submission-point','submission-package'])
    args,remaining=parser.parse_known_args()
    official=verify_official(args.official_root);sys.path[:0]=[str(official),str(ROOT)]
    if args.tool=='prepare-runtime':
        preparation=argparse.ArgumentParser();preparation.add_argument('--data-root',type=Path,required=True);preparation.add_argument('--domain',choices=['web','enterprise'],required=True);preparation.add_argument('--tier',choices=['small','medium'],default='small');preparation.add_argument('--output-dir',type=Path,required=True);preparation.add_argument('--question-id',action='append');preparation.add_argument('--limit',type=int);p=preparation.parse_args(remaining)
        print(json.dumps(materialize_official_inputs(official,p.data_root,p.domain,p.tier,p.output_dir,p.question_id,p.limit)));return
    if args.sic_core_config:os.environ['SIC_LME_CORE_CONFIG_PATH']=str(args.sic_core_config.resolve())
    import benchmarks.core_lme_memory # register against the actual official Memory class
    module={'harness':'evaluation.harness','submission-point':'leaderboard.build_submission_step_1_single_operating_point','submission-package':'leaderboard.build_submission_step_2_build_package'}[args.tool]
    if remaining and remaining[0]=='--':remaining=remaining[1:]
    sys.argv=[str(official/module.replace('.','/'))+'.py',*remaining]
    if args.tool=='harness':
        harness=importlib.import_module(module)
        if args.processor_path:configure_processor_loader(harness,args.processor_path)
        elif '--skip-evaluation' not in remaining:raise ValueError('Reader evaluation requires --processor-path with source-locked real multimodal counting assets')
        harness.main()
    else:runpy.run_module(module,run_name='__main__')

if __name__=='__main__':main()
