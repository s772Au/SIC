"""New-Core dataset tasks; reuse public source lock and private resolver, never old results."""
import hashlib
import importlib.util
import json
from pathlib import Path
import sys
ROOT=Path(__file__).resolve().parents[1]
sys.path.insert(0,str(ROOT/'scripts'))
from dataset_adapters import cases,source_units
spec=importlib.util.spec_from_file_location('research_source_paths',ROOT/'benchmarks/source_paths.py');paths=importlib.util.module_from_spec(spec);spec.loader.exec_module(paths)
VERSION='sic-core-task-adapters-v2'


def tasks(dataset,limit=1,chunk_bytes=32768,question_limit=2):
    for case in cases(dataset,limit=limit,question_limit=question_limit):
        for q in case['questions']:
            yield {'schema':'sic-research-task-v2','task_id':dataset+':'+case['id']+':'+q['id'],
                   'goal':q.get('tail',q['question']),'question':q['question'],'blocks':list(source_units(case['context'],chunk_bytes)),
                   'protected':case['protected'],'source':case['locator'],
                   'source_content_sha256':hashlib.sha256(case['context'].encode()).hexdigest(),
                   'references':{'answer':q['reference'],'rubric':q['rubric'],'category':q['category'],'declared_upstream_scorer':q['scorer']},
                   'scoring':{'kind':'normalized_exact' if dataset.startswith('babilong') else 'human_rubric','official_score':False},
                   'block_policy':{'unit':'utf8_bytes','limit':chunk_bytes,'lossless':True},'adapter':VERSION}


def readiness():
    lock_path=ROOT/'benchmarks/sources-lock.json';lock=json.loads(lock_path.read_text(encoding='utf-8'))
    prior=json.loads((ROOT/'benchmarks/prepared/source-verification.json').read_text(encoding='utf-8'))
    evidence={x['dataset']:x for x in prior['files']};rows=[]
    for entry in lock['files']:
        path=paths.resolve_source(entry,ROOT);old=evidence.get(entry['id'],{})
        exists=path.is_file();size=path.stat().st_size if exists else None
        rows.append({'id':entry['id'],'path':paths.public_source_path(entry),'exists':exists,'bytes':size,
                     'size_matches_lock':size==entry['bytes'],'sha256':entry['sha256'],
                     'prior_hash_verified':old.get('hashOk') is True,'hash_verification_date':prior.get('sourceVerificationDate'),
                     'current_hash_status':'prior_verified_current_size_matches' if old.get('hashOk') and size==entry['bytes'] else 'not_verified'})
    return {'schema':'sic-research-data-readiness-v2','sources_lock_sha256':hashlib.sha256(lock_path.read_bytes()).hexdigest(),
            'files':rows,'full_rehash_performed':False,'no_private_paths':True,
            'text_adapters':['corpusqa-4m','corpusqa-10m','beam-10m','babilong-qa3-sample',*[f'babilong-qa{i}' for i in range(1,6)]],
            'limitations':{'longmemeval_multimodal':'native screenshot carrier and official Memory/harness wrapper available separately; use prepared media task plus explicit data root or pinned official wrapper, never text flattening; real task quality not_evaluated'}}
