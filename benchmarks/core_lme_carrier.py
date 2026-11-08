"""Lossless portable official LME-to-Core image carrier; no official SDK dependency."""
import base64,hashlib,json,mimetypes
from pathlib import Path,PurePosixPath
from sic_core import media

OFFICIAL_REVISION='2cc8c540bdb87fe6761629b585e727e1c4704520'
DATA_REVISION='f152293e235517d504809563c833d7190b8c713b'


def screenshot_block(data_root,ref,detail='high'):
    if not isinstance(ref,str) or not ref:raise ValueError('Screenshot reference must be a nonempty relative path')
    relative=PurePosixPath(ref)
    if relative.is_absolute() or '..' in relative.parts or ':' in ref or '\\' in ref:raise ValueError('Unsafe screenshot reference')
    root=Path(data_root).resolve();path=(root/relative).resolve()
    if not path.is_relative_to(root) or not path.is_file():raise ValueError('Screenshot missing or outside declared dataset root')
    mime=mimetypes.guess_type(path.name)[0];block={'type':'image_url','image_url':{'url':'data:'+str(mime)+';base64,'+base64.b64encode(path.read_bytes()).decode(),'detail':detail}}
    media.image_info(block)
    return block


def trajectory_request(trajectory,data_root,detail='high'):
    if not isinstance(trajectory,dict) or not isinstance(trajectory.get('id'),str) or not isinstance(trajectory.get('states'),list):raise ValueError('Official trajectory requires id and ordered states')
    messages=[{'role':'user','content':json.dumps({'trajectory_id':trajectory['id'],'domain':trajectory.get('domain'),'goal':trajectory.get('goal'),'outcome':trajectory.get('outcome')},ensure_ascii=False)}]
    previous=-1
    for state in trajectory['states']:
        index=state['state_index']
        if not isinstance(index,int) or index<=previous:raise ValueError('Trajectory states must retain strict original state order')
        previous=index
        content=[{'type':'text','text':json.dumps({key:state.get(key) for key in ('state_index','url','accessibility_tree','action','thought')},ensure_ascii=False)}]
        if state.get('screenshot') is not None:content.append(screenshot_block(data_root,state['screenshot'],detail))
        messages.append({'role':'user','content':content})
    return {'messages':messages}


def load_prepared_trajectories(task,data_root):
    """Read exact locked raw records; never pass question/gold/evaluator to Memory."""
    root=Path(data_root);tier,question=task['task_id'].removeprefix('longmemeval-v2-').split(':',1)
    if tier not in ('small','medium') or task['source']['revision']!=DATA_REVISION:raise ValueError('Prepared official task source mismatch')
    haystack=root/'haystacks'/('lme_v2_'+tier+'.json')
    if hashlib.sha256(haystack.read_bytes()).hexdigest()!=task['source']['haystack_sha256']:raise ValueError('Haystack changed after task preparation')
    members=json.loads(haystack.read_text(encoding='utf-8'))[question]
    if members!=[x['id'] for x in task['ordered_trajectories']]:raise ValueError('Prepared trajectory order differs from official haystack')
    with (root/'trajectories.jsonl').open('rb') as handle:
        for item in task['ordered_trajectories']:
            locator=item['record_locator']
            if locator['path']!='benchmarks/data/longmemeval-v2/trajectories.jsonl':raise ValueError('Unexpected raw trajectory path')
            handle.seek(locator['offset_bytes']);raw=handle.read(locator['length_bytes'])
            if hashlib.sha256(raw).hexdigest()!=locator['sha256']:raise ValueError('Raw trajectory changed after task preparation')
            trajectory=json.loads(raw)
            if trajectory['id']!=item['id']:raise ValueError('Trajectory locator identity mismatch')
            for state in item['states']:
                screenshot=state.get('image')
                if screenshot is not None:
                    block=screenshot_block(root,screenshot['relative_path'])
                    if media.image_info(block)['sha256']!=screenshot['sha256']:raise ValueError('Screenshot changed after task preparation')
            yield trajectory



def question_request(task,data_root,detail='high'):
    goal=task.get('goal')
    if isinstance(goal,dict):goal=goal.get('text')
    if not isinstance(goal,str):raise ValueError('Prepared task goal must be question text')
    content=[{'type':'text','text':goal}]
    screenshot=task.get('question_image')
    if screenshot is not None:
        block=screenshot_block(data_root,screenshot['relative_path'],detail)
        if media.image_info(block)['sha256']!=screenshot['sha256']:raise ValueError('Question screenshot changed after preparation')
        content.append(block)
    return {'messages':[{'role':'user','content':content}]}
