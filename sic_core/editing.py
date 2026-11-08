"""Pure content operations. Protocol sidecars are never model-editable targets."""
import copy,json
from . import decision
OPS={'KEEP','DELETE','FLAG','REORDER','COMPRESS','REPLACE','ADD'}

def decision_operation_content(content):
    """An I operation remains text; the reserved primary view encodes JSON."""
    def pairs(items):
        value={}
        for key,item in items:
            if key in value:raise ValueError('duplicate decision JSON key')
            value[key]=item
        return value
    def reject_constant(value):raise ValueError('non-finite decision JSON constant')
    try:raw=json.loads(content,object_pairs_hook=pairs,parse_constant=reject_constant)
    except (ValueError,TypeError):raise ValueError('decision_tree I operation content must encode a four-view JSON object') from None
    decision.from_editor(raw,decision.empty())
    return raw

def validate_response(scope,ops,pvi,*,primary_decision=False):
    if not isinstance(ops,list):raise ValueError('ops must be ordered array')
    seen=set()
    for op in ops:
        if not isinstance(op,dict) or set(op)-{'op','target','content','reason','order','anchor','lifecycle','view_key','risk'}:raise ValueError('unknown editing field; native protocol mutation forbidden')
        kind=op.get('op')
        lifecycle=op.get('lifecycle','P')
        if lifecycle not in ('P','V','I'):raise ValueError('content lifecycle must be P/V/I')
        if lifecycle!='P' and kind not in ('ADD','COMPRESS','REPLACE','FLAG'):raise ValueError('only generated content operations can route content to V/I')
        if lifecycle=='I' and not isinstance(op.get('view_key'),str):raise ValueError('I operation requires view_key')
        if 'risk' in op and op['risk'] not in ('none','low','medium','high','unknown'):raise ValueError('invalid loss-risk classification')
        if kind not in OPS:raise ValueError('unknown content operation')
        target=op.get('target')
        if kind!='ADD':
            if target not in scope:raise ValueError('operation outside assigned editable scope')
            seen.add(target)
        elif target is not None:raise ValueError('ADD has no mutable source target')
        if 'anchor' in op:
            anchor=op['anchor']
            if kind!='ADD' or not isinstance(anchor,dict) or set(anchor)!={'target','position'} or anchor['target'] not in scope or anchor['position'] not in ('before','after'):raise ValueError('ADD anchor requires assigned target and before/after position')
        if kind in ('ADD','REPLACE','COMPRESS','FLAG') and not isinstance(op.get('content'),str):raise ValueError('content string required')
        if primary_decision and lifecycle=='I' and op['view_key']=='decision_tree':decision_operation_content(op['content'])
        if kind=='REORDER' and (not isinstance(op.get('order'),int) or isinstance(op['order'],bool)):raise ValueError('REORDER requires integer order')
    if not isinstance(pvi,dict) or set(pvi)-{'P','V','I'}:raise ValueError('pvi accepts P/V/I only')
    for key in ('P','V'):
        if not isinstance(pvi.get(key,[]),list) or any(not isinstance(x,str) for x in pvi.get(key,[])):raise ValueError('P/V must be arrays of content strings')
    if not isinstance(pvi.get('I',{}),dict):raise ValueError('I must be named content views')
    if primary_decision and 'decision_tree' in pvi.get('I',{}):decision.from_editor(pvi['I']['decision_tree'],decision.empty())

