"""Explicit bounded working projections over an immutable, fully navigable archive."""
import copy,json,re
from .adapters import digest
from . import hierarchy,media
POLICY='recent-plus-query-v1'

def project(core,state,role='worker',max_chars=16384,query=''):
    if not isinstance(max_chars,int) or max_chars<512:raise ValueError('projection max_chars must be >=512')
    ids=hierarchy.selected_ids(core,state,role);ranked=list(reversed(ids))
    if query:
        terms=set(re.findall(r'\w+',query.casefold()));ranked=ranked[:1]+sorted(ranked[1:],key=lambda cid:-sum(term in core.store.get(cid,'cb')['body'].casefold() for term in terms))
    chosen=[];used=0;fragments=[]
    # Bound both body and operation input. Full operations remain retrievable.
    for cb_id in ranked:
        cb=core.store.get(cb_id,'cb');remaining=max_chars-used
        if remaining<512:break
        text=cb['body'];body_limit=max(64,min(remaining//2,max_chars//4));body=text if len(text)<=body_limit else text[:body_limit]
        op_records=[];op_chars=0
        for record in cb['ops']:
            encoded=json.dumps(record,ensure_ascii=False)
            if op_chars+len(encoded)>max(64,remaining//4):break
            op_records.append(record);op_chars+=len(encoded)
        value={k:copy.deepcopy(cb[k]) for k in ('cb_id','source_id','source_ids','sequence')};value.update(level=cb.get('level',1),context_id=cb.get('context_id','main'),wb_id=cb.get('wb_id'));
        if 'level' not in cb:value.update(level=None,hierarchy_index_status='legacy-unindexed',wb_formation_order='unknown')
        if cb.get('media_source_ids'):value['media_source_ids']=list(cb['media_source_ids'])
        value['proposal_refs']={'original_count':len(cb.get('proposal_refs',{}).get('original_responses',[])),'revision_count':len(cb.get('proposal_refs',{}).get('candidate_revisions',[])),'read_tool':'sic_memory_page','kind':'proposal','object_id':cb_id};value.update(body=body,ops=op_records,index=[],protocol_events=copy.deepcopy(cb.get('protocol_events',[])))
        value['archive_navigation']={'body_chars':len(text),'body_range':[0,len(body)],'operation_count':len(cb['ops']),'operation_range':[0,len(op_records)],'read_tool':'sic_memory_page','locate_tool':'sic_memory_locate','expand_tool':'sic_hierarchy_expand'}
        if cb.get('media_source_ids'):value['archive_navigation']['media']=media.cb_manifest(core,cb)
        size=len(json.dumps(value,ensure_ascii=False))
        if size>remaining:
            # A large native protocol descriptor is retained in archive, not silently put into a partial prompt.
            value['protocol_events']=[];value['archive_navigation']['protocol_events_in_archive']=True;size=len(json.dumps(value,ensure_ascii=False))
        if size>remaining:continue
        chosen.append(value);used+=size
        if len(body)<len(text) or len(op_records)<len(cb['ops']):fragments.append(cb_id)
    order={cb:i for i,cb in enumerate(ids)};chosen.sort(key=lambda cb:order[cb['cb_id']]);selected=[cb['cb_id'] for cb in chosen]
    manifest={'active_level':hierarchy.selected_view(state,role)['level'],'context_id':hierarchy.selected_view(state,role)['context_id'],'hierarchy_read_tool':'sic_hierarchy_expand','legacy_unindexed_count':len(hierarchy.legacy_ids(core,state)),'legacy_unindexed_navigation':hierarchy.legacy_navigation(core,state),'policy':POLICY,'role':role,'archive_count':len(ids),'archive_chain_digest':digest(ids),'selected_cb_ids':selected,'omitted_count':len(ids)-len(selected),'fragment_cb_ids':fragments,'max_chars':max_chars,'presented_chars':used,'query':query,'navigation':{'timeline_operation':'memory.timeline','source_page_operation':'memory.page','locate_operation':'memory.locate'},'physical_cache_hit':'unknown'}
    media_all=list(dict.fromkeys(sid for cid in ids for sid in core.store.get(cid,'cb').get('media_source_ids',[])));media_selected=list(dict.fromkeys(sid for cb in chosen for sid in cb.get('media_source_ids',[])))
    if media_all:manifest['media']={'policy':'retain-native-image-carriers-for-selected-CBs; omitted-CBs-explicitly-navigable','selected_source_count':len(media_selected),'selected_source_digest':digest(media_selected),'omitted_source_count':sum(sid not in media_selected for sid in media_all),'archive_source_digest':digest(media_all),'sample_selected_source_ids':media_selected[:8],'source_navigation':'hierarchy.expand -> source.read; media page returns exact native carrier','read_operation':'source.read','complete_page_kind':'media','image_tokens_in_text_char_budget':False,'complete_provider_admission_required':True}
    return {'cbs':chosen,'manifest':manifest}

def page(core,state,p):
    kind=p['kind'];object_id=p['object_id'];offset=int(p.get('offset',0));limit=int(p.get('limit',2048))
    if offset<0 or not 1<=limit<=8192:raise ValueError('page offset>=0 and limit1..8192 required')
    sources={x for group in state['groups'].values() for x in group}
    if kind=='source':
        if object_id not in sources:raise ValueError('source not on authorized chain')
        source=core.store.get(object_id,'source');text=''.join(('' if i==0 or u.get('continuation') else '\n')+u['text'] for i,u in enumerate(source['units']));value=text[offset:offset+limit];total=len(text);unit='unicode-codepoints';sidecar=False
    elif kind=='media':
        if object_id not in sources:raise ValueError('media source not on authorized chain')
        source=core.store.get(object_id,'source');data=json.dumps({'source_id':object_id,'media':source.get('media',[]),'native_request':source['sidecar']},ensure_ascii=False);value=data[offset:offset+limit];total=len(data);unit='native-media-json-codepoints';sidecar=True
    elif kind in ('cb','ops'):
        if object_id not in hierarchy.all_cbs(state):raise ValueError('CB not on authorized chain')
        cb=core.store.get(object_id,'cb');data=cb['body'] if kind=='cb' else json.dumps(cb['ops'],ensure_ascii=False);value=data[offset:offset+limit];total=len(data);unit='unicode-codepoints' if kind=='cb' else 'ordered-operations-json-codepoints';sidecar=False
    elif kind=='proposal':
        if object_id not in hierarchy.all_cbs(state):raise ValueError('proposal CB not on authorized chain')
        cb=core.store.get(object_id,'cb');refs=cb.get('proposal_refs',{});original=[]
        for ref in refs.get('original_responses',[]):
            record=core.store.get(ref['object_id'],ref['kind'])
            if ref['kind']=='provider_response':
                raw=record['response']['choices'][0]['message']['content'];parsed=json.loads(raw);original.append({**ref,'raw_model_text':raw,'ops':parsed.get('ops',[]),'pvi':parsed.get('pvi',{})})
            else:original.append({**ref,'ops':record.get('ops',[]),'pvi':record.get('pvi',{})})
            if ref.get('canonical_response_id'):original[-1]['canonical_delivery']=core.store.get(ref['canonical_response_id'],'editor_canonical_response')
        data=json.dumps({'cb_id':object_id,'original_responses':original,'candidate_revisions':[core.store.get(r,'candidate_revision') for r in refs.get('candidate_revisions',[])]},ensure_ascii=False);value=data[offset:offset+limit];total=len(data);unit='proposal-json-codepoints';sidecar=False
    elif kind=='tool_result':
        if object_id not in state.get('tool_pages',[]):raise ValueError('tool result not on authorized chain')
        record=core.store.get(object_id,'tool_return');data=json.dumps(record['value'],ensure_ascii=False);value=data[offset:offset+limit];total=len(data);unit='tool-result-json-codepoints';sidecar=False
    elif kind=='view':
        cut=core.store.get(object_id,'cut')
        if cut['chain_id']!=state['chain_id']:raise ValueError('view cut not on chain')
        instant=cut['instant']
        if 'role' in p:instant=copy.deepcopy(hierarchy.selected_instant(cut,p['role']))
        elif 'level' in p or 'context_id' in p:instant=hierarchy.level(cut,int(p.get('level',1)),p.get('context_id','main'))['instant']
        data=json.dumps(instant,ensure_ascii=False);value=data[offset:offset+limit];total=len(data);unit='instant-view-json-codepoints';sidecar=False
    elif kind in ('prefix_plan','provider_request'):
        record=core.store.get(object_id,'shared_prefix_plan' if kind=='prefix_plan' else 'provider_request')
        if record.get('chain_id')!=state['chain_id']:raise ValueError('prefix/request object not on authorized chain')
        data=json.dumps(record,ensure_ascii=False);value=data[offset:offset+limit];total=len(data);unit='frozen-prefix-request-json-codepoints';sidecar=False
    else:raise ValueError('page kind must be source/media/cb/ops/proposal/tool_result/view')
    return {'kind':kind,'object_id':object_id,'offset':offset,'limit':limit,'total':total,'unit':unit,'value':value,'next_offset':offset+len(value) if offset+len(value)<total else None,'full_digest':digest(data if kind!='source' else text),'protocol_sidecar_included':sidecar}


def split_source(source,max_chars=4096):
    value=copy.deepcopy(source);units=[]
    for original in source['units']:
        text=original['text']
        for start in range(0,max(len(text),1),max_chars):
            unit=copy.deepcopy(original);unit['unit_id']='u'+str(len(units)+1);unit['text']=text[start:start+max_chars];unit['continuation']=start>0;unit['anchor']['char_start']=start;unit['anchor']['char_end']=min(start+max_chars,len(text));units.append(unit)
    value['units']=units;return value

def bounded_instant(instant,max_chars,cut_id,view_params=None):
    if len(json.dumps(instant,ensure_ascii=False))<=max_chars:return copy.deepcopy(instant)
    # The navigation metadata is part of the budget too. A per-key directory
    # grows without bound even when every individual value is tiny.
    result={};manifest={'kind':'view','object_id':cut_id,**(view_params or {}),'total':len(instant),'omitted':len(instant),'digest':digest(instant),'read_operation':'memory.page'}
    result['instant_projection']=manifest
    if len(json.dumps(result,ensure_ascii=False))>max_chars:raise ValueError('instant budget cannot hold archive navigation')
    for key,value in instant.items():
        if key=='instant_projection':continue
        proposed={**result,key:value};proposed['instant_projection']={**manifest,'omitted':len(instant)-(len(result))}
        if len(json.dumps(proposed,ensure_ascii=False))<=max_chars:
            result=copy.deepcopy(proposed);manifest=result['instant_projection']
    return result
