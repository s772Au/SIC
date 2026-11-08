"""Sourced four-view decision workspace. Events are facts asserted by a host/Editor, not inferred task quality."""
import copy
from .adapters import digest
VIEWS=('Working on','Failed Before','Not Tried Yet','Overlooked')
KINDS={'work':'Working on','failure':'Failed Before','candidate':'Not Tried Yet','overlooked':'Overlooked'}

def empty():return {'revision':0,'views':{name:[] for name in VIEWS},'discovery_prompt':'What have I missed?','event_ids':[]}

def apply(current,events):
    value=copy.deepcopy(current)
    if not events:return value
    for event in events:
        if not isinstance(event,dict) or event.get('kind') not in (*KINDS,'complete'):raise ValueError('unknown decision event kind')
        if not isinstance(event.get('text'),str) or not isinstance(event.get('evidence',[]),list):raise ValueError('decision text/evidence required')
        event_id=event.get('id') or digest(event)
        if event_id in value['event_ids']:continue
        if event['kind']=='complete':
            target=event.get('target')
            for item in value['views']['Working on']:
                if item['id']==target:item['status']='completed';item['completion']=event['text'];item['completion_evidence']=event.get('evidence',[])
        else:
            row={'id':event_id,'text':event['text'],'evidence':copy.deepcopy(event.get('evidence',[])),'status':'active' if event['kind']=='work' else 'recorded'};value['views'][KINDS[event['kind']]].append(row)
        value['event_ids'].append(event_id)
    value['revision']+=1;return value


def from_editor(raw,current):
    if not isinstance(raw,dict):raise ValueError('decision_tree must be a four-view object')
    views=raw.get('views',raw)
    if not isinstance(views,dict):raise ValueError('decision views must be an object')
    aliases={'working_on':'Working on','failed_before':'Failed Before','not_tried_yet':'Not Tried Yet','overlooked':'Overlooked'};canonical={aliases.get(key,key):value for key,value in views.items()}
    if set(canonical)-set(VIEWS):raise ValueError('unknown decision view')
    result=copy.deepcopy(current)
    for key,items in canonical.items():
        result['views'][key]=[]
        if isinstance(items,str):items=[{'text':items,'evidence':[]}]
        if not isinstance(items,list):raise ValueError('decision view entries must be arrays')
        for item in items:
            if isinstance(item,str):item={'text':item,'evidence':[]}
            if not isinstance(item,dict) or not isinstance(item.get('text'),str) or not isinstance(item.get('evidence',[]),list):raise ValueError('decision entries require text and evidence array')
            result['views'][key].append({'id':item.get('id',digest([key,item])),'text':item['text'],'evidence':copy.deepcopy(item.get('evidence',[])),'status':item.get('status','active' if key=='Working on' else 'recorded')})
    result['revision']+=1;return result