def materialize(source,assignments,responses,*,primary_decision=False):
    units=source['units'];order={u['unit_id']:i for i,u in enumerate(units)};texts={u['unit_id']:u['text'] for u in units};flags={};persistent=[];volatile=[];instant={};ops=[];adds=[]
    for assignment in assignments:
        response=responses[assignment['editor']];validate_response(assignment['unit_ids'],response['ops'],response['pvi'],primary_decision=primary_decision)
        for op in response['ops']:
            op=copy.deepcopy(op);op['editor']=assignment['editor'];ops.append(op)
            kind=op['op'];target=op.get('target');lifecycle=op.get('lifecycle','P')
            if lifecycle!='P':
                if kind in ('REPLACE','COMPRESS'):texts.pop(target,None)
                if lifecycle=='V':volatile.append(op['content'])
                else:
                    key=op['view_key']
                    if key in instant:raise ValueError('conflicting instantaneous content key')
                    instant[key]=decision_operation_content(op['content']) if primary_decision and key=='decision_tree' else op['content']
                continue
            if kind=='DELETE':texts.pop(target,None)
            elif kind in ('REPLACE','COMPRESS'):texts[target]=op['content']
            elif kind=='FLAG':
                if target not in texts:raise ValueError('cannot FLAG a deleted unit')
                flags[target]=(flags.get(target,'')+'; '+op['content']).strip('; ')
            elif kind=='REORDER':order[target]=op['order']
            elif kind=='ADD':adds.append(op)
        pvi=response['pvi'];persistent.extend(pvi.get('P',[]));volatile.extend(pvi.get('V',[]))
        ops.extend({'op':'ADD','content':text,'origin':'editor-P','editor':assignment['editor']} for text in pvi.get('P',[]))
        for key,value in pvi.get('I',{}).items():
            if key in instant:raise ValueError('Editors conflict on same instant view')
            instant[key]=value
    explicit={op.get('target') for op in ops}
    # Default KEEP is materialized compactly but discoverably in immutable CB ops.
    keeps=[u['unit_id'] for u in units if u['unit_id'] not in explicit]
    if keeps:ops.append({'op':'KEEP','targets':keeps,'origin':'implicit-program'})
    active=sorted(texts,key=lambda key:(order[key],next(i for i,u in enumerate(units) if u['unit_id']==key)))
    pieces=[]
    for addition in adds:
        if addition.get('anchor') and addition['anchor']['target'] not in texts:raise ValueError('ADD anchor was deleted from active view')
    for key in active:
        pieces.extend(a['content'] for a in adds if a.get('anchor')=={'target':key,'position':'before'})
        unit=next(u for u in units if u['unit_id']==key)
        if unit.get('continuation') and pieces and not any(a.get('anchor')=={'target':key,'position':'before'} for a in adds):pieces[-1]+=texts[key]+(('\n[FLAG] '+flags[key]) if key in flags else '')
        else:pieces.append(texts[key]+(('\n[FLAG] '+flags[key]) if key in flags else ''))
        pieces.extend(a['content'] for a in adds if a.get('anchor')=={'target':key,'position':'after'})
    body='\n'.join(pieces+[a['content'] for a in adds if not a.get('anchor')]+persistent)
    return {'body':body,'ops':ops,'volatile':volatile,'instant':instant,'index':[{'unit_id':u['unit_id'],'anchor':u['anchor'],'operation_indexes':[i for i,op in enumerate(ops) if op.get('target')==u['unit_id'] or u['unit_id'] in op.get('targets',[])]} for u in units]}

def program_ops(source,rules):
    allowed={'deduplicate_exact','drop_empty','normalize_newlines'}
    if set(rules)-allowed:raise ValueError('unknown deterministic program rule')
    seen=set();ops=[]
    for unit in source['units']:
        text=unit['text'];op=None
        if 'drop_empty' in rules and not text.strip():op={'op':'DELETE','target':unit['unit_id'],'reason':'empty text'}
        elif 'deduplicate_exact' in rules and text in seen:op={'op':'DELETE','target':unit['unit_id'],'reason':'exact duplicate within WB'}
        elif 'normalize_newlines' in rules and '\r' in text:op={'op':'REPLACE','target':unit['unit_id'],'content':text.replace('\r\n','\n').replace('\r','\n'),'reason':'deterministic newline normalization'}
        seen.add(text)
        if op:ops.append(op)
    return ops

def replay(source,records,step,*,primary_decision=False):
    """Replay a prefix of the published operation ledger on its archived WB.

    This computes a content representation only. It does not replay a model,
    publish a CB, invoke a tool or undo an external side effect.
    """
    if not isinstance(step,int) or isinstance(step,bool) or not 0<=step<=len(records):raise ValueError('replay step must be 0..operation count')
    allowed={'op','target','content','reason','order','anchor','lifecycle','view_key','risk'}
    scope=[u['unit_id'] for u in source['units']];assignments=[];responses={};communications=[]
    for i,record in enumerate(records[:step]):
        if record.get('op')=='KEEP' and 'targets' in record:
            if not set(record['targets'])<=set(scope):raise ValueError('replay KEEP has unknown WB target')
            continue
        if record.get('origin')=='persistent-communication':
            communications.append(record['content']);continue
        editor='replay-'+str(i);assignments.append({'editor':editor,'unit_ids':scope})
        if record.get('origin')=='editor-P':responses[editor]={'ops':[],'pvi':{'P':[record['content']]}}
        else:responses[editor]={'ops':[{k:copy.deepcopy(v) for k,v in record.items() if k in allowed}],'pvi':{}}
    value=materialize(source,assignments,responses,primary_decision=primary_decision)
    if communications:value['body']+='\n'+'\n'.join(communications)
    return {'body':value['body'],'volatile':value['volatile'],'instant':value['instant'],'step':step,'operation_count':len(records),'external_effects_replayed':False,'basis':'archived WB with published ordered operation prefix'}
