"""Ordered WB/CB chains inside one Agent route, with immutable recursive material."""
import copy
from .adapters import decode,digest
from .store import uid


def ensure(state):
    h=state.setdefault('hierarchy',{'levels':{},'active':{'worker':1,'editor':1}})
    h['levels'].setdefault('1',{'wbs':[],'cbs':[],'pending':[],'instant':{},'pairs':{}})
    # Level one aliases are restored after JSON persistence, never another route.
    one=h['levels']['1'];one['cbs']=state['cbs'];one['pending']=state['pending'];one['instant']=state['instant']
    return h


def context(state,context_id='main'):
    h=ensure(state)
    if context_id=='main':return h
    if context_id not in h.get('contexts',{}):raise ValueError('unknown internal context')
    return h['contexts'][context_id]


def level(state,number,context_id='main'):
    if not isinstance(number,int) or isinstance(number,bool) or number<1:raise ValueError('positive integer level required')
    return context(state,context_id)['levels'].setdefault(str(number),{'wbs':[],'cbs':[],'pending':[],'instant':{},'pairs':{}})


def contexts(state):
    h=ensure(state);return {'main':h,**h.get('contexts',{})}


def all_cbs(state):return [cid for ctx in contexts(state).values() for row in ctx['levels'].values() for cid in row['cbs']]


def legacy_ids(core,state):
    """Only historical objects lacking the new explicit level contract qualify."""
    return [cid for cid in all_cbs(state) if 'level' not in core.store.get(cid,'cb')]


def legacy_navigation(core,state):
    actor=state.get('_actor',state['owners'][0]);available={t['name'] for t in state['tools'] if core._acl(t,'discover',actor) and core._acl(t,'execute',actor) and core._acl(t,'expand',actor) and t.get('expanded')}
    tool='sic_hierarchy_legacy' if 'sic_hierarchy_legacy' in available else 'sic_operation_directory' if 'sic_operation_directory' in available else None
    return {'operation':'hierarchy.legacy','tool':tool,'offset':0,'limit':20,'tool_arguments':{'offset':0,'limit':20},'fallback_directory_contains_all_cbs':tool=='sic_operation_directory','wb_formation_order':'unknown'}


def legacy_expand(core,state,cb):
    from .service import require
    cid=cb['cb_id'];wid=cb.get('wb_id');wb=core.store.get(wid,'wb') if wid else {};sources=list(wb.get('source_ids',cb['source_ids']));root=cb['source_id'];allowed={x for group in state['groups'].values() for x in group}
    require(bool(sources) and set(sources)<=allowed,'Legacy WB sources not on authorized route','forbidden',403)
    archived=copy.copy(state);archived['groups']={**state['groups'],root:sources};ids=state['cbs'];index=ids.index(cid) if cid in ids else None
    return {'chain_id':state['chain_id'],'context_id':cb.get('context_id','main'),'level':None,'legacy':True,'hierarchy_index_status':'legacy-unindexed','wb_formation_order':'unknown','wb_sequence':None,'wb_formed_at':None,'known_cb_order':{'membership_index':index,'saved_sequence':cb.get('sequence'),'predecessor':ids[index-1] if index is not None and index else None,'successor':ids[index+1] if index is not None and index+1<len(ids) else None},'wb_id':wid,'cb_id':cid,'source_ids':sources,'members':[],'original_units':core._source(archived,root)['units'],'complete_read':{'kind':'source','object_id':root},'complete_reads':[{'kind':'source','object_id':x} for x in sources],'publication_supplement_sources':[{'kind':'source','object_id':x} for x in cb['source_ids'] if x not in sources],'legacy_navigation':{'operation':'hierarchy.legacy','offset':0,'limit':20},'guarantee':'Saved CB order, saved CB-WB relation and retained source references only; original WB formation chronology is unknown.'}


def source_level(state,source_id):
    for ctx in contexts(state).values():
        for n,row in ctx['levels'].items():
            if source_id in row['pending']:return int(n)
    raise ValueError('source is not a pending WB at any level')


def source_context(state,source_id):
    for name,ctx in contexts(state).items():
        if any(source_id in row['pending'] for row in ctx['levels'].values()):return name
    raise ValueError('source is not a pending WB in any internal context')


