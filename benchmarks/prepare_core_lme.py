"""Prepare official LME ordered multimodal evidence, never silently flatten to text."""
import argparse,hashlib,json,sys
from pathlib import Path,PurePosixPath
ROOT=Path(__file__).resolve().parents[1]

def prepare(data_root,tier='small',question_id=None):
    data_root=Path(data_root);question=None
    with (data_root/'questions.jsonl').open(encoding='utf-8') as f:
        for line in f:
            q=json.loads(line)
            if question_id is None or q['id']==question_id:question=q;break
    if question is None:raise ValueError('Question not found')
    haystack=data_root/'haystacks'/('lme_v2_'+tier+'.json');members=json.loads(haystack.read_text())[question['id']];wanted=set(members);found={};missing_images=[];image_cache={}
    def image(ref):
        if ref is None:return None
        path=PurePosixPath(ref)
        if path.is_absolute() or '..' in path.parts or ':' in ref or '\\' in ref:raise ValueError('Unsafe screenshot path')
        local=data_root/path
        if not local.is_file():missing_images.append(path.as_posix())
        if path.as_posix() not in image_cache:image_cache[path.as_posix()]={'path':'benchmarks/data/longmemeval-v2/'+path.as_posix(),'relative_path':path.as_posix(),'exists':local.is_file(),**({'bytes':local.stat().st_size,'sha256':hashlib.sha256(local.read_bytes()).hexdigest()} if local.is_file() else {})}
        return dict(image_cache[path.as_posix()])
    with (data_root/'trajectories.jsonl').open('rb') as f:
        offset=0
        for line in f:
            row=json.loads(line);length=len(line)
            if row['id'] in wanted:
                found[row['id']]={'id':row['id'],'record_locator':{'path':'benchmarks/data/longmemeval-v2/trajectories.jsonl','offset_bytes':offset,'length_bytes':length,'sha256':hashlib.sha256(line).hexdigest()},'domain':row['domain'],'goal':row['goal'],'outcome':row['outcome'],'states':[{'state_index':s['state_index'],'observation':s['accessibility_tree'],'action':s['action'],'thought':s.get('thought'),'url':s['url'],'image':image(s['screenshot'])} for s in row['states']]}
            offset+=length
            if len(found)==len(wanted):break
    if set(found)!=wanted:raise ValueError('Missing trajectories')
    question_image=image(question.get('image') or (question['question'].get('image') if isinstance(question['question'],dict) else None))
    return {'schema':'sic-research-task-v2','task_id':'longmemeval-v2-'+tier+':'+question['id'],'goal':question['question'],'media_required':True,'question_image':question_image,'ordered_trajectories':[found[x] for x in members],'references':{'answer':question['answer'],'official_eval_function':question['eval_function']},'missing_images':missing_images,'source':{'revision':'f152293e235517d504809563c833d7190b8c713b','haystack_sha256':hashlib.sha256(haystack.read_bytes()).hexdigest()},'execution_status':'blocked-missing-images' if missing_images else 'ready-for-official-memory-adapter','memory_adapter':'benchmarks.core_lme_memory:SICCoreMemory','research_evaluation':'not_evaluated','not_a_text_only_official_score':True}

def main():
    p=argparse.ArgumentParser();p.add_argument('--data-root',type=Path,default=ROOT/'benchmarks/data/longmemeval-v2');p.add_argument('--tier',choices=['small','medium'],default='small');p.add_argument('--question-id');p.add_argument('--output',type=Path,required=True);a=p.parse_args()
    if a.output.exists():raise ValueError('Do not overwrite task artifact')
    value=prepare(a.data_root,a.tier,a.question_id);a.output.parent.mkdir(parents=True,exist_ok=True);a.output.write_text(json.dumps(value,ensure_ascii=False,indent=2)+'\n',encoding='utf-8');print(json.dumps({'task_id':value['task_id'],'trajectories':len(value['ordered_trajectories']),'missing_images':len(value['missing_images']),'execution_status':value['execution_status']}))
if __name__=='__main__':main()
