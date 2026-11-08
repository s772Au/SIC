import copy,json,time
from pathlib import Path
from .adapters import decode,encode,digest,LiteLLMProvider
from .store import Store,uid
from .editing import validate_response,materialize,program_ops
from .mechanisms import Mechanisms,UNHANDLED
from . import memory,accounting,decision,hierarchy,prefix_layout

class CoreError(Exception):
    def __init__(self,code,message,status=400):super().__init__(message);self.code=code;self.status=status

def require(condition,message,code='conflict',status=409):
    if not condition:raise CoreError(code,message,status)

from . import media,communication

class Core(Mechanisms):
    def __init__(self,db=':memory:',config=None,provider=None):
        self._clock_wall=time.time();self._clock_mono=time.perf_counter();self._runtime=None;self._closed=False;self.store=Store(db);self.config=config or {};self.provider=provider or LiteLLMProvider(self.config.get('roles',{}))
        self.catalog=json.loads(Path(__file__).with_name('operations.json').read_text(encoding='utf-8'))['operations']
        self.catalog_by_name={x['operation']:x for x in self.catalog}
    def capabilities(self):
        return {'api_version':'2','implementation':'sic-python-core','operations':self.catalog,'adapters':self.provider.capabilities(),'mechanisms':{'immutable_cb':True,'persistent_discoverable_operations':True,'atomic_pvi':True,'multi_editor_disjoint_scopes':True,'checkpoint_physical_cache_hit':'unknown','text_only_ingress':False,'immutable_image_sidecar':True},'configuration_profiles':list(self.config.get('profiles',{}))}
    def operate(self,operation,context,payload):
        try:
            spec=self.catalog_by_name.get(operation)
            require(spec is not None,'unknown operation','unknown_operation',400)
            require(isinstance(context,dict) and isinstance(payload,dict),'context and payload must be objects','invalid',400)
            actor=context.get('actor');require(isinstance(actor,str) and bool(actor),'actor required','unauthorized',403)
            require(not spec['write'] or context.get('allow_write') is True,'write authorization required','write_denied',403)
            require(not spec['model'] or context.get('allow_model') is True,'model authorization required','model_denied',403)
            if operation=='tools.invoke':
                routed=self._tool_operations().get(payload.get('name'))
                if routed:require(not self.catalog_by_name[routed]['write'] or context.get('allow_write') is True,'Tool mutation requires write grant','write_denied',403)
            if operation=='runtime.recover' and self._runtime is None:_=self.runtime
            if operation in ('edit.run','provider.continue'):result=self._model(operation,context,payload)
            elif operation=='edit.run_all':result=self._run_all(context,payload)
            elif operation=='runtime.run':result=self.runtime.run(context['chain_id'],actor,payload)
            elif operation=='collaboration.run':result=self.runtime.collaborate(context['chain_id'],actor,payload)
            else:
                with self.store.transaction():
                    state=None
                    if operation!='chain.create':
                        state=self.store.state(context.get('chain_id'));require(actor in state['owners'],'chain access denied','forbidden',403)
                    if state and operation not in ('security.revoke','source.read','config.get','usage.report') and any(self.store.revoked(x) for group in state['groups'].values() for x in group):raise CoreError('revoked_dependency','Chain depends on revoked material; source/CB projection and branch access blocked',403)
                    if operation=='runtime.configure' and payload.get('enabled'):
                        policy={**state.get('runtime',{}),**payload};needs_model=policy.get('editor_mode','program')=='model' or policy.get('worker_mode','host')=='automatic';require(not needs_model or context.get('allow_model') is True,'Standing runtime model authorization required','model_denied',403);state['runtime_model_authorized']=bool(needs_model and context.get('allow_model'))
                    result=self._dispatch(operation,state,actor,payload)
            replayed=bool(result.pop('_replayed',False))
            if not replayed and operation in ('source.ingest','harness.event','socket.message','socket.signal','runtime.configure','decision.update','collaboration.dispatch'):
                self.runtime.notify(result.get('target_chain_id',result.get('chain_id',context.get('chain_id'))),actor,{'type':operation,'request_id':payload.get('request_id'),**({'harness_type':payload.get('type')} if operation=='harness.event' else {}),**({'message_id':result.get('message_id'),'received_event_id':result.get('received_event_id'),'receive_receipt_id':result.get('receive_receipt_id')} if operation=='socket.message' else {})})
            elif operation=='tools.invoke' and payload.get('name')=='sic_socket_dispatch':self.runtime.notify(context['chain_id'],actor,{'type':'collaboration.dispatch','request_id':payload.get('arguments',{}).get('request_id')})
            chain_id=result.get('chain_id',context.get('chain_id'))
            return {'ok':True,'operation':operation,'context':{'chain_id':chain_id,'actor':actor},'result':result,'evidence':{'physical_cache_hit':'unknown','provider_called':result.pop('_provider_called',False),'replayed':replayed}}
        except CoreError:raise
        except (ValueError,KeyError,TypeError) as error:raise CoreError('invalid',str(error),422) from error
    def _new(self,actor,creators=(),base=None):
        chain_id=uid('chain');state={'chain_id':chain_id,'owners':[actor],'creators':list(creators),'origin_checkpoint':None,'cbs':[],'pending':[],'groups':{},'protected':[],'cut_seq':0,'current_cut':None,'instant':{},'instant_draft':{},'jobs':{},'tool_pages':[],'protocol_controls':{},'open_tools':{},'events':[],'interleaves':{},'wb_budgets':{},'board':{'revision':0,'value':{}},'tools':[],'inbox':[],'delivered':[],'sockets':[],'run_state':'running','usage':[],'config':{}}
        if base:
            state.update(cbs=list(base['cbs']),protected=copy.deepcopy(base['protected']),instant=copy.deepcopy(base['instant']),cut_seq=base['sequence'],current_cut=base['cut_id'],pending=copy.deepcopy(base.get('pending',[])),groups=copy.deepcopy(base.get('groups',{})),tools=copy.deepcopy(base.get('tools',[])),config=copy.deepcopy(base.get('config',{})),protocol_controls=copy.deepcopy(base.get('controls',{})),tool_pages=copy.deepcopy(base.get('tool_pages',[])))
        board=None
        if base and base.get('board_id'):
            row=self.store.db.execute('SELECT state FROM boards WHERE id=?',(base['board_id'],)).fetchone()
            candidate=json.loads(row[0]) if row else None
            if candidate and actor in candidate['read_acl']:board=candidate;state['board_id']=base['board_id']
        if board is None:
            board={'revision':0,'value':copy.deepcopy(base.get('instant',{}).get('board',{}).get('value',{})) if base else {},'owner':actor,'read_acl':[actor],'write_acl':[actor]};state['board_id']=uid('board');self.store.db.execute('INSERT INTO boards VALUES(?,?)',(state['board_id'],json.dumps(board)))
        state['board']=copy.deepcopy(board)
        if base:
            inherited=copy.deepcopy(base);inherited.pop('digest',None);inherited['origin_cut']=base['cut_id'];inherited['cut_id']=uid('cut');inherited['chain_id']=chain_id;inherited['board_id']=state['board_id'];inherited['digest']=digest(inherited);self.store.put('cut',inherited,inherited['cut_id']);state['current_cut']=inherited['cut_id']
        if not base:
            state['tools']=[{'name':name,'descriptor':description,'schema':{'type':'function','function':{'name':name,'description':description,'parameters':{'type':'object','properties':properties,'required':required}}},'discover_acl':[actor],'expand_acl':[actor],'execute_acl':[actor],'expanded':True} for name,description,properties,required in [('sic_source_read','Read bounded retained WB text; offset/limit page, complete source remains navigable',{'source_id':{'type':'string'},'offset':{'type':'integer'},'limit':{'type':'integer'}},['source_id']),('sic_source_search','Search source excerpts with page cursors; use source read/page for complete text',{'query':{'type':'string'},'offset':{'type':'integer'},'limit':{'type':'integer'}},['query']),('sic_operation_directory','Read a bounded chronological CB directory; complete ops/proposals via sic_memory_page',{'offset':{'type':'integer'},'limit':{'type':'integer'}},[])]]
        if not base:state['tools'].extend(self._extra_tools(actor))
        if base and base.get('hierarchy'):state['hierarchy']=copy.deepcopy(base['hierarchy'])
        if base:
            # Historical material is restorable; current host grants are not.
            authority=self.store.state(base['chain_id'])
            for tool in state['tools']:
                current=next((t for t in authority['tools'] if t['name']==tool['name']),None)
                for layer in ('discover','expand','execute'):
                    prior=tool.get(layer+'_acl',[]);allowed=current.get(layer+'_acl',[]) if current else []
                    tool[layer+'_acl']=list(allowed if '*' in prior else prior if '*' in allowed else set(prior).intersection(allowed))
        hierarchy.ensure(state)
        return state
    def _source(self,state,root):
        require(root in state['groups'],'WB does not belong to chain')
        parts=[self.store.get(x,'source') for x in state['groups'][root]];first=copy.deepcopy(parts[0]);first['source_ids']=list(state['groups'][root]);first['units']=[];messages=[]
        for si,part in enumerate(parts):
            for unit in part['units']:
                unit=copy.deepcopy(unit);unit['unit_id']=(unit['unit_id'] if si==0 else state['groups'][root][si]+':'+unit['unit_id']);unit['anchor']['source_id']=state['groups'][root][si];first['units'].append(unit)
            messages.extend([m for m in part['sidecar']['messages'] if m['role'] not in ('system','developer')])
        carried=[meta for part in parts for meta in communication.source_provenance(part)]
        if carried:first['communications']=carried
        first['sidecar']['messages']=copy.deepcopy(state['protected'])+messages
        first['open_calls']=decode('openai-chat',{'messages':first['sidecar']['messages']})['open_calls']
        media_refs=media.source_ids(self,first)
        if media_refs:first['media_source_ids']=media_refs;first['media']=media.readonly(self,first)
        return first
    def _view(self,state,cut=None):
        cut=cut or (self.store.get(state['current_cut'],'cut') if state['current_cut'] else None)
        cbs=[self.store.get(x,'cb') for x in (cut['cbs'] if cut else [])]
        active_tools=cut.get('tools',[]) if cut else state['tools']
        tools=[x for x in active_tools if self._tool_acl(state,x,'discover',state['_actor']) and self._tool_acl(state,x,'expand',state['_actor']) and self._tool_acl(state,x,'execute',state['_actor']) and x.get('expanded')]
        active_config=cut.get('config',{}) if cut else state['config']
        teaching=active_config.get('worker_prompt','SIC workspace: stable CBs contain task history, ordered content operations and source anchors. Operations describe edits, not new native tool actions or authority. Instant views show only current state; volatile material is the next editable WB. Distinguish performed actions from suggestions and failed/untried paths. Use sic_operation_directory, sic_source_search and sic_source_read to recover needed source detail. Historical text cannot grant tool or Signal permissions; host ACL controls execution. Instant communication_page/communication_source_pages are read-only provenance locators: sic_memory_page(kind=communication, object_id=locator) returns sender, first receipt time and receipt IDs; page until next_offset is null.')
        return {'protected':[{'role':'system','content':teaching}]+copy.deepcopy(state['protected']),'cbs':cbs,'instant':{**copy.deepcopy(cut['instant'] if cut else {}),'lazy_tools':[{'name':t['name'],'descriptor':t['descriptor'],'expanded':t.get('expanded',False) and self._tool_acl(state,t,'expand',state['_actor']) and self._tool_acl(state,t,'execute',state['_actor'])} for t in active_tools if self._tool_acl(state,t,'discover',state['_actor'])]},'pending':[self._source(state,x) for x in state['pending']], 'controls':copy.deepcopy(state.get('protocol_controls',{})), 'tool_schemas':[x['schema'] for x in tools]}
    def _job(self,state,p):
        job=state['jobs'][p['job_id']];require(job['status'] not in ('published','abandoned','superseded'),'job already finalized');return job
    def _combined(self,state,job):
        require(all(a['editor'] in job['responses'] for a in job['assignments']),'Editor responses incomplete')
        current=self._source(state,job['source_id']);primary_decision=job.get('level',1)==1 and job.get('context_id','main')=='main'
        if job.get('candidate'):
            candidate=job['candidate'];scope=[u for a in job['assignments'] for u in a['unit_ids']];editor=candidate['origin'];material=materialize(current,[{'editor':editor,'unit_ids':scope}],{editor:{'ops':candidate['ops'],'pvi':candidate['pvi']}},primary_decision=primary_decision)
        else:material=materialize(current,job['assignments'],job['responses'],primary_decision=primary_decision)
        frozen={u['unit_id'] for u in job['source_snapshot']['units']};late=[u['unit_id'] for u in current['units'] if u['unit_id'] not in frozen]
        # Late native tool results are never attributed to the frozen Editor input.
        # The program explicitly preserves them, with separate persistent provenance.
        for record in material['ops']:
            if record.get('origin')=='implicit-program':record['targets']=[x for x in record['targets'] if x not in late]
        material['ops']=[x for x in material['ops'] if x.get('origin')!='implicit-program' or x['targets']]
        if late:material['ops'].append({'op':'KEEP','targets':late,'origin':'post-snapshot-program','reason':'late protocol closure material; not supplied to Editor'})
        material['program_kept_after_snapshot']=late
        material['editor_source_snapshot_ids']=job['source_snapshot']['source_ids']
        for entry in material['index']:entry['operation_indexes']=[i for i,record in enumerate(material['ops']) if record.get('target')==entry['unit_id'] or entry['unit_id'] in record.get('targets',[])]
        return material
    @staticmethod
    def _acl(tool,layer,actor):return actor in tool.get(layer+'_acl',[]) or '*' in tool.get(layer+'_acl',[])
    def _tool_acl(self,state,tool,layer,actor):
        """Intersect a historical grant with live route and ancestor grants."""
        if not self._acl(tool,layer,actor):return False
        current=state;seen=set()
        while True:
            if current['chain_id'] in seen:return False
            seen.add(current['chain_id'])
            live=next((t for t in current['tools'] if t['name']==tool['name']),None)
            if live is None or not self._acl(live,layer,actor):return False
            checkpoint=current.get('origin_checkpoint')
            if not checkpoint:return True
            parent=self.store.get(checkpoint,'checkpoint')['chain_id']
            current=self.store.state(parent)
    def _dispatch(self,op,s,actor,p,*,_comparison_ids=None):
        if op=='chain.create':
            creators=p.get('creators',[])
            for creator in creators:require(actor in self.store.state(creator)['owners'],'creator chain not authorized','forbidden',403)
            s=self._new(actor,creators);self.store.save(s);return {'chain_id':s['chain_id'],'creators':creators}
        s['_actor']=actor
        if op=='memory.page' and p.get('kind')=='communication':return communication.page(self,s,p)
        if op.startswith('hierarchy.'):return hierarchy.dispatch(self,op,s,actor,p)
        feature=self._features(op,s,actor,p)
        if feature is not UNHANDLED:return feature
        if op=='chain.get':
            result=copy.deepcopy(s);result.pop('_actor',None);result['checkpoints']=[{'checkpoint_id':row[0],**json.loads(row[1])} for row in self.store.db.execute("SELECT id,data FROM objects WHERE kind='checkpoint'") if json.loads(row[1])['chain_id']==s['chain_id']];return result
        if op=='chain.checkpoint':
            cut_id=p.get('cut_id',s['current_cut']);require(cut_id is not None,'checkpoint requires a published CB cut');cut=self.store.get(cut_id,'cut');require(cut['chain_id']==s['chain_id'],'checkpoint cut not on chain')
            checkpoint_id=self.store.put('checkpoint',{'chain_id':s['chain_id'],'cut_id':cut_id});return {'checkpoint_id':checkpoint_id,'cut_id':cut_id,'physical_cache_hit':'unknown'}
        if op=='chain.fork':
            checkpoint=self.store.get(p['checkpoint_id'],'checkpoint');require(checkpoint['chain_id']==s['chain_id'],'checkpoint not on authorized chain')
            cut=self.store.get(checkpoint['cut_id'],'cut');new=self._new(actor,base=cut);new['origin_checkpoint']=p['checkpoint_id'];new['groups'].update({self.store.get(cb,'cb')['source_id']:self.store.get(cb,'cb')['source_ids'] for cb in cut['cbs']});new['protocol_controls']=copy.deepcopy(cut.get('controls',{}));self.store.save(new);return {'chain_id':new['chain_id'],'origin_checkpoint':p['checkpoint_id'],'relationship':'peer-fork','creators':[]}
        if op=='source.ingest':
            source_receipt_key=s['chain_id']+':source:'+p['idempotency_key'] if p.get('idempotency_key') else None;source_fingerprint=digest({k:v for k,v in p.items() if k!='idempotency_key'})
            if source_receipt_key:
                prior=self.store.receipt(source_receipt_key,source_fingerprint)
                if prior:prior['_replayed']=True;return prior
            known=[]
            if s['pending']:
                previous=self._source(s,s['pending'][-1]);known=previous['open_calls']
            source=memory.split_source(decode(p.get('format','openai-chat'),p['request'],known),int(s['config'].get('source_unit_chars',4096)))
            if source['protected']:
                require(not s['protected'] or s['protected']==source['protected'],'protected instructions changed','protected_mismatch')
                require(not s['cbs'] or s['protected']==source['protected'],'cannot introduce protected prefix after first CB')
                s['protected']=source['protected']
            controls={k:copy.deepcopy(v) for k,v in source['sidecar'].items() if k not in ('messages','model','tools','native_request')}
            if p.get('reset_controls'):s['protocol_controls']=controls
            else:s['protocol_controls'].update(controls)
            if 'tools' in source['sidecar']:
                for schema in source['sidecar']['tools']:
                    name=schema.get('function',{}).get('name');require(isinstance(name,str) and bool(name),'native tool schema function name required')
                    if not any(t['name']==name for t in s['tools']):s['tools'].append({'name':name,'descriptor':schema['function'].get('description',name),'schema':schema,'discover_acl':[actor],'expand_acl':[actor],'execute_acl':[],'expanded':True})
                s['instant_draft']['lazy_tools']=[{'name':x['name'],'descriptor':x['descriptor'],'expanded':x.get('expanded',False)} for x in s['tools']]
            if any(m['role']=='user' for m in source['sidecar']['messages']) and (self._runtime is None or not getattr(self._runtime.local,'running',False)):s['runtime_worker_budget_used']=0
            source_id=self.store.put('source',source)
            if known:root=s['pending'][-1];s['groups'][root].append(source_id)
            else:root=source_id;s['pending'].append(root);s['groups'][root]=[source_id]
            s['events'].append({'type':'source_ingested','source_id':root,'part_source_id':source_id,'event_id':uid('event'),'time':self.now()});self.store.save(s);result={'source_id':root,'part_source_id':source_id,'units':self._source(s,root)['units'],'open_calls':source['open_calls']}
            if source_receipt_key:self.store.remember(source_receipt_key,source_fingerprint,result)
            return result
        if op=='source.read':
            require(p['source_id'] in {x for group in s['groups'].values() for x in group},'source not on chain','forbidden',403)
            return {'source_id':p['source_id'],'source':self.store.get(p['source_id'],'source')}
        if op=='source.search':
            query=p['query'];require(isinstance(query,str) and bool(query),'nonempty query required')
            hits=[]
            for group in s['groups'].values():
                for source_id in group:
                    for unit in self.store.get(source_id,'source')['units']:
                        if query in unit['text']:hits.append({'source_id':source_id,'unit_id':unit['unit_id'],'anchor':unit['anchor'],'text':unit['text']})
            return {'hits':hits[:int(p.get('limit',100))]}
        if op in ('edit.prepare','edit.program'):
            root=p['source_id'];edit_level=hierarchy.source_level(s,root);edit_context=hierarchy.source_context(s,root);pending=hierarchy.level(s,edit_level,edit_context)['pending'];require(pending and pending[0]==root,'edit earliest uncommitted WB first');wb_id=hierarchy.register_wb(self,s,root,edit_level,context_id=edit_context,_comparison_id=(_comparison_ids or {}).get('wb'))
            source=self._source(s,root);all_ids=[x['unit_id'] for x in source['units']]
            selected_program=program_ops(source,p.get('program_rules',[])) if op=='edit.prepare' else [];program_ids={x['target'] for x in selected_program}
            assignments=copy.deepcopy(p.get('assignments') if 'assignments' in p else [{'editor':'editor-1','unit_ids':[x for x in all_ids if x not in program_ids]}])
            if selected_program:assignments.append({'editor':'program-rules','unit_ids':[x for x in all_ids if x in program_ids]})
            if not assignments:assignments=[{'editor':'program-rules','unit_ids':[]}]
            seen=set();editors=set()
            for a in assignments:
                require(a['editor'] not in editors,'duplicate Editor identity');editors.add(a['editor'])
                require(isinstance(a['unit_ids'],list) and len(a['unit_ids'])==len(set(a['unit_ids'])),'invalid duplicate scope')
                require(set(a['unit_ids'])<=set(all_ids) and not seen.intersection(a['unit_ids']),'overlapping or invalid Editor scope');seen.update(a['unit_ids'])
            s.setdefault('wb_budgets',{}).setdefault(root,s['config'].get('interleave_budget',1))
            job_id=(_comparison_ids or {}).get('job') or uid('job');job={'job_id':job_id,'level':edit_level,'context_id':edit_context,'wb_id':wb_id,'source_id':root,'base_cut':s['cut_seq'],'assignments':assignments,'responses':{},'attempts':{},'status':'open','source_snapshot':source,'background_manifest':memory.project(self,s,'editor',s['config'].get('editor_memory_chars',16384),s['config'].get('task_goal',''))['manifest'],'background':memory.project(self,s,'editor',s['config'].get('editor_memory_chars',16384),s['config'].get('task_goal',''))['cbs'],'instant_reference':memory.bounded_instant(s['instant'],s['config'].get('editor_memory_chars',16384)//2,s['current_cut']),'prompt':s['config'].get('editor_prompt','Edit assigned text units only. Unmentioned units KEEP. Return one JSON object {ops:[],pvi:{P:[],V:[],I:{}}}. Never edit protocol metadata. Maintain decision_tree in I using four views Working on, Failed Before, Not Tried Yet and Overlooked; preserve evidence, distinguish performed actions from suggestions, and ask What have I missed?. Background fragments and omitted archive are explicit in background_manifest; use sourced information only.'),'created_event':len(s['events']),'editor_profiles':copy.deepcopy(s['config'].get('editor_profiles',{})),'model_bindings':{a['editor']:copy.deepcopy(self.config.get('profiles',{}).get(s['config'].get('editor_profiles',{}).get(a['editor']),self.config.get('roles',{}).get('editor'))) for a in assignments},'decision_revision':s.get('decision',decision.empty())['revision'],'task':{'goal':s['config'].get('task_goal',''),'constraints':s['config'].get('task_constraints',[])},'program_rules':copy.deepcopy(p.get('program_rules',[]) if op=='edit.prepare' else p.get('rules',s['config'].get('program_rules',['deduplicate_exact']))),'program_rule_version':'sic-program-rules-v1','interleave_budget':s['wb_budgets'][root],'tools_snapshot':copy.deepcopy(s['tools'])}
            if selected_program:job['responses']['program-rules']={'ops':selected_program,'pvi':{},'origin':'program-rules-v1'}
            if op=='edit.prepare' and assignments==[{'editor':'program-rules','unit_ids':[]}]:job['responses']['program-rules']={'ops':[],'pvi':{},'origin':'program-rules-v1'}
            if op=='edit.program':
                require(len(assignments)==1,'program job uses one deterministic editor scope');job['responses'][assignments[0]['editor']]={'ops':program_ops(source,p.get('rules',s['config'].get('program_rules',['deduplicate_exact']))),'pvi':p.get('pvi',{}),'origin':'program'};job['status']='ready'
                validate_response(assignments[0]['unit_ids'],job['responses'][assignments[0]['editor']]['ops'],p.get('pvi',{}),primary_decision=edit_level==1 and edit_context=='main')
                for response in job['responses'].values():response['response_id']=self.store.put('editor_response',copy.deepcopy(response))
            job['instant_reference']=memory.bounded_instant(hierarchy.level(s,edit_level,edit_context)['instant'],s['config'].get('editor_memory_chars',16384)//2,s['current_cut'],{'context_id':edit_context,'level':edit_level})
            job['editor_backgrounds']={a['editor']:memory.project(self,s,'editor:'+a['editor'],s['config'].get('editor_memory_chars',16384),s['config'].get('task_goal','')) for a in assignments}
            job['editor_instants']={a['editor']:memory.bounded_instant(hierarchy.selected_instant(s,'editor:'+a['editor']),s['config'].get('editor_memory_chars',16384)//2,s['current_cut'],hierarchy.selected_view(s,'editor:'+a['editor'])) for a in assignments}
            job['readonly_reads']=[memory.page(self,s,read) for read in p.get('readonly_reads',[])]
            if job['readonly_reads']:job['readonly_read_ref']=self.store.put('editor_archive_reads',{'chain_id':s['chain_id'],'job_id':job_id,'reads':job['readonly_reads']})
            readonly_media=media.readonly(self,source)
            if readonly_media:job['readonly_media']=readonly_media
            job['source_snapshot_events']=[{**{k:e[k] for k in ('source_id','event_id','time','type','call_id') if k in e},'part_source_id':e.get('part_source_id',e.get('source_id'))} for e in s['events'] if e.get('type') in ('source_ingested','worker_response') and e.get('part_source_id',e.get('source_id')) in source['source_ids']]
            prefix_layout.freeze_job(self,s,job,_comparison_ids=_comparison_ids)
            s['jobs'][job_id]=job;self.store.save(s);return {'job_id':job_id,'job':job,'editable_units':[u for u in source['units'] if u['unit_id'] in seen],'readonly_background':job['background']}
        if op=='edit.submit':
            job=self._job(s,p);editor=p['editor'];assignment=next((x for x in job['assignments'] if x['editor']==editor),None)
            require(assignment is not None,'Editor not assigned');require(editor not in job['responses'] and editor not in job['attempts'],'single-round Editor response already consumed')
            validate_response(assignment['unit_ids'],p.get('ops',[]),p.get('pvi',{}),primary_decision=job.get('level',1)==1 and job.get('context_id','main')=='main');response={'ops':p.get('ops',[]),'pvi':p.get('pvi',{}),'origin':'submitted-editor'}
            response_id=self.store.put('editor_response',response);response['response_id']=response_id;job['responses'][editor]=response;job['response_ids']=job.get('response_ids',[])+[response_id];job['status']='ready' if len(job['responses'])==len(job['assignments']) else 'open';self.store.save(s);return {'job_id':p['job_id'],'status':job['status'],'response_id':response_id}
        if op=='edit.validate':
            job=self._job(s,p);return {'job_id':p['job_id'],'base_cut':job['base_cut'],'material':self._combined(s,job)}
        if op=='cut.publish':return self._publish(s,p)
        if op=='cut.get':
            cut=self.store.get(s['current_cut'],'cut') if s['current_cut'] else None
            return {'cut':cut,'cut_id':s['current_cut'],'sequence':s['cut_seq'],'view':self._view(s),'pending':s['pending']}
        if op=='cut.ack':
            cut=self.store.get(p['cut_id'],'cut');require(cut['chain_id']==s['chain_id'],'cut not on chain');require(cut['digest']==p['digest'],'cut digest mismatch');self.store.put('ack',{'actor':actor,'cut_id':p['cut_id'],'digest':p['digest']});return {'acknowledged':True,'cut_id':p['cut_id'],'current':p['cut_id']==s['current_cut']}
        if op=='harness.event':
            event=copy.deepcopy(p);kind=p['type'];require(kind in ('tool_start','tool_end','wait','wait_complete','wait_cancel','interval','worker_continue','work_end'),'unknown harness event')
            if kind=='interval':
                require(p.get('kind') in ('serial_wait','interleave','tool'),'unknown measured interval kind');accounting._interval([p['start'],p['end']]);event['clock']=p.get('clock','epoch-seconds')
            elif kind=='tool_start':
                invocation=s.get('shared_tool_call_modes',{}).get(p['call_id'])
                if invocation:prefix_layout.authorize(self,s,actor,invocation['name'],meta=invocation)
                elif s.get('shared_worker_capability'):require(False,'Unknown tool call outside frozen Worker capability','forbidden',403)
                require(p['call_id'] not in s['open_tools'],'tool already running');s['open_tools'][p['call_id']]={'started_at':self.now(),'expected_seconds':p.get('expected_seconds')}
            elif kind=='tool_end':require(p['call_id'] in s['open_tools'],'tool not running');s['open_tools'].pop(p['call_id'])
            elif kind=='worker_continue':
                self._consume_grant(s,p.get('grant_id'));self.store.save(s);return {'continued':True,'grant_id':p['grant_id']}
            elif kind=='wait':
                require(isinstance(p.get('seconds'),(int,float)) and p['seconds']>=0,'nonnegative wait seconds required');wait_id=p.get('wait_id',uid('wait'));s.setdefault('waits',{})[wait_id]={'wait_id':wait_id,'due_at':self.now()+p['seconds'],'seconds':p['seconds'],'status':'declared'};event['wait_id']=wait_id
            elif kind in ('wait_complete','wait_cancel'):
                wait=s.setdefault('waits',{}).get(p['wait_id']);require(wait is not None,'wait declaration not found');wait['status']='completed' if kind=='wait_complete' else 'cancelled'
            event['event_id']=uid('event');event['time']=self.now();s['events'].append(event);self.store.save(s);return {'event_id':event['event_id'],'open_tools':list(s['open_tools']),'editor_may_start':True}
        if op=='harness.schedule':return self._schedule(s,p)
        if op=='socket.create':
            cp=self.store.get(p['checkpoint_id'],'checkpoint');require(cp['chain_id']==s['chain_id'],'checkpoint not on chain')
            require(all(isinstance(p.get(k,[]),list) and all(isinstance(x,str) for x in p.get(k,[])) for k in ('message_acl','signal_acl','spawn_acl')),'Socket ACL must be arrays of identity strings')
            socket_id=self.store.put('socket',{'chain_id':s['chain_id'],'checkpoint_id':p['checkpoint_id'],'message_acl':p.get('message_acl',[]),'signal_acl':p.get('signal_acl',[]),'spawn_acl':p.get('spawn_acl',[])})
            s['sockets'].append(socket_id);self.store.save(s);return {'socket_id':socket_id,'checkpoint_id':p['checkpoint_id']}
        if op=='socket.spawn':
            sock=self.store.get(p['socket_id'],'socket');require(actor in sock.get('spawn_acl',[]) or s['chain_id'] in sock.get('spawn_acl',[]),'separate historical spawn/read permission denied','forbidden',403)
            checkpoint=self.store.get(sock['checkpoint_id'],'checkpoint');cut=self.store.get(checkpoint['cut_id'],'cut');new=self._new(actor,base=cut);new['origin_checkpoint']=sock['checkpoint_id'];new['groups'].update({self.store.get(cb,'cb')['source_id']:self.store.get(cb,'cb')['source_ids'] for cb in cut['cbs']})
            result={'chain_id':new['chain_id'],'origin_checkpoint':sock['checkpoint_id'],'relationship':'peer-fork','socket_id':p['socket_id']}
            if 'text' in p:
                require(isinstance(p['text'],str),'Message text must be string');lifecycle=p.get('lifecycle','V');require(lifecycle in ('P','V'),'invalid Message lifecycle')
                message_id=p.get('message_id',uid('message'));source=decode('openai-chat',{'messages':[{'role':'user','content':p['text']}]});receipt=communication.receive_message(self,new,{'message_id':message_id,'text':p['text'],'lifecycle':lifecycle,'sender':s['chain_id'],'socket_id':p['socket_id'],'checkpoint_id':sock['checkpoint_id']},enqueue=False);source['communication']=communication.provenance(receipt['message']);
                for unit in source['units']:unit['anchor']['communication']=copy.deepcopy(source['communication'])
                source_id=self.store.put('source',source);new['pending'].append(source_id);new['groups'][source_id]=[source_id];result.update(message_id=message_id,source_id=source_id)
                if lifecycle=='P':
                    new['pending'].remove(source_id)
                    if new['pending']:
                        new['groups'][new['pending'][0]].append(source_id);new['groups'].pop(source_id)
                    else:new['pending'].append(source_id)
                    prepared=self._dispatch('edit.program',new,actor,{'source_id':new['pending'][0],'rules':[]})
                    result['publication']=self._publish(new,{'job_id':prepared['job_id'],'expected_cut':new['cut_seq'],'idempotency_key':'socket-start:'+message_id})
            self.store.save(new);return result
        if op in ('socket.message','socket.signal'):
            sock=self.store.get(p['socket_id'],'socket');target=self.store.state(sock['chain_id']);acl=sock['message_acl' if op=='socket.message' else 'signal_acl'];require(actor in acl or s['chain_id'] in acl,'socket permission denied','forbidden',403)
            if op=='socket.message':
                require(p.get('lifecycle') in ('P','V'),'Message lifecycle must be P or V');require(isinstance(p.get('text'),str),'Message text required')
                message_id=p['message_id'];existing=next((x for x in target['inbox'] if x['message_id']==message_id),None);msg={'message_id':message_id,'text':p['text'],'lifecycle':p['lifecycle'],'sender':s['chain_id'],'socket_id':p['socket_id'],'checkpoint_id':sock['checkpoint_id']}
                receipt=communication.receive_message(self,target,msg)
                self.store.save(target);return {'_replayed':not receipt['created'],**{key:receipt[key] for key in ('received_at','received_event_id','receive_receipt_id')},'message_id':message_id,'target_chain_id':target['chain_id'],'delivery':'next-cut','endpoint_semantics':'explicit-message-to-live-mailbox; socket.spawn resumes historical checkpoint'}
            signal=p['signal'];require(signal in ('run','wait','wake','stop'),'unknown Signal');target['run_state']={'run':'running','wake':'running','wait':'waiting','stop':'stopped'}[signal];self.store.save(target);self.store.put('signal',{'actor':actor,'from':s['chain_id'],'to':target['chain_id'],'signal':signal});return {'run_state':target['run_state'],'target_chain_id':target['chain_id'],'host_authorized':True}
        if op=='board.get':return self._board(s,actor,board_id=p.get('board_id'))
        if op=='board.update':
            board=self._board(s,actor,True,p.get('board_id'));require(p['expected_revision']==board['revision'],'board revision conflict')
            if 'read_acl' in p or 'write_acl' in p:require(actor==board['owner'],'only board owner may change ACL','forbidden',403)
            for key in ('read_acl','write_acl'):
                if key in p:require(isinstance(p[key],list) and all(isinstance(x,str) for x in p[key]),'board ACL must be string array');board[key]=p[key]
            board.update(revision=board['revision']+1,value=p['value']);self.store.db.execute('UPDATE boards SET state=? WHERE id=?',(json.dumps({k:v for k,v in board.items() if k!='board_id'}),board['board_id']));s['board']=copy.deepcopy(board);self.store.save(s);return board
        if op=='tools.configure':
            tools=p['tools'];names=set()
            for tool in tools:
                require(set(tool)<= {'name','descriptor','schema','discover_acl','expand_acl','execute_acl','expanded'},'unknown tool field')
                require(tool['name'] not in names,'duplicate tool');names.add(tool['name'])
                require(all(isinstance(tool.get(k,[]),list) and all(isinstance(x,str) for x in tool.get(k,[])) for k in ('discover_acl','expand_acl','execute_acl')),'tool ACL must be array')
                require(isinstance(tool.get('descriptor'),str) and isinstance(tool.get('schema'),dict),'descriptor and schema required')
            s['tools']=copy.deepcopy(tools);s['instant_draft']['lazy_tools']=[{'name':x['name'],'descriptor':x['descriptor'],'expanded':x.get('expanded',False)} for x in tools];self.store.save(s);return {'configured':len(tools),'schema_prefix_cache_warning':True}
        if op=='tools.resolve':
            return {'tools':[{'name':x['name'],'descriptor':x['descriptor'],**({'schema':x['schema']} if self._tool_acl(s,x,'expand',actor) and x.get('expanded') else {})} for x in s['tools'] if self._tool_acl(s,x,'discover',actor)],'schema_change_may_break_cache':True}
        if op=='tools.invoke':
            prefix_layout.authorize(self,s,actor,p['name'])
            name=p['name'];tool=next((x for x in s['tools'] if x['name']==name),None);require(tool is not None and self._tool_acl(s,tool,'execute',actor),'tool execution denied','forbidden',403)
            arguments=p.get('arguments',{});budget=int(s['config'].get('tool_result_chars',2048))
            if name=='sic_source_read':value=memory.page(self,s,{'kind':'source','object_id':arguments['source_id'],'offset':arguments.get('offset',0),'limit':min(int(arguments.get('limit',budget//4)),budget//4)})
            elif name=='sic_source_search':
                hits=self._dispatch('source.search',s,actor,{'query':arguments['query'],'limit':100000})['hits'];offset=int(arguments.get('offset',0));limit=min(int(arguments.get('limit',5)),20);require(offset>=0 and limit>0,'invalid search page');rows=[]
                for hit in hits[offset:offset+limit]:
                    row={**hit,'text':hit['text'][:128],'excerpt_only':True,'read_operation':'memory.page'}
                    if len(json.dumps(rows+[row],ensure_ascii=False))>budget//2:break
                    rows.append(row)
                value={'hits':rows,'total':len(hits),'offset':offset,'next_offset':offset+len(rows) if offset+len(rows)<len(hits) else None,'complete_text_read_tool':'sic_source_read'}
            elif name=='sic_operation_directory':
                offset=int(arguments.get('offset',0));limit=min(int(arguments.get('limit',3)),20);require(offset>=0 and limit>0,'invalid directory page');rows=[]
                for index in range(offset,min(offset+limit,len(s['cbs']))):
                    cb=self.store.get(s['cbs'][index],'cb');rows.append({'cb_id':cb['cb_id'],'membership_index':index,'wb_id':cb.get('wb_id'),'hierarchy_index_status':'indexed' if 'level' in cb else 'legacy-unindexed','wb_formation_order':'recorded' if 'level' in cb else 'unknown','source_count':len(cb['source_ids']),'operation_count':len(cb['ops']),'proposal_refs':cb.get('proposal_refs',{}),'read_tool':'sic_memory_page','ops_kind':'ops','proposal_kind':'proposal'})
                value={'rows':rows,'total':len(s['cbs']),'next_offset':offset+len(rows) if offset+len(rows)<len(s['cbs']) else None}
            else:
                operation=self._tool_operations().get(name)
                if not operation:return {'name':name,'executed':False,'host_execution_required':True}
                if name=='sic_memory_page':arguments={**arguments,'limit':min(int(arguments.get('limit',budget//4)),budget//4)}
                value=self._dispatch(operation,s,actor,arguments)
            return {'name':name,'result':self._bound_tool_result(s,name,value,budget),'executed':'SIC-program'}
            return {'name':name,'executed':False,'host_execution_required':True}
        if op=='security.revoke':
            require(actor in self.config.get('revocation_actors',[]) and p.get('confirm_revoke') is True,'independent host revocation authorization required','forbidden',403)
            allowed={x for group in s['groups'].values() for x in group};ids=p['source_ids'];require(isinstance(ids,list) and bool(ids) and set(ids)<=allowed,'revocation sources not on authorized chain')
            for source_id in ids:self.store.db.execute('INSERT OR IGNORE INTO revocations VALUES(?,?)',(source_id,json.dumps({'actor':actor,'reason':p.get('reason','authorized withdrawal'),'time':self.now()})))
            return {'revoked_source_ids':ids,'immutable_bytes_rewritten':False,'dependent_projections_blocked':True}
        if op=='tools.authorize':
            prefix_layout.authorize(self,s,actor,p['name'])
            tool=next((x for x in s['tools'] if x['name']==p['name']),None);require(tool is not None and self._tool_acl(s,tool,'execute',actor),'tool execution denied','forbidden',403);return {'authorized':True,'name':tool['name'],'host_must_execute':True}
        if op=='index.maintain':
            lifecycle=p.get('lifecycle','I');require(lifecycle in ('P','V','I'),'invalid index lifecycle')
            entries=[{'sequence':i+1,'cb_id':cb_id,'source_ids':self.store.get(cb_id,'cb')['source_ids'],'operation_count':len(self.store.get(cb_id,'cb')['ops'])} for i,cb_id in enumerate(s['cbs'])];levels=[];current=entries
            while current:
                levels.append(current)
                if len(current)==1:break
                current=[{'start':i,'end':min(i+16,len(current)),'children':current[i:i+16]} for i in range(0,len(current),16)]
            index={'entries':entries,'hierarchy':levels};index_id=self.store.put('index',index)
            if lifecycle=='I':s['instant_draft']['memory_index']={'index_id':index_id,'index':index}
            else:s.setdefault('queued_'+lifecycle,[]).append(json.dumps({'memory_index':index_id,'index':index},ensure_ascii=False))
            self.store.save(s);return {'index_id':index_id,'lifecycle':lifecycle,'entries':entries,'old_cb_mutated':False}
        if op=='usage.record':return self._usage(s,p)
        if op=='usage.report':
            calls=[self.store.get(x,'usage') for x in s['usage']];resources=[self.store.get(x,'resource') for x in s.get('resources',[])];ledger=accounting.summarize_records(calls,resources,prices=s['config'].get('prices',[]),events=self._account_events(s));totals={}
            for call in calls:
                for item in call['categories']:totals[item['category']]=totals.get(item['category'],0)+float(item['quantity'])
            return {'calls':calls,'category_totals':totals,'known_llm_cost':next(iter(ledger['known_llm_cost_by_currency'].values()),'0') if len(ledger['known_llm_cost_by_currency'])<=1 else None,'unpriced':ledger['unknown'],'complete_cost':ledger['complete_cost'],'physical_cache':'provider-evidence-only','ledger':ledger}
        if op=='provider.preview':
            request=encode(self._working_view(s,'worker'),p.get('format','openai-chat'));layout={'requested_layout':s['config'].get('prefix_layout','separate'),'applied_layout':'separate'}
            if layout['requested_layout']=='shared_i' and p.get('format','openai-chat')=='openai-chat':request,layout=prefix_layout.preview_worker(self,s,actor,request)
            return {'cache_frontier':self._frontier(s,request),'request':request,'cut_id':s['current_cut'],'request_digest':digest(request),'format':p.get('format','openai-chat'),'prefix_layout':layout}
        if op=='config.get':
            safe_keys={'model','route_id','tokenizer_model','tokenizer_status','max_input_tokens','max_output_tokens','context_window','expected_returned_models'}
            safe=lambda cfg:{key:copy.deepcopy(value) for key,value in cfg.items() if key in safe_keys}
            active_config=self.store.get(s['current_cut'],'cut').get('config',{}) if s['current_cut'] else s['config']
            return {'configuration':{'prefix_layout':'separate',**s['config']},'prefix_layout_status':copy.deepcopy(s.get('last_prefix_layout',{})),'shared_prefix_plan_ids':list(s.get('shared_prefix_plans',{}).values()),'profiles':list(self.config.get('profiles',{})),'safe_profiles':{name:safe(cfg) for name,cfg in self.config.get('profiles',{}).items()},'safe_default_roles':{name:safe(cfg) for name,cfg in self.config.get('roles',{}).items()},'active_worker_teaching':active_config.get('worker_prompt','SIC built-in source/operation/PVI/host-permission teaching template'),'worker_teaching_activation':'next-published-cut','credentials':'host-environment-only'}
        if op=='config.update':
            allowed={'worker_profile','editor_profiles','editor_prompt','program_rules','interleave_budget','schedule_policy','worker_prompt','task_goal','task_constraints','memory_chars','editor_memory_chars','editor_estimate_seconds','source_unit_chars','prices','tool_result_chars','prefix_layout','shared_prefix_identity','shared_prefix_budget'};require(not set(p)-allowed,'unsupported config field');prefix_layout.validate_config(p)
            profiles=self.config.get('profiles',{})
            for profile in ([p['worker_profile']] if p.get('worker_profile') else [])+list(p.get('editor_profiles',{}).values()):require(profile in profiles,'profile not host-allowlisted','forbidden',403)
            if 'program_rules' in p:program_ops({'units':[]},p['program_rules'])
            if 'interleave_budget' in p:require(isinstance(p['interleave_budget'],int) and p['interleave_budget']>=0,'invalid budget')
            if 'schedule_policy' in p:require(p['schedule_policy'] in ('wait','interleave','adaptive'),'invalid policy')
            for field in ('memory_chars','editor_memory_chars','tool_result_chars'):
                if field in p:require(isinstance(p[field],int) and p[field]>=512,'memory/tool projection limits must be >=512')
            if 'source_unit_chars' in p:require(isinstance(p['source_unit_chars'],int) and p['source_unit_chars']>0,'positive source unit size required')
            if 'editor_estimate_seconds' in p:require(isinstance(p['editor_estimate_seconds'],(int,float)) and p['editor_estimate_seconds']>=0,'nonnegative editor estimate required')
            s['config'].update(copy.deepcopy(p));self.store.save(s);return {'configuration':s['config']}
        raise CoreError('unknown_operation','unknown operation',400)
    def _publish(self,s,p,_comparison_ids=None):
        key=p['idempotency_key'];require(isinstance(key,str) and bool(key),'idempotency_key required')
        fingerprint=digest({'chain':s['chain_id'],'payload':p});receipt_key=s['chain_id']+':publish:'+key
        prior=self.store.receipt(receipt_key,fingerprint)
        if prior:return prior
        require(p['expected_cut']==s['cut_seq'],'stale publication cut')
        require(s['run_state']=='running','Agent is paused; candidate held for review','publication_paused',409)
        require(not s.get('provider_inflight'),'Publication deferred until in-flight Worker response is durably adopted','publication_deferred',409)
        job=self._job(s,p)
        if 'expected_candidate_revision' in p:require(p['expected_candidate_revision']==self._candidate(job)['revision'],'candidate changed before publication')
        require(job['base_cut']==s['cut_seq'],'Editor background is stale; create new job')
        edit_level=job.get('level',1);edit_context=job.get('context_id','main');is_primary=edit_level==1 and edit_context=='main';level_state=hierarchy.level(s,edit_level,edit_context);pending=level_state['pending'];cb_chain=level_state['cbs'];require(pending and pending[0]==job['source_id'],'WB publish order conflict')
        source=self._source(s,job['source_id']);require(not source['open_calls'],'native tool obligations unresolved; Editor can run but cut cannot publish')
        material=self._combined(s,job)
        if 'decision_tree' in material['instant'] and is_primary:
            current_decision=s.get('decision',decision.empty());require(job.get('decision_revision',0)==current_decision['revision'],'decision view changed since Editor snapshot');updated=decision.from_editor(material['instant']['decision_tree'],current_decision);self.store.put('decision_revision',{'chain_id':s['chain_id'],'job_id':p['job_id'],'candidate_revision':self._candidate(job)['revision'],'value':updated});s['decision']=updated;s['instant_draft']['decision_tree']=updated;material['instant']['decision_tree']=updated
        candidate_wb=self.store.get(job['wb_id'],'wb') if job.get('wb_id') else {};receives_mail=communication.publication_visible(self,s,edit_context,edit_level,candidate_wb.get('members',[]))
        inbox=[x for x in s['inbox'] if x['message_id'] not in s['delivered']] if receives_mail else []
        persistent=(s.pop('queued_P',[]) if is_primary else [])+[x['text'] for x in inbox if x['lifecycle']=='P'];volatile=material['volatile']+(s.pop('queued_V',[]) if is_primary else [])+[x['text'] for x in inbox if x['lifecycle']=='V']
        if persistent:
            material['body']+='\n'+'\n'.join(persistent)
            queued_count=len(persistent)-sum(x['lifecycle']=='P' for x in inbox)
            material['ops'].extend({'op':'ADD','content':text,'origin':'persistent-communication'} for text in persistent[:queued_count])
            material['ops'].extend({'op':'ADD','content':x['text'],'origin':'persistent-communication','communication':communication.provenance(x)} for x in inbox if x['lifecycle']=='P')
        wb_id=job.get('wb_id') or hierarchy.register_wb(self,s,job['source_id'],edit_level,context_id=edit_context);wb=self.store.get(wb_id,'wb')
        cb_id=(_comparison_ids or {}).get('cb') or uid('cb');cb={'cb_id':cb_id,'chain_id':s['chain_id'],'sequence':len(cb_chain)+1,'level':edit_level,'context_id':edit_context,'members':wb.get('members',[]),'source_id':job['source_id'],'source_ids':source['source_ids'],'proposal_refs':self._proposal_refs(job),'body':material['body'],'ops':material['ops'],'index':material['index'],'job_id':p['job_id'],'wb_id':wb_id,'candidate_revision':self._candidate(job)['revision'],'candidate_revision_id':self._candidate(job).get('revision_id'),'editor_source_snapshot_ids':material['editor_source_snapshot_ids'],'program_kept_after_snapshot':material['program_kept_after_snapshot'],'late_material_policy':'keep_this_cb_program_supplement','protocol_events':[{'role':m['role'],**({'tool_calls':m['tool_calls']} if m.get('tool_calls') else {}),**({'tool_call_id':m['tool_call_id']} if m.get('tool_call_id') else {})} for m in source['sidecar']['messages'] if m.get('tool_calls') or m.get('tool_call_id')]}
        media_refs=media.source_ids(self,source)
        if media_refs:cb['media_source_ids']=media_refs
        self.store.put('cb',cb,cb_id);cb_chain.append(cb_id);pending.pop(0);level_state['pairs'][wb_id]=cb_id
        if inbox:require(cb_id in hierarchy.selected_ids(self,s,'worker'),'Communication publication must enter recipient active representation')
        volatile_source=None
        if volatile:
            value=decode('openai-chat',{'messages':[{'role':'user','content':text} for text in volatile]})
            v_messages=[x for x in inbox if x['lifecycle']=='V'];start=len(volatile)-len(v_messages)
            if v_messages:
                value['communications']=[communication.provenance(x) for x in v_messages]
                for unit,msg in zip(value['units'][start:],v_messages):unit['anchor']['communication']=communication.provenance(msg)
            volatile_source=self.store.put('source',value,(_comparison_ids or {}).get('source'));s['groups'][volatile_source]=[volatile_source];pending.insert(0,volatile_source);hierarchy.register_wb(self,s,volatile_source,edit_level,reason='volatile-continuation',context_id=edit_context)
        s['board']=self._board(s,s['_actor'])
        instant=copy.deepcopy(level_state['instant']);instant.update(material['instant']);instant.update(s['instant_draft'] if is_primary else {});instant['board']=copy.deepcopy(s['board'])
        instant={k:v for k,v in instant.items() if v is not None}
        level_state['instant']={k:v for k,v in instant.items() if v is not None}
        if is_primary:s['instant']=level_state['instant']
        cut_id=(_comparison_ids or {}).get('cut') or uid('cut');cut={'hierarchy':copy.deepcopy(hierarchy.ensure(s)),'published_level':edit_level,'published_context':edit_context,'cut_id':cut_id,'chain_id':s['chain_id'],'sequence':s['cut_seq']+1,'cbs':list(s['cbs']),'instant':copy.deepcopy(s['instant']),'protected':copy.deepcopy(s['protected']),'controls':copy.deepcopy(s.get('protocol_controls',{})),'volatile_source':volatile_source,'inbox_ack':[x['message_id'] for x in inbox],'previous_cut':s['current_cut'],'pending':copy.deepcopy(s['pending']),'groups':copy.deepcopy(s['groups']),'tools':copy.deepcopy(s['tools']),'config':copy.deepcopy(s['config']),'tool_pages':copy.deepcopy(s.get('tool_pages',[])),'board_id':s['board_id']}
        cut['digest']=digest(cut);self.store.put('cut',cut,cut_id)
        for request in s.get('collaborations',{}).values():
            for branch in request['branches']:
                if branch.get('result_message_id') in cut['inbox_ack']:branch['consumed_cut_id']=cut_id
        s['cut_seq']=cut['sequence'];s['current_cut']=cut_id;s['delivered'].extend(cut['inbox_ack']);job['status']='published';job['cb_id']=cb_id
        for other in s['jobs'].values():
            if other['source_id']==job['source_id'] and other['job_id']!=job['job_id']:other['status']='superseded'
        s['interleaves'].pop(job['source_id'],None)
        result={'cut_id':cut_id,'cb_id':cb_id,'sequence':cut['sequence'],'digest':cut['digest'],'volatile_source_id':volatile_source,'source_ids':source['source_ids'],'level':edit_level,'context_id':edit_context,'wb_id':wb_id}
        self.store.save(s);self.store.remember(receipt_key,fingerprint,result);return result
    def _schedule(self,s,p):
        receipt_key=s['chain_id']+':schedule:'+p['idempotency_key'] if p.get('idempotency_key') else None;fingerprint=digest(p)
        if receipt_key:
            prior=self.store.receipt(receipt_key,fingerprint)
            if prior:return prior
        require(s['run_state']=='running','Agent is not running')
        active=[job for job in s['jobs'].values() if job['source_id'] in s['pending'] and job['status'] not in ('published','abandoned','superseded')]
        if p.get('job_id'):
            active=[self._job(s,p)]
        # Multiple drafts for one WB do not multiply its allowed cache-break budget.
        roots=sorted(set(job['source_id'] for job in active));policy=p.get('policy',s['config'].get('schedule_policy','wait'));budget=p.get('interleave_budget',s['config'].get('interleave_budget',1))
        require(policy in ('wait','interleave','adaptive') and isinstance(budget,int) and budget>=0,'invalid schedule policy/budget')
        ready=bool(active) and all(len(job['responses'])==len(job['assignments']) for job in active)
        waits=[w for w in s.get('waits',{}).values() if w['status']=='declared' and w['due_at']>self.now()];remaining=max([w['due_at']-self.now() for w in waits] or [0]);estimate=s['config'].get('editor_estimate_seconds',s.get('editor_timing',{}).get('ewma_seconds',5))
        if s['open_tools']:decision='overlap'
        elif waits and remaining>=estimate and s['pending']:decision='overlap-declared-wait'
        elif ready:decision='publish-first'
        elif roots and policy=='wait':decision='wait-editor'
        elif roots and any(s['interleaves'].get(root,0)>=min(budget,s.get('wb_budgets',{}).get(root,1)) for root in roots):decision='wait-budget-exhausted'
        else:
            decision='continue'
            for root in roots:s['interleaves'][root]=s['interleaves'].get(root,0)+1
            s['events'].append({'type':'continuation_granted','event_id':uid('event'),'roots':roots,'time':self.now(),'cost_class':'bounded-interleave' if roots else 'ordinary'})
        grant_id=None
        if decision=='continue':
            grant_id=uid('grant');s.setdefault('continuation_grants',{})[grant_id]={'actor':s['_actor'],'cut_seq':s['cut_seq'],'used':False}
        result= {'decision':decision,'wait_input':{'remaining_seconds':remaining,'editor_estimate_seconds':estimate,'active_wait_ids':[w['wait_id'] for w in waits]},'worker_may_continue':decision=='continue','editor_may_start':True,'frontier':s['current_cut'],'open_tools':list(s['open_tools']),'interleaves':dict(s['interleaves']),'budget':budget,'per_wb_budget':{root:s.get('wb_budgets',{}).get(root,1) for root in roots},'grant_id':grant_id}
        self.store.save(s)
        if receipt_key:self.store.remember(receipt_key,fingerprint,result)
        return result
    def _consume_grant(self,s,grant_id):
        grant=s.get('continuation_grants',{}).get(grant_id);require(grant is not None and not grant['used'] and grant['actor']==s['_actor'] and grant['cut_seq']==s['cut_seq'] and not s['open_tools'],'invalid, stale, consumed, or unsafe continuation grant')
        grant['used']=True;s['events'].append({'type':'worker_continue','grant_id':grant_id,'time':self.now()})
    def _usage(self,s,p):
        categories=accounting.validate_categories(p.get('categories',[]))
        record=copy.deepcopy(p);record['categories']=categories;record.setdefault('physical_cache_evidence','unknown');call_id=record['call_id']
        for usage_id in s['usage']:
            existing=self.store.get(usage_id,'usage')
            if existing['call_id']==call_id:require(existing==record,'call_id usage conflict');return {'usage_id':usage_id,'recorded':True}
        usage_id=self.store.put('usage',record);s['usage'].append(usage_id);self.store.save(s);return {'usage_id':usage_id,'recorded':True}
    def _normalized_usage(self,s,result,role,call_id):
        raw=result.get('usage');normalized=accounting.normalize_usage(raw,result.get('usage_format','openai-chat'))
        cached=((raw or {}).get('prompt_tokens_details') or {}).get('cached_tokens')
        return self._usage(s,{'call_id':call_id,'role':role,'chain_id':s['chain_id'],'model':result.get('requested_model'),'returned_model':result.get('returned_model'),'route':result.get('route'),**normalized,'source_refs':result.get('sic_context',{}),'admission':result.get('admission'),'physical_cache_evidence':{'cached_tokens':cached} if cached is not None else 'unknown','call_duration_seconds':result.get('call_duration_seconds'),'started_at':result.get('started_at'),'ended_at':result.get('ended_at')})

    def _configured_provider(self,state,role,editor=None):
        profile_name=state['config'].get('worker_profile') if role=='worker' else state['config'].get('editor_profiles',{}).get(editor)
        if not profile_name:return self.provider,role
        profile=self.config.get('profiles',{})[profile_name]
        return LiteLLMProvider({role:profile}),role
    def _editor_request(self,job,editor):
        assignment=next(a for a in job['assignments'] if a['editor']==editor)
        units=[{k:u[k] for k in ('unit_id','text')} for u in job['source_snapshot']['units'] if u['unit_id'] in assignment['unit_ids']]
        provenance=prefix_layout.provenance(self,job,assignment)
        editor_background=job.get('editor_backgrounds',{}).get(editor,{'cbs':job['background'],'manifest':job.get('background_manifest')})
        request={'messages':[{'role':'system','content':job['prompt']},{'role':'user','content':json.dumps({'task':job.get('task',{}),'background_manifest':editor_background['manifest'],'readonly_archive_reads':job.get('readonly_reads',[]),'read_only_cb':editor_background['cbs'],'read_only_instant':job.get('editor_instants',{}).get(editor,job['instant_reference']),'editable_text_units':units,'readonly_provenance':provenance,'readonly_protocol_messages':prefix_layout.protocol_skeleton(self,job),'response_schema':{'ops':[],'pvi':{'P':[],'V':[],'I':{}}}},ensure_ascii=False)}]}
        if job.get('readonly_media'):
            payload=json.loads(request['messages'][-1]['content']);payload['readonly_media']=copy.deepcopy(job['readonly_media']);request['messages'][-1]['content']=json.dumps(payload,ensure_ascii=False)
        return request

    def _model(self,op,context,p):
        chain=context['chain_id'];actor=context['actor'];call_id=uid('call')
        with self.store.transaction():
            s=self.store.state(chain);require(actor in s['owners'],'chain access denied','forbidden',403);s['_actor']=actor
            require(not any(self.store.revoked(x) for group in s['groups'].values() for x in group),'revoked source dependency','revoked_dependency',403)
            if op=='edit.run':
                job=self._job(s,p);editor=p['editor'];assignment=next((x for x in job['assignments'] if x['editor']==editor),None)
                require(assignment is not None,'Editor not assigned');require(editor not in job['attempts'] and editor not in job['responses'],'single-round invocation already consumed')
                # No raw request, roles, tool IDs, headers, or native control fields sent as editable targets.
                editor_background=job.get('editor_backgrounds',{}).get(editor,{'cbs':job['background'],'manifest':job.get('background_manifest')})
                request,layout_meta=prefix_layout.editor_request(self,s,job,editor,self._editor_request(job,editor))
                job['attempts'][editor]={'call_id':call_id,'status':'running','started_at':self.now()};s['events'].append({'type':'editor_dispatch','job_id':p['job_id'],'editor':editor,'call_id':call_id,'time':self.now()});binding=job.get('model_bindings',{}).get(editor);provider,role=(LiteLLMProvider({'editor':binding}),'editor') if binding else (self.provider,'editor')
            else:
                key=p['idempotency_key'];fingerprint=digest({'chain':chain,'payload':p});receipt_key=chain+':provider:'+key;existing=self.store.receipt(receipt_key,fingerprint)
                if existing:return existing
                require(not s.get('provider_inflight'),'provider call already in flight')
                if p.get('grant_id'):self._consume_grant(s,p['grant_id'])
                else:
                    decision=self._schedule(s,{k:v for k,v in p.items() if k!='idempotency_key'});require(decision['worker_may_continue'],'Worker blocked by scheduler: '+decision['decision'],'worker_wait',409);self._consume_grant(s,decision['grant_id'])
                request,layout_meta=prefix_layout.worker_request(self,s,actor,encode(self._working_view(s,'worker'),'openai-chat'));frontier=self._frontier(s,request);request_id=self.store.put('provider_request',{'chain_id':chain,'role':'worker','request':request,'cache_frontier':frontier,'cut_id':s['current_cut'],'prefix_layout':layout_meta});s['last_provider_request']=request_id;s['provider_inflight']={'request_id':request_id,'call_id':call_id,'base_cut':s['cut_seq'],'receipt_key':receipt_key,'fingerprint':fingerprint};provider,role=self._configured_provider(s,'worker')
            if op=='edit.run':
                request_id=self.store.put('provider_request',{'chain_id':chain,'role':role,'request':request,'job_id':p['job_id'],'background_manifest':editor_background['manifest'],'cut_id':s['current_cut'],'prefix_layout':layout_meta});job['attempts'][editor]['request_id']=request_id
            layout_meta['provider_request_id']=request_id
            if op=='edit.run':job['attempts'][editor]['prefix_layout']=copy.deepcopy(layout_meta)
            s['last_prefix_layout']=copy.deepcopy(layout_meta);self.store.save(s)
        try:
            call_started=time.perf_counter();started_at=self.now();result=provider.call(role,request);result['call_duration_seconds']=time.perf_counter()-call_started;result['started_at']=started_at;result['ended_at']=self.now();result['sic_context']={'chain_id':chain,'request_id':request_id,'job_id':p.get('job_id'),'editor':p.get('editor'),'prefix_layout':copy.deepcopy(layout_meta)}
            with self.store.transaction():
                s=self.store.state(chain);s['_actor']=actor;response_id=self.store.put('provider_response',result)
                try:self._normalized_usage(s,result,role,call_id)
                except (CoreError,ValueError,TypeError):self._usage(s,{'call_id':call_id,'role':role,'model':result.get('requested_model'),'categories':[],'usage_status':'unknown','raw_usage':result.get('usage'),'invalid_usage_report':True})
                self.store.save(s)
            with self.store.transaction():
                s=self.store.state(chain);s['_actor']=actor
                require(not any(self.store.revoked(x) for group in s['groups'].values() for x in group),'revoked source dependency','revoked_dependency',403)
                require(result.get('returned_model') in result.get('expected_returned_models',[result.get('requested_model')]),'provider model identity mismatch or unknown; response/usage retained, not adopted','model_identity',422)
                if op=='edit.run':
                    job=self._job(s,p);choice=result['response']['choices'][0]
                    if choice['message'].get('tool_calls') and layout_meta.get('requested_layout')=='shared_i':
                        require(False,'Editor content-JSON mode has no activated executable tools','forbidden',403)
                    require(choice.get('finish_reason')=='stop','Editor response is truncated, interrupted, or not a completed single response');text=choice['message']['content'];parsed=json.loads(text);require(isinstance(parsed,dict) and not set(parsed)-{'ops','pvi'},'Editor response envelope invalid');raw_delivery=copy.deepcopy(parsed);parsed=prefix_layout.adopt_editor(job,editor,parsed,layout_meta)
                    validate_response(assignment['unit_ids'],parsed.get('ops',[]),parsed.get('pvi',{}),primary_decision=job.get('level',1)==1 and job.get('context_id','main')=='main');job['responses'][editor]={'ops':parsed.get('ops',[]),'pvi':parsed.get('pvi',{}),'origin':'model','response_id':response_id}
                    if layout_meta.get('applied_layout')=='shared_i':
                        canonical_ref=self.store.put('editor_canonical_response',{'chain_id':chain,'job_id':job['job_id'],'editor':editor,'raw_response_id':response_id,'alias_map':job['editor_alias_maps'][editor],'raw_delivery':raw_delivery,'ops':parsed.get('ops',[]),'pvi':parsed.get('pvi',{}),'prefix_layout':layout_meta});job['responses'][editor].update(canonical_response_id=canonical_ref,alias_map_id=layout_meta['alias_map_id'])
                    job['attempts'][editor]['status']='completed';job['attempts'][editor]['ended_at']=self.now();s['editor_timing']={'ewma_seconds':result['call_duration_seconds']};s['events'].append({'type':'editor_complete','job_id':p['job_id'],'editor':editor,'call_id':call_id,'time':self.now()});job['status']='failed' if any(a['status']=='failed' for a in job['attempts'].values()) else ('ready' if len(job['responses'])==len(job['assignments']) else 'open');self.store.save(s);return {'job_id':p['job_id'],'status':job['status'],'response_id':response_id,'call_id':call_id,'_provider_called':True}
                inflight=s['provider_inflight'];require(inflight['base_cut']==s['cut_seq'],'provider returned for superseded cut; response retained, not adopted')
                message=result['response']['choices'][0]['message']
                if layout_meta.get('applied_layout')=='shared_i':
                    for call in message.get('tool_calls') or []:
                        name=call['function']['name'];prefix_layout.authorize(self,s,actor,name,meta=layout_meta);s.setdefault('shared_tool_call_modes',{})[call['id']]={'name':name,'capability_id':layout_meta['capability_id'],'request_id':request_id}
                decoded=decode('openai-chat',{'messages':[message]});source_id=self.store.put('source',decoded);s['pending'].append(source_id);s['groups'][source_id]=[source_id];s['provider_inflight']=None;s['events'].append({'type':'worker_response','source_id':source_id,'basis_cut':inflight['base_cut'],'call_id':call_id,'time':self.now()})
                output={'call_id':call_id,'response_id':response_id,'response':result['response'],'source_id':source_id,'request_digest':digest(request),'request_id':inflight['request_id'],'cache_frontier':self.store.get(inflight['request_id'],'provider_request')['cache_frontier'],'cut_id':s['current_cut']};self.store.save(s);self.store.remember(receipt_key,fingerprint,output);output['_provider_called']=True;return output
        except Exception as error:
            with self.store.transaction():
                s=self.store.state(chain)
                if op=='edit.run':
                    failed_job=s['jobs'][p['job_id']];failed_job['attempts'][p['editor']]['status']='failed'
                    if failed_job['status'] not in ('published','abandoned','superseded'):failed_job['status']='failed'
                else:
                    # Indeterminate/failure is durable and requires a new explicit key; no silent duplicate request.
                    inflight=s.pop('provider_inflight',None)
                    if inflight:
                        failure={'call_id':call_id,'status':'failed-or-indeterminate','automatic_retry':False}
                        self.store.remember(inflight['receipt_key'],inflight['fingerprint'],failure)
                self.store.put('call_failure',{'call_id':call_id,'role':role,'error_type':type(error).__name__})
                if not any(self.store.get(x,'usage')['call_id']==call_id for x in s['usage']):self._usage(s,{'call_id':call_id,'role':role,'model':'configured-role','call_duration_seconds':time.perf_counter()-call_started if 'call_started' in locals() else None,'categories':[],'usage_status':'unknown','physical_cache_evidence':'unknown','attempt_status':'failed-or-indeterminate'})
                self.store.save(s)
            if isinstance(error,CoreError):raise
            raise CoreError('model_call_failed','Model call or response validation failed; no automatic retry. Inspect job/call status.',422) from error

    def _frontier(self,s,request):
        from .store import dumps
        encoded=dumps(request).encode();previous=None
        if s.get('last_provider_request'):previous=self.store.get(s['last_provider_request'],'provider_request')['request']
        old=dumps(previous).encode() if previous else b'';common=0
        for a,b in zip(old,encoded):
            if a!=b:break
            common+=1
        controls={k:v for k,v in request.items() if k!='messages'}
        previous_controls={k:v for k,v in (previous or {}).items() if k!='messages'}
        # A top-level tool/schema change can precede messages in provider cache layout.
        if previous and controls!=previous_controls:common=0
        return {'stable_cb_count':len(s['cbs']),'cut_id':s['current_cut'],'protocol_controls_digest':digest(controls),'canonical_request_bytes':len(encoded),'prior_canonical_prefix_bytes':common,'reencoded_suffix_bytes':len(encoded)-common,'measurement':'canonical-UTF8-bytes-not-provider-tokens','physical_cache_hit':'unknown','provider_layout_verified':False}

    def _board(self,s,actor,write=False,board_id=None):
        board_id=board_id or s['board_id'];row=self.store.db.execute('SELECT state FROM boards WHERE id=?',(board_id,)).fetchone();require(row is not None,'board not found')
        board=json.loads(row[0]);require(actor in board['write_acl' if write else 'read_acl'],'shared board permission denied','forbidden',403);return {'board_id':board_id,**board}

    def close(self):
        if self._closed:return
        self._closed=True
        if getattr(self,'host_tools',None) is not None:self.host_tools.close()
        if self._runtime is not None:self._runtime.stop()
        self.store.db.close()
    @property
    def runtime(self):
        if self._runtime is None:
            from .runtime import Runtime
            self._runtime=Runtime(self)
        return self._runtime
    def _working_view(self,s,role):
        view=self._view(s);view['pending']=[self._source(s,root) for root in communication.pending_sources(self,s,role)];projection=memory.project(self,s,role,int(s['config'].get('editor_memory_chars' if role=='editor' else 'memory_chars',16384)),s['config'].get('task_goal',''));view['cbs']=projection['cbs'];view['media_carriers']=media.project_carriers(self,view['cbs']);view['instant']={**copy.deepcopy(hierarchy.selected_instant(s,role)),'lazy_tools':view['instant'].get('lazy_tools',[])};view['instant']['hierarchy_active']=copy.deepcopy(hierarchy.ensure(s)['active']);nav={}
        if s['current_cut'] and communication.cut_provenance(self,self.store.get(s['current_cut'],'cut'),role):nav['communication_page']=s['current_cut']
        cut_groups=self.store.get(s['current_cut'],'cut').get('groups',{}) if s['current_cut'] else {}
        covered={sid for group in cut_groups.values() for sid in group}
        initial=[sid for source in view['pending'] for sid in source['source_ids'] if sid not in covered and communication.source_provenance(self.store.get(sid,'source'))]
        if initial:nav['communication_source_pages']=list(dict.fromkeys(initial))
        limit=int(s['config'].get('memory_chars',16384))//2
        view['instant']=memory.bounded_instant(view['instant'],limit-len(json.dumps(nav,ensure_ascii=False)),s['current_cut'],{'role':role});view['instant'].update(nav);
        view['instant']['memory_projection']=projection['manifest'];return view

    @staticmethod
    def _tool_operations():return {'sic_hierarchy_legacy':'hierarchy.legacy','sic_memory_replay':'memory.replay','sic_hierarchy_expand':'hierarchy.expand','sic_hierarchy_timeline':'hierarchy.timeline','sic_memory_page':'memory.page','sic_memory_timeline':'memory.timeline','sic_memory_locate':'memory.locate','sic_decision_update':'decision.update','sic_socket_dispatch':'collaboration.dispatch','sic_socket_signal':'socket.signal','sic_agent_routes':'chain.list','sic_collaboration_status':'collaboration.status'}
    def _extra_tools(self,actor):
        definitions=[('sic_hierarchy_legacy','Page retained legacy CBs with unknown original WB formation chronology; expand and read saved sources',{'offset':{'type':'integer'},'limit':{'type':'integer'}},[]),('sic_memory_replay','Replay content after a prefix of immutable CB operations against its frozen WB; no tools or external actions run',{'cb_id':{'type':'string'},'step':{'type':'integer'}},['cb_id']),('sic_hierarchy_expand','Expand a CB to its same-level WB and ordered lower-level CBs; use memory pages for complete original text',{'cb_id':{'type':'string'},'wb_id':{'type':'string'}},[]),('sic_hierarchy_timeline','Navigate independent WB/CB formation chains at any level',{'level':{'type':'integer'},'context_id':{'type':'string'},'kind':{'type':'string'},'offset':{'type':'integer'},'limit':{'type':'integer'}},[]),('sic_memory_page','Read bounded source/CB/operation pages; communication_page and communication_source_pages in I name immutable communication provenance objects (kind=communication); use next_offset until complete',{'kind':{'type':'string'},'object_id':{'type':'string'},'offset':{'type':'integer'},'limit':{'type':'integer'},'level':{'type':'integer'},'context_id':{'type':'string'},'role':{'type':'string'}},['kind','object_id']),('sic_memory_timeline','Navigate chain time order and original WB links',{'offset':{'type':'integer'},'limit':{'type':'integer'}},[]),('sic_memory_locate','Resolve CB/source relationships',{'source_id':{'type':'string'},'cb_id':{'type':'string'}},[]),('sic_decision_update','Maintain current Working on, Failed Before, Not Tried Yet and Overlooked views',{'expected_revision':{'type':'integer'},'events':{'type':'array','items':{'type':'object'}}},['expected_revision','events']),('sic_socket_dispatch','Ask authorized historical peer branches to explore and return sourced results',{'socket_ids':{'type':'array','items':{'type':'string'}},'question':{'type':'string'},'request_id':{'type':'string'},'lifecycle':{'type':'string'}},['socket_ids','question','request_id']),('sic_socket_signal','Host-authorized run/wait/wake/stop of a peer',{'socket_id':{'type':'string'},'signal':{'type':'string'}},['socket_id','signal']),('sic_agent_routes','Discover authorized routes',{},[]),('sic_collaboration_status','Read peer request/result state',{},[])]
        return [{'name':name,'descriptor':description,'schema':{'type':'function','function':{'name':name,'description':description,'parameters':{'type':'object','properties':properties,'required':required}}},'discover_acl':[actor],'expand_acl':[actor],'execute_acl':[actor],'expanded':True} for name,description,properties,required in definitions]

    @staticmethod
    def _account_events(s):
        starts={};intervals=[]
        for event in s['events']:
            if event.get('duplicate_observation'):continue
            if event.get('type')=='tool_start':starts[event['call_id']]=event['time']
            elif event.get('type')=='tool_end' and event['call_id'] in starts:intervals.append({'kind':'tool','start':starts.pop(event['call_id']),'end':event['time'],'call_id':event['call_id']})
            elif event.get('kind') in ('serial_wait','interleave') and 'start' in event and 'end' in event:intervals.append(event)
        return intervals

    def _proposal_refs(self,job):
        responses=[]
        for editor,response in job['responses'].items():
            response_id=response.get('response_id')
            if response_id:responses.append({'editor':editor,'object_id':response_id,'kind':'provider_response' if response.get('origin')=='model' else 'editor_response',**{key:response[key] for key in ('canonical_response_id','alias_map_id') if key in response}})
        revisions=[];revision_id=job.get('candidate',{}).get('revision_id')
        while revision_id:
            revisions.append(revision_id);revision_id=self.store.get(revision_id,'candidate_revision').get('previous_revision_id')
        return {'original_responses':responses,'candidate_revisions':list(reversed(revisions)),'read_tool':'sic_memory_page','kind':'proposal','prefix_layout':{editor:{**{k:copy.deepcopy(v) for k,v in job.get('attempts',{}).get(editor,{}).get('prefix_layout',meta).items() if k not in ('snapshot_payload','suffix')},'basis':'actual-request' if job.get('attempts',{}).get(editor,{}).get('request_id') else 'prepared-only'} for editor,meta in job.get('prefix_layout',{}).items()}}
    def _bound_tool_result(self,state,name,value,budget):
        text=json.dumps(value,ensure_ascii=False,allow_nan=False)
        if len(text)<=budget:return value
        object_id=self.store.put('tool_return',{'chain_id':state['chain_id'],'name':name,'value':value});state.setdefault('tool_pages',[]).append(object_id);self.store.save(state)
        return {'deferred_result_id':object_id,'total_chars':len(text),'complete':False,'read_tool':'sic_memory_page','kind':'tool_result','object_id':object_id,'offset':0,'limit':max(64,budget//4),'reason':'explicit bounded model-tool return; complete result retained'}

    def now(self):
        """Epoch-aligned monotonic clock shared by calls and host execution events."""
        return self._clock_wall+(time.perf_counter()-self._clock_mono)