def register_wb(core,state,source_id,number=1,members=None,reason='edit-boundary',context_id='main',*,_comparison_id=None):
    row=level(state,number,context_id)
    for wid in row['wbs']:
        if core.store.get(wid,'wb')['source_id']==source_id:return wid
    source=core._source(state,source_id)
    wb={'wb_id':_comparison_id or uid('wb'),'chain_id':state['chain_id'],'context_id':context_id,'level':number,'sequence':len(row['wbs'])+1,'source_id':source_id,'source_ids':source['source_ids'],'unit_ids':[u['unit_id'] for u in source['units']],'members':copy.deepcopy(members or []),'reason':reason,'formed_at':core.now()}
    core.store.put('wb',wb,wb['wb_id']);row['wbs'].append(wb['wb_id']);return wb['wb_id']


def selected_ids(core,state,role):
    """Replace only a contiguous covered range; preserve all uncovered tails."""
    view=selected_view(state,role)
    return context_ids(core,state,view['context_id'],view['level'],role)


def selected_view(state,role):
    h=ensure(state)
    return h.get('role_views',{}).get(role,h.get('role_views',{}).get('editor') if role.startswith('editor:') else None) or {'context_id':'main','level':h['active'].get(role,1)}


def selected_instant(state,role):
    view=selected_view(state,role);return level(state,view['level'],view['context_id'])['instant']


def context_ids(core,state,context_id, target,role):
    ctx=context(state,context_id);ids=list(level(state,1,context_id)['cbs'])
    for number in range(2,target+1):
        for cid in level(state,number,context_id)['cbs']:
            cb=core.store.get(cid,'cb');members=cb.get('members',[])
            if not members:
                ids.append(cid)  # A V continuation adds work at this same level.
                continue
            try:start=ids.index(members[0])
            except ValueError:continue
            if ids[start:start+len(members)]==members:ids[start:start+len(members)]=[cid]
    shared=[]
    for shared_id in ctx.get('shared_contexts',[]):shared.extend(context_ids(core,state,shared_id,context(state,shared_id)['active'].get(role,context(state,shared_id)['active'].get('editor',1)),role))
    return list(dict.fromkeys(shared+ids))


