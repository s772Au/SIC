"""Independent manifest/accounting validator and task/annotation scorer; no model calls."""
import argparse,hashlib,json,sys,zipfile
from pathlib import Path,PurePosixPath
ROOT=Path(__file__).resolve().parents[1];sys.path[:0]=[str(ROOT),str(ROOT/'scripts')]
from sic_core.accounting import summarize_records,compare_workflows
from research_scoring import score_answer,score_annotations,score_task_rubric,digest_text

def read(p):return json.loads(p.read_text(encoding='utf-8-sig'))
def verify(folder):
    folder=Path(folder).resolve();manifest=read(folder/'manifest.json')
    if manifest['schema']!='sic-compression-research-run-v2':raise ValueError('Unsupported schema')
    for name,spec in manifest['files'].items():
        rel=PurePosixPath(name)
        if rel.is_absolute() or '..' in rel.parts or ':' in name or '\\' in name:raise ValueError('Unsafe manifest path')
        p=(folder/name).resolve();p.relative_to(folder)
        if not p.is_file() or p.stat().st_size!=spec['bytes'] or hashlib.sha256(p.read_bytes()).hexdigest()!=spec['sha256']:raise ValueError('Artifact hash mismatch: '+name)
    for name in ('task.json','configuration.json','code-snapshot.zip'):
        if name not in manifest['files']:raise ValueError('Frozen input missing')
    with zipfile.ZipFile(folder/'code-snapshot.zip') as z:
        if z.testzip() is not None:raise ValueError('Snapshot CRC failure')
        if any('.local/' in n or '..' in PurePosixPath(n).parts for n in z.namelist()):raise ValueError('Private/unsafe snapshot')
    if manifest['status']!='passed':return {'status':'valid-failure-artifact','quality':'not_evaluated'}
    config=read(folder/'configuration.json');counts={}
    if manifest['mode'] not in ('owned_fixture','fixed_provider') or config['mode']!=manifest['mode']:raise ValueError('Run mode mismatch')
    with zipfile.ZipFile(folder/'code-snapshot.zip') as snapshot:
        if hashlib.sha256(snapshot.read('research/compression/pack/control-pack.json')).hexdigest()!=config['pack_sha256']:raise ValueError('Control pack freeze mismatch')
    for engine in config['engines']:
        ledger=read(folder/engine/'ledger.json');summary=read(folder/engine/'summary.json');result=read(folder/engine/'result.json')
        independent=summarize_records(ledger['calls'],ledger['resources'],ledger['task_interval'],ledger['prices'],ledger['events'])
        if summary!=independent:raise ValueError('Accounting recomputation mismatch')
        n=sum(c['role']=='editor' for c in ledger['calls']);counts[engine]=n
        if engine in ('program_exact','langchain_trim','unchanged') and n:raise ValueError('Non-LLM editing control invoked Editor model')
        if result.get('endpoint_policy')!='retain-final-question-and-final-answer-unedited; no post-answer compression':raise ValueError('Final endpoint policy missing')
        consumption=result.get('final_consumption',{})
        if consumption.get('expected_active_cb_ids') and not set(consumption.get('actually_visible_cb_ids',[])).intersection(consumption['expected_active_cb_ids']):raise ValueError('Final Worker did not consume edited view')
        request=read(folder/engine/'final-request.json')
        if request['request'] is None or request.get('cut_id')!=consumption.get('cut_id'):raise ValueError('Final immutable request evidence mismatch')
        if read(folder/'task.json').get('media_required'):
            from sic_core.media import image_info
            hashes=[image_info(b)['sha256'] for m in request['request']['messages'] if isinstance(m.get('content'),list) for b in m['content'] if b.get('type')=='image_url']
            if hashes!=consumption.get('actual_image_sha256') or not set(consumption.get('expected_image_sha256',[]))<=set(hashes):raise ValueError('Final image preservation evidence mismatch')
        workers=sum(c['role']=='worker' for c in ledger['calls'])
        if workers!=len(result['worker_steps'])+1:raise ValueError('Missing accounted final Worker call')
        if not any(t['operation']=='provider.continue' and t['response'].get('ok') and t['response']['result']['request_id']==consumption.get('request_id') for t in read(folder/engine/'trace.json')):raise ValueError('Final request has no actual invocation')
        traces=read(folder/engine/'trace.json');snapshots=[t['response']['result']['view']['cbs'] for t in traces if t['operation']=='cut.get' and t['response'].get('ok')]
        for snapshot in snapshots:
            if snapshot!=result['fixed_cbs'][:len(snapshot)]:raise ValueError('Immutable CB history differs')
        if result['quality']['status']!='not_evaluated':raise ValueError('Runner cannot self-grade task quality')
    comparison=read(folder/'comparison.json');by_engine={r['engine']:r['summary'] for r in comparison['arms']};anchor=comparison.get('reference_engine')
    if anchor:
        for engine,pair in comparison.get('independent_accounting_pairs',{}).items():
            if pair!=compare_workflows(by_engine[anchor],by_engine[engine]):raise ValueError('Paired accounting mismatch')
    return {'status':'passed','files_checked':len(manifest['files']),'accounting_recomputed':True,'editor_calls':counts,'model_quality_inferred':False}

