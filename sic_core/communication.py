"""Immutable first-receipt facts; text never supplies transport authority."""
import copy,json
from .adapters import digest
from .store import uid
GENERATED={'received_at','received_event_id','receive_receipt_id'}

def semantic_message(message):return {k:copy.deepcopy(v) for k,v in message.items() if k not in GENERATED}

def provenance(message):
    fields=('message_id','sender','socket_id','checkpoint_id','request_id','result_source_id','lifecycle','received_at','received_event_id','receive_receipt_id')
    value={key:copy.deepcopy(message[key]) for key in fields if key in message}
    for key in GENERATED:value.setdefault(key,None)
    if value['received_at'] is None:value['receipt_guarantee']='legacy-arrival-time-unknown'
    return value

def receive_message(core,state,message,*,enqueue=True):
    """Caller holds Store transaction and has checked current authorization.

    Saves immutable receipt + one event on first delivery; caller saves state.
    Existing historical messages are never assigned a fabricated receive time.
    """
    from .service import require
    require(isinstance(message,dict) and not GENERATED.intersection(message),'Receipt metadata is host-generated','invalid',422)
    require(isinstance(message.get('message_id'),str) and bool(message['message_id']),'Message identity required','invalid',422)
    require(isinstance(message.get('text'),str) and message.get('lifecycle') in ('P','V'),'Message text and P/V lifecycle required','invalid',422)
    existing=next((row for row in state['inbox'] if row['message_id']==message['message_id']),None)
    receipt=state.get('communication_receipts',{}).get(message['message_id'])
    if existing is None and receipt:existing=core.store.get(receipt,'communication_receipt')['message']
    if existing is not None:
        require(semantic_message(existing)==message,'message id reused with different content')
        return {'message':copy.deepcopy(existing),'created':False,**{key:existing.get(key) for key in GENERATED}}
    at=core.now();event_id=uid('communication_event');receipt_id=uid('communication_receipt')
    stored={**copy.deepcopy(message),'received_at':at,'received_event_id':event_id,'receive_receipt_id':receipt_id}
    core.store.put('communication_receipt',{'receipt_id':receipt_id,'chain_id':state['chain_id'],'message':stored},receipt_id)
    state.setdefault('communication_receipts',{})[stored['message_id']]=receipt_id
    if enqueue:state['inbox'].append(stored)
    state['events'].append({'type':'communication_received','event_id':event_id,'message_id':stored['message_id'],'receive_receipt_id':receipt_id,'sender':stored.get('sender'),'socket_id':stored.get('socket_id'),'request_id':stored.get('request_id'),'time':at})
    return {'message':copy.deepcopy(stored),'created':True,**{key:stored[key] for key in GENERATED}}


def source_provenance(source):
    rows=list(source.get('communications',[]))
    if source.get('communication'):rows.append(source['communication'])
    return [provenance(row) for row in rows]


def cut_provenance(core,cut,role='worker'):
    from . import hierarchy
    rows=[];seen=set();visited=set()
    def add(value):
        key=value.get('receive_receipt_id') or (value.get('sender'),value.get('message_id'))
        if key not in seen:seen.add(key);rows.append(copy.deepcopy(value))
    def visit(cid):
        if cid in visited:return
        visited.add(cid);cb=core.store.get(cid,'cb')
        for member in cb.get('members',[]):visit(member)
        for record in cb['ops']:
            if record.get('communication'):add(record['communication'])
        for sid in cb['source_ids']:
            for value in source_provenance(core.store.get(sid,'source')):add(value)
    for cid in hierarchy.selected_ids(core,copy.deepcopy(cut),role):visit(cid)
    for root in pending_sources(core,copy.deepcopy(cut),role):
        for sid in cut['groups'].get(root,[root]):
            for value in source_provenance(core.store.get(sid,'source')):add(value)
    return rows


def page(core,state,p):
    from .service import require
    oid=p['object_id'];offset=int(p.get('offset',0));limit=int(p.get('limit',2048));require(offset>=0 and 1<=limit<=8192,'communication page bounds invalid','invalid',422)
    sources={sid for group in state['groups'].values() for sid in group}
    if oid in sources:rows=source_provenance(core.store.get(oid,'source'))
    else:
        cut=core.store.get(oid,'cut');require(cut['chain_id']==state['chain_id'],'communication cut not on authorized route','forbidden',403);rows=cut_provenance(core,cut,p.get('role','worker'))
    value=json.dumps({'readonly':True,'communications':rows},ensure_ascii=False,sort_keys=True)
    part=value[offset:offset+limit]
    return {'kind':'communication','object_id':oid,'readonly':True,'offset':offset,'limit':limit,'total':len(value),'value':part,'next_offset':offset+len(part) if offset+len(part)<len(value) else None,'full_digest':digest(value),'unit':'communication-provenance-json-codepoints','protocol_sidecar_included':False}


def visible_contexts(state,role='worker'):
    """Actual selected context plus its explicitly shared context DAG."""
    from . import hierarchy
    selected=hierarchy.selected_view(state,role);found={}
    def visit(name,number):
        if found.get(name,0)>=number:return
        found[name]=number;ctx=hierarchy.context(state,name)
        for shared in ctx.get('shared_contexts',[]):
            active=hierarchy.context(state,shared)['active'];visit(shared,active.get(role,active.get('editor',1)))
    visit(selected['context_id'],selected['level']);return found


def pending_sources(core,state,role='worker'):
    """Real continuation WBs only; a derived member snapshot is already in CBs."""
    from . import hierarchy
    records=[];seen=set();ordinal=0
    visible=visible_contexts(state,role)
    if role=='worker':visible={'main':max(1,visible.get('main',0)),**visible}
    for name,maximum in visible.items():
        ctx=hierarchy.context(state,name)
        for number,row in ctx['levels'].items():
            if int(number)>maximum:continue
            by_source={}
            for wid in row['wbs']:
                wb=core.store.get(wid,'wb');by_source[wb['source_id']]=wb
            for root in row['pending']:
                wb=by_source.get(root,{})
                if root in seen or wb.get('members'):continue
                seen.add(root);ordinal+=1;records.append((wb.get('formed_at',float('-inf')),ordinal,root))
    return [root for _,_,root in sorted(records)]


def publication_visible(core,state,context_id,number,members):
    """Evaluate the real replacement rule with one prospective CB, without writes."""
    from . import hierarchy
    marker=object();selected=hierarchy.selected_view(state,'worker')
    def projected(name,maximum):
        ctx=hierarchy.context(state,name);ids=list(hierarchy.level(state,1,name)['cbs'])
        if name==context_id and number==1:ids.append(marker)
        for n in range(2,maximum+1):
            candidates=list(hierarchy.level(state,n,name)['cbs'])
            if name==context_id and n==number:candidates.append(marker)
            for cid in candidates:
                lower=members if cid is marker else core.store.get(cid,'cb').get('members',[])
                if not lower:ids.append(cid);continue
                try:start=ids.index(lower[0])
                except ValueError:continue
                if ids[start:start+len(lower)]==lower:ids[start:start+len(lower)]=[cid]
        shared=[]
        for shared_id in ctx.get('shared_contexts',[]):
            active=hierarchy.context(state,shared_id)['active'];shared.extend(projected(shared_id,active.get('worker',active.get('editor',1))))
        return shared+ids
    return marker in projected(selected['context_id'],selected['level'])