def dispatch(core,op,state,actor,p):
    from .service import require
    h=ensure(state)
    if op=='hierarchy.get':return {'chain_id':state['chain_id'],**copy.deepcopy(h)}
    context_id=p.get('context_id','main')
    if op=='hierarchy.legacy':
        ids=legacy_ids(core,state);offset=int(p.get('offset',0));limit=int(p.get('limit',20));require(offset>=0 and 1<=limit<=100,'invalid legacy directory bounds');rows=[]
        for cid in ids[offset:offset+limit]:
            cb=core.store.get(cid,'cb');rows.append({'cb_id':cid,'wb_id':cb.get('wb_id'),'saved_cb_sequence':cb.get('sequence'),'hierarchy_index_status':'legacy-unindexed','wb_formation_order':'unknown','expand':{'cb_id':cid},'read':{'kind':'cb','object_id':cid},'source_count':len(cb['source_ids'])})
        return {'chain_id':state['chain_id'],'legacy_unindexed_count':len(ids),'rows':rows,'offset':offset,'next_offset':offset+len(rows) if offset+len(rows)<len(ids) else None,'wb_formation_order':'unknown'}
    if op=='hierarchy.context.create':
        name=p['context_id'];require(isinstance(name,str) and bool(name) and name!='main' and name not in h.get('contexts',{}),'unique internal context_id required')
        shared=p.get('shared_contexts',[]);require(isinstance(shared,list) and len(shared)==len(set(shared)) and set(shared)<=set(contexts(state)),'shared contexts must already exist on this Agent route')
        h.setdefault('contexts',{})[name]={'levels':{},'active':{'worker':1,'editor':1},'shared_contexts':list(shared),'purpose':p.get('purpose','editor-persistent-context')};level(state,1,name);core.store.save(state)
        return {'chain_id':state['chain_id'],'context_id':name,'shared_contexts':shared,'agent_routes_created':0}
    if op=='hierarchy.context.ingest':
        require(context_id!='main','use source.ingest for primary working context');row=level(state,1,context_id)
        source=decode('openai-chat',p['request']);require(not source['protected'] and not source['open_calls'],'internal persistent material must be closed content without protected instructions')
        sid=core.store.put('source',source);state['groups'][sid]=[sid];row['pending'].append(sid);wid=register_wb(core,state,sid,1,reason='internal-context-material',context_id=context_id);core.store.save(state)
        return {'chain_id':state['chain_id'],'context_id':context_id,'source_id':sid,'wb_id':wid,'units':core._source(state,sid)['units']}
    if op=='hierarchy.select':
        require(not state.get('provider_inflight'),'cannot switch representation during Worker invocation')
        number=p['level'];row=level(state,number,context_id);require(bool(row['cbs']),'selected level has no published representation')
        roles=p.get('roles',['worker','editor']);require(isinstance(roles,list) and bool(roles) and all(isinstance(r,str) and (r in ('worker','editor') or r.startswith('editor:') and len(r)>7) for r in roles),'invalid representation roles')
        for role in roles:
            context(state,context_id)['active'][role]=number;h.setdefault('role_views',{})[role]={'context_id':context_id,'level':number}
        if state.get('current_cut'):
            # Adoption is itself a new historical cut, so a checkpoint taken
            # now freezes the role selections rather than consulting a live head.
            cut=copy.deepcopy(core.store.get(state['current_cut'],'cut'));cut.pop('digest',None)
            cut.update(cut_id=uid('cut'),sequence=state['cut_seq']+1,previous_cut=state['current_cut'],hierarchy=copy.deepcopy(h),representation_selection=True)
            cut['digest']=digest(cut);core.store.put('cut',cut,cut['cut_id']);state['current_cut']=cut['cut_id'];state['cut_seq']=cut['sequence']
        state['events'].append({'event_id':uid('event'),'type':'representation_selected','level':number,'roles':roles,'time':core.now()});core.store.save(state)
        return {'active':copy.deepcopy(h['active']),'role_views':copy.deepcopy(h.get('role_views',{})),'selected_cb_ids':{role:selected_ids(core,state,role) for role in roles},'cut_id':state['current_cut'],'sequence':state['cut_seq'],'archive_mutated':False}
    if op=='hierarchy.form':
        number=p['level'];require(isinstance(number,int) and number>=2,'higher WB requires level >=2');lower=level(state,number-1,context_id);row=level(state,number,context_id)
        members=p.get('cb_ids');require(isinstance(members,list) and bool(members) and len(members)==len(set(members)),'nonempty unique ordered cb_ids required')
        require(members[0] in lower['cbs'],'member not in lower level','forbidden',403);start=lower['cbs'].index(members[0]);require(lower['cbs'][start:start+len(members)]==members,'members must be consecutive lower-level CBs in chain order')
        consumed=[cid for wid in row['wbs'] for cid in core.store.get(wid,'wb').get('members',[])]
        require(not set(consumed).intersection(members),'lower CB interval already formed into a WB')
        if consumed:require(start>lower['cbs'].index(consumed[-1]),'higher WB formation must follow lower chain order')
        blocks=[core.store.get(cid,'cb') for cid in members]
        source=decode('openai-chat',{'messages':[{'role':'user','content':cb['body']} for cb in blocks]})
        media_refs=list(dict.fromkeys(x for cb in blocks for x in cb.get('media_source_ids',[])))
        if media_refs:source['media_source_ids']=media_refs
        source['dependencies']=list(dict.fromkeys(x for cb in blocks for x in cb['source_ids']));source['members']=members;source['level']=number
        for unit in source['units']:unit['anchor']['cb_id']=members[unit['anchor']['message']];unit['anchor']['level']=number-1
        sid=core.store.put('source',source);state['groups'][sid]=[sid];row['pending'].append(sid)
        wid=register_wb(core,state,sid,number,members,'ordered-lower-CB-interval',context_id);core.store.save(state)
        return {'chain_id':state['chain_id'],'context_id':context_id,'level':number,'wb_id':wid,'source_id':sid,'members':members,'units':core._source(state,sid)['units']}
    if op=='hierarchy.timeline':
        number=p.get('level',1);row=level(state,number,context_id);kind=p.get('kind','wb');require(kind in ('wb','cb'),'kind must be wb/cb');ids=row[kind+'s'];offset=int(p.get('offset',0));limit=int(p.get('limit',20));require(offset>=0 and 1<=limit<=100,'invalid timeline bounds');rows=[]
        reverse={v:k for k,v in row['pairs'].items()}
        for i in range(offset,min(offset+limit,len(ids))):
            obj=core.store.get(ids[i],kind);legacy=kind=='cb' and 'level' not in obj;rows.append({'id':ids[i],'context_id':context_id,'level':None if legacy else number,'hierarchy_index_status':'legacy-unindexed' if legacy else 'indexed','wb_formation_order':'unknown' if legacy else 'recorded','membership_index':i,'predecessor':ids[i-1] if i else None,'successor':ids[i+1] if i+1<len(ids) else None,'wb_id':ids[i] if kind=='wb' else reverse.get(ids[i],obj.get('wb_id')),'cb_id':ids[i] if kind=='cb' else row['pairs'].get(ids[i]),'source_id':obj['source_id'],'members':obj.get('members',[])})
        return {'chain_id':state['chain_id'],'context_id':context_id,'level':number,'kind':kind,'rows':rows,'total':len(ids),'next_offset':offset+len(rows) if offset+len(rows)<len(ids) else None}
    if op=='hierarchy.expand':
        cid=p.get('cb_id');wid=p.get('wb_id');require(bool(cid)^bool(wid),'specify cb_id or wb_id')
        if cid:
            require(cid in all_cbs(state),'CB not on authorized route','forbidden',403);cb=core.store.get(cid,'cb')
            if 'level' not in cb:return legacy_expand(core,state,cb)
            wid=cb['wb_id']
        else:
            linked=next((core.store.get(x,'cb') for x in all_cbs(state) if core.store.get(x,'cb').get('wb_id')==wid),None)
            if linked and 'level' not in linked:return legacy_expand(core,state,linked)
            require(any(wid in row['wbs'] for ctx in contexts(state).values() for row in ctx['levels'].values()),'WB not on authorized route','forbidden',403)
        wb=core.store.get(wid,'wb');require(all(k in wb for k in ('level','context_id','sequence','source_id','source_ids')),'New WB is missing required hierarchy metadata','hierarchy_integrity',409)
        row=level(state,wb['level'],wb['context_id']);require(wid in row['wbs'],'New WB is absent from its formation chain','hierarchy_integrity',409)
        paired=row['pairs'].get(wid);require(not cid or paired==cid,'New CB-WB correspondence is inconsistent','hierarchy_integrity',409);cid=paired
        archived=copy.copy(state);archived['groups']={**state['groups'],wb['source_id']:wb['source_ids']}
        publication_sources=core.store.get(cid,'cb')['source_ids'] if cid else wb['source_ids']
        return {'chain_id':state['chain_id'],'context_id':wb.get('context_id','main'),'level':wb['level'],'wb_id':wid,'cb_id':cid,'source_ids':wb['source_ids'],'members':[{'cb_id':x,'level':wb['level']-1,'read':{'kind':'cb','object_id':x},'expand':{'cb_id':x}} for x in wb.get('members',[])],'original_units':core._source(archived,wb['source_id'])['units'],'complete_read':{'kind':'source','object_id':wb['source_id']},'complete_reads':[{'kind':'source','object_id':x} for x in wb['source_ids']],'publication_supplement_sources':[{'kind':'source','object_id':x} for x in publication_sources if x not in wb['source_ids']]}
    if op=='hierarchy.maintain':
        fanout=int(p.get('fanout',8));require(fanout>=2,'fanout must be >=2');actions=[];number=1
        # Program editing is real materialization, with the same immutable publication lifecycle.
        while number<=max(map(int,context(state,context_id)['levels'])):
            lower=level(state,number,context_id);upper=level(state,number+1,context_id);used={cid for wid in upper['wbs'] for cid in core.store.get(wid,'wb').get('members',[])};remaining=[cid for cid in lower['cbs'] if cid not in used]
            while len(remaining)>=fanout:
                formed=dispatch(core,'hierarchy.form',state,actor,{'context_id':context_id,'level':number+1,'cb_ids':remaining[:fanout]});prepared=core._dispatch('edit.program',state,actor,{'source_id':formed['source_id'],'rules':p.get('rules',['deduplicate_exact'])});published=core._publish(state,{'job_id':prepared['job_id'],'expected_cut':state['cut_seq'],'idempotency_key':'hierarchy-maintain:'+formed['wb_id']});actions.append({'formed':formed['wb_id'],'published':published['cb_id'],'level':number+1});remaining=remaining[fanout:]
            # Creating an empty upper row must not make this traversal infinite.
            if not upper['cbs']:break
            number+=1
        if actions:dispatch(core,'hierarchy.select',state,actor,{'context_id':context_id,'level':max(a['level'] for a in actions),'roles':p.get('roles',['worker','editor'])})
        core.store.save(state);return {'actions':actions,'active':copy.deepcopy(h['active']),'agent_routes_created':0}
    raise ValueError('unknown hierarchy operation')