def score(folder,annotations=None):
    folder=Path(folder);valid=verify(folder);m=read(folder/'manifest.json');config=read(folder/'configuration.json');task=read(folder/'task.json');results={}
    for item in (annotations or {}).values():
        for criterion in item.get('criteria',[]):
            for evidence in criterion.get('evidence',[]):
                if evidence.get('artifact') not in m['files'] or not evidence.get('locator'):raise ValueError('Task evidence must locate a verified run artifact')
    if valid['status']!='passed':return {'schema':'sic-research-score-v2','artifact_validation':valid,'quality':{'status':'not_evaluated'}}
    for engine in config['engines']:
        result=read(folder/engine/'result.json');last=result.get('final_answer')
        fixture=m['mode']=='owned_fixture' or any(str(s.get('raw_response',{}).get('id','')).startswith('owned-experiment-fixture') for s in result['worker_steps'])
        quality=score_answer(last,task.get('references',{}).get('answer'),task.get('scoring',{}).get('kind','human_rubric'),fixture)
        if task.get('scoring',{}).get('kind')!='normalized_exact':
            quality=score_task_rubric(last,(annotations or {}).get(engine+':task'),fixture)
        noise=[]
        for i,edit in enumerate(result['edits']):
            material=edit['candidate'];candidate=material.get('body','') if isinstance(material,dict) else str(material)
            key=engine+':'+str(i);noise.append({'edit':i,'level':edit['level'],'source_length':len(edit['source_text']),'candidate_length':len(candidate),'assessment':score_annotations(edit['source_text'],candidate,(annotations or {}).get(key),fixture)})
        results[engine]={'quality':quality,'editing_noise':noise}
    return {'schema':'sic-research-score-v2','artifact_validation':valid,'scorer':'independent-reference-human-spans-v2','arms':results,'research_evaluation':'not_evaluated' if m['mode']=='owned_fixture' or all(v['quality']['status']=='not_evaluated' for v in results.values()) else 'partial-independent-scoring','model_calls':0,'rubric_sha256':hashlib.sha256((ROOT/'research/compression/pack/rubric.json').read_bytes()).hexdigest()}

def main():
    p=argparse.ArgumentParser();p.add_argument('run',type=Path);p.add_argument('--annotations',type=Path);p.add_argument('--annotation-template',type=Path);p.add_argument('--output',type=Path);a=p.parse_args()
    if a.annotation_template:
        verify(a.run);template={}
        for engine in read(a.run/'configuration.json')['engines']:
            result=read(a.run/engine/'result.json');answer=result.get('final_answer')
            template[engine+':task']={'reviewer':'','answer_sha256':digest_text(answer or ''),'criteria':[{'criterion':name,'status':'unknown','rationale':'not yet reviewed','evidence':[]} for name in ('fact_retrieval','decision_continuation','cross_level_traceability','collaboration','resource_tradeoff')]}
            for i,edit in enumerate(read(a.run/engine/'result.json')['edits']):
                candidate=edit['candidate'].get('body','');template[engine+':'+str(i)]={'source_sha256':digest_text(edit['source_text']),'candidate_sha256':digest_text(candidate),'reviewer':'','coverage':'partial','source':edit['source_text'],'candidate':candidate,'claims':[],'format_checks':[]}
        out=a.annotation_template;value={'schema':'sic-noise-annotations-v2','items':template,'note':'Add independent spans/evidence/rationale. Fixture remains unscored even when annotated.'}
    else:
        value=score(a.run,read(a.annotations)['items'] if a.annotations else None);out=a.output
    if out:
        if out.exists():raise ValueError('Never overwrite existing scores/annotations')
        out.write_text(json.dumps(value,ensure_ascii=False,indent=2)+'\n',encoding='utf-8')
    else:print(json.dumps(value,ensure_ascii=False))
if __name__=='__main__':main()
