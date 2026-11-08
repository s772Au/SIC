"""Independent human/span or reference scoring, with fixture quality guard."""
import hashlib,json,re,unicodedata
from pathlib import Path
NOISE=('unsupported_fact','source_distortion','format_drift')

def digest_text(text):return hashlib.sha256(text.encode()).hexdigest()
def normalized(text):return ' '.join(re.sub(r'[^\w\s]',' ',unicodedata.normalize('NFKC',str(text)).casefold()).split())

def score_answer(answer,reference,mode='human_rubric',fixture=False):
    if fixture:return {'status':'not_evaluated','score':None,'reason':'owned fixture is not quality evidence'}
    if answer is None or not str(answer).strip():return {'status':'unknown','score':None,'reason':'missing answer'}
    if reference is None:return {'status':'unknown','score':None,'reason':'missing reference'}
    if mode!='normalized_exact':return {'status':'not_evaluated','score':None,'reason':'independent human rubric required'}
    refs=reference if isinstance(reference,list) else [reference]
    refs=[r for r in refs if isinstance(r,str) and normalized(r)]
    if not refs:return {'status':'unknown','score':None,'reason':'reference has no evaluable label'}
    return {'status':'scored','score':int(any(normalized(answer)==normalized(x) for x in refs)),'method':'local-normalized-exact-v2','official_score':False}

def score_annotations(source,candidate,annotations,fixture=False):
    if fixture:return {'status':'not_evaluated','noise':{k:None for k in NOISE},'reason':'fixture output remains unscored'}
    if annotations is None:return {'status':'unknown','noise':{k:None for k in NOISE},'reason':'human annotations absent'}
    if annotations.get('source_sha256')!=digest_text(source) or annotations.get('candidate_sha256')!=digest_text(candidate):raise ValueError('Annotation source/candidate digest mismatch')
    if not annotations.get('reviewer') or annotations.get('coverage') not in ('partial','exhaustive'):raise ValueError('Independent reviewer and coverage required')
    counts={k:0 for k in NOISE};denominator=0;suggestions=0
    for item in annotations.get('claims',[]):
        a,b=item['span'];labels=item['labels']
        if not 0<=a<b<=len(candidate) or not labels or set(labels)-{'supported','unsupported_fact','source_distortion','suggestion','unknown'}:raise ValueError('Invalid candidate span/claim label')
        if not item.get('rationale'):raise ValueError('Claim rationale required')
        if any(k in labels for k in ('supported','source_distortion')) and not item.get('evidence'):raise ValueError('Source-grounded label requires source evidence')
        for evidence in item.get('evidence',[]):
            x,y=evidence['span']
            if not 0<=x<y<=len(source):raise ValueError('Invalid source evidence span')
        if 'suggestion' in labels:suggestions+=1;continue
        if 'unknown' in labels:continue
        denominator+=1
        for k in NOISE[:2]:counts[k]+=k in labels
    checks=annotations.get('format_checks',[])
    if any(c.get('status') not in ('pass','fail','unknown') or not c.get('rule') or not c.get('rationale') for c in checks):raise ValueError('Invalid format check')
    checked=[c for c in checks if c['status']!='unknown'];counts['format_drift']=sum(c['status']=='fail' for c in checked)
    return {'status':'annotated' if annotations['coverage']=='exhaustive' else 'partial','noise':{k:{'count':counts[k],'denominator':len(checked) if k=='format_drift' else denominator,'rate':counts[k]/(len(checked) if k=='format_drift' else denominator) if (len(checked) if k=='format_drift' else denominator) else None} for k in NOISE},'legal_suggestions':suggestions,'unknown_claims':sum('unknown' in x['labels'] for x in annotations.get('claims',[])),'coverage':annotations['coverage'],'not_automatic_semantic_ground_truth':True}


def score_task_rubric(answer,annotation,fixture=False):
    if fixture:return {'status':'not_evaluated','score':None,'reason':'fixture is not task quality evidence'}
    if not answer or annotation is None:return {'status':'unknown','score':None,'reason':'answer or independent criterion annotations missing'}
    if not annotation.get('reviewer') or annotation.get('answer_sha256')!=digest_text(answer):raise ValueError('Task reviewer or answer binding invalid')
    allowed={'fact_retrieval','decision_continuation','cross_level_traceability','collaboration','resource_tradeoff'}
    items=annotation.get('criteria',[])
    if not items or any(x.get('criterion') not in allowed or x.get('status') not in ('pass','fail','unknown','not_applicable') or not x.get('rationale') for x in items):raise ValueError('Invalid independent task criteria')
    if len({x['criterion'] for x in items})!=len(items):raise ValueError('Duplicate criterion')
    for item in items:
        if item['status'] in ('pass','fail') and not item.get('evidence'):raise ValueError('Judged criterion requires evidence')
    evaluated=[x for x in items if x['status'] in ('pass','fail')]
    return {'status':('partial' if evaluated else 'unknown') if any(x['status']=='unknown' for x in items) else ('scored' if evaluated else 'not_applicable'),'criteria':items,'score':sum(x['status']=='pass' for x in evaluated)/len(evaluated) if evaluated else None,'denominator':len(evaluated),'unknown':sum(x['status']=='unknown' for x in items),'official_score':False}
