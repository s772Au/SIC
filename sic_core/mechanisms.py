"""Mechanism operations kept separate from HTTP and legacy service code."""
import copy,json,time,math
from concurrent.futures import ThreadPoolExecutor
from .adapters import digest,encode
from .editing import validate_response,materialize,replay
from .store import uid
from . import decision,memory,accounting,hierarchy
UNHANDLED=object()

class Mechanisms:
    def _candidate(self,job):
        if job.get('candidate'):return copy.deepcopy(job['candidate'])
        ops=[];pvi={'P':[],'V':[],'I':{}}
        for assignment in job['assignments']:
            response=job['responses'].get(assignment['editor'])
            if response is None:continue
            ops.extend(copy.deepcopy(response['ops']));pvi['P'].extend(response['pvi'].get('P',[]));pvi['V'].extend(response['pvi'].get('V',[]))
            for key,value in response['pvi'].get('I',{}).items():
                if key in pvi['I']:raise ValueError('Editors conflict on same instantaneous key')
                pvi['I'][key]=copy.deepcopy(value)
        return {'revision':0,'ops':ops,'pvi':pvi,'origin':'original-responses','revision_id':None}
    def _features(self,op,s,actor,p):
        from .service import require,CoreError
        if op=='memory.replay':
            cid=p['cb_id'];require(cid in hierarchy.all_cbs(s),'CB not on authorized route','forbidden',403);cb=self.store.get(cid,'cb');frozen=copy.copy(s);frozen['groups']={**s['groups'],cb['source_id']:list(cb['source_ids'])};source=self._source(frozen,cb['source_id']);step=p.get('step',len(cb['ops']));result=replay(source,cb['ops'],step,primary_decision=cb.get('level',1)==1 and cb.get('context_id','main')=='main');complete=step==len(cb['ops']);matches=result['body']==cb['body'] if complete else None
            require(not complete or matches,'Published CB body differs from replayed immutable source and operations','replay_integrity',409)
            return {**result,'chain_id':s['chain_id'],'context_id':cb.get('context_id','main'),'level':cb.get('level'),'hierarchy_index_status':'indexed' if 'level' in cb else 'legacy-unindexed','wb_formation_order':'recorded' if 'level' in cb else 'unknown','cb_id':cid,'wb_id':cb['wb_id'],'source_ids':list(cb['source_ids']),'body_matches_published_cb':matches,'cut_instant_reconstructed':False,'pvi_only_views_reconstructed':False}
        if op=='resource.record':
            record=copy.deepcopy(p);require(record.get('record_id') and record.get('resource') and record.get('measurement_method'),'resource identity and measurement method required');accounting.summarize_records([], [record],required_resources=())
            for rid in s.get('resources',[]):
                old=self.store.get(rid,'resource')
                if old['record_id']==record['record_id']:require(old==record,'resource record id conflict');return {'resource_id':rid}
            rid=self.store.put('resource',record);s.setdefault('resources',[]).append(rid);self.store.save(s);return {'resource_id':rid}
        if op=='ledger.export':
            require(p.get('scope','chain') in ('chain','task'),'ledger scope must be chain or task')
            if p.get('scope')=='task':
                from .task_ledger import export_task
                return export_task(self,s,actor,p)
            require(not any(k in p for k in ('additional_chain_ids','prices_by_chain')),'Cross-route inputs require scope=task')
            calls=[self.store.get(x,'usage') for x in s['usage']];resources=[self.store.get(x,'resource') for x in s.get('resources',[])];return {'schema':'sic-ledger-export-v1','chain_id':s['chain_id'],'scope':{'kind':'chain','included_chain_ids':[s['chain_id']],'completeness':'this route only; use scope=task to include collaboration branches'},'calls':calls,'resource_records':resources,'events':copy.deepcopy(s['events']),'summary':accounting.summarize_records(calls,resources,p.get('task_interval'),p.get('prices',s['config'].get('prices',[])),self._account_events(s))}
        if op=='chain.list':
            rows=[json.loads(row[0]) for row in self.store.db.execute('SELECT state FROM chains')];return {'routes':[{'chain_id':row['chain_id'],'run_state':row['run_state'],'cut_id':row['current_cut'],'origin_checkpoint':row['origin_checkpoint'],'creators':row['creators']} for row in rows if actor in row['owners']]}
        if op.startswith('edit.candidate.'):
            job=self._job(s,p);candidate=self._candidate(job)
            if op=='edit.candidate.get':return {'job_id':job['job_id'],**candidate,'ready':len(job['responses'])==len(job['assignments']),'original_responses':copy.deepcopy(job['responses']),'source_ids':job['source_snapshot']['source_ids']}
            require(op=='edit.candidate.update','unknown candidate operation');require(len(job['responses'])==len(job['assignments']),'Editor responses not complete');require(p['expected_revision']==candidate['revision'],'candidate revision conflict')
            scope=[u for a in job['assignments'] for u in a['unit_ids']];primary_decision=job.get('level',1)==1 and job.get('context_id','main')=='main';validate_response(scope,p['ops'],p.get('pvi',{}),primary_decision=primary_decision);materialize(job['source_snapshot'],[{'editor':'manual:'+actor,'unit_ids':scope}],{'manual:'+actor:{'ops':p['ops'],'pvi':p.get('pvi',{})}},primary_decision=primary_decision)
            candidate={'revision':candidate['revision']+1,'ops':copy.deepcopy(p['ops']),'pvi':copy.deepcopy(p.get('pvi',{})),'origin':'manual:'+actor,'previous_revision_id':candidate.get('revision_id'),'original_response_ids':job.get('response_ids',[])+[r['response_id'] for r in job['responses'].values() if r.get('response_id')]}
            revision_id=self.store.put('candidate_revision',{'job_id':job['job_id'],'actor':actor,'time':time.time(),**candidate});candidate['revision_id']=revision_id;job['candidate']=candidate;self.store.save(s);return {'job_id':job['job_id'],**candidate}
        if op=='wb.form':
            roots=p.get('source_ids') or list(s['pending']);require(bool(roots) and roots==s['pending'][:len(roots)],'WB sources must be a nonempty consecutive pending prefix')
            active={j['source_id'] for j in s['jobs'].values() if j['status'] not in ('published','superseded','abandoned')};active.update(self.store.get(wid,'wb')['source_id'] for row in hierarchy.ensure(s)['levels'].values() for wid in row['wbs']);require(not active.intersection(roots),'cannot change frozen job membership')
            root=roots[0];parts=[part for r in roots for part in s['groups'][r]];s['groups'][root]=parts
            for old in roots[1:]:s['pending'].remove(old);s['groups'].pop(old)
            source=self._source(s,root);wb_id=hierarchy.register_wb(self,s,root,1,reason=p.get('reason','explicit-boundary'));s.setdefault('wb_versions',{})[root]=wb_id
            event={'type':'wb_formed','wb_id':wb_id,'source_id':root,'parts':parts,'reason':p.get('reason','explicit-boundary'),'time':time.time(),'event_id':uid('event')};s['events'].append(event);self.store.save(s);return {'wb_id':wb_id,'source_id':root,'source_ids':parts,'event_id':event['event_id'],'units':source['units']}
        if op=='decision.get':return copy.deepcopy(s.get('decision',decision.empty()))
        if op in ('decision.update','decision.maintain'):
            current=s.get('decision',decision.empty());events=p.get('events',[])
            if op=='decision.update':require(p['expected_revision']==current['revision'],'decision revision conflict')
            else:
                events=[];seen=set(current['event_ids'])
                for event in s['events']:
                    event_id=event.get('event_id')
                    if event_id in seen:continue
                    if event['type']=='tool_end' and event.get('status')=='failed':events.append({'id':event_id,'kind':'failure','text':event.get('error','Tool '+event['call_id']+' failed'),'evidence':[{'event_id':event_id,'call_id':event['call_id']}]})
                for root in s['pending']:
                    for source_id in s['groups'][root]:
                        source=self.store.get(source_id,'source')
                        for unit in source['units']:
                            message=source['sidecar']['messages'][unit['anchor']['message']]
                            if message['role']=='user':
                                identity='user-material:'+source_id+':'+unit['unit_id']
                                if identity not in seen:events.append({'id':identity,'kind':'work','text':'User input to process: '+unit['text'][:512],'evidence':[{'source_id':source_id,'unit_id':unit['unit_id'],'excerpt_chars':min(512,len(unit['text']))}]})
                if s['config'].get('task_goal') and not current['views']['Working on']:events.append({'id':'task-goal:'+digest(s['config']['task_goal']),'kind':'work','text':s['config']['task_goal'],'evidence':[{'configuration':'task_goal'}]})
            value=decision.apply(current,events);record_id=self.store.put('decision_revision',{'chain_id':s['chain_id'],'actor':actor,'events':events,'value':value});s['decision']=value;s['instant_draft']['decision_tree']=value;s['events'].append({'type':'decision_updated','revision_id':record_id,'revision':value['revision'],'time':time.time()});self.store.save(s);return {**value,'revision_id':record_id}
        if op=='memory.timeline':
            offset=int(p.get('offset',0));limit=int(p.get('limit',20));require(offset>=0 and 1<=limit<=100,'timeline page bounds invalid');ids=s['cbs'];rows=[]
            for index in range(offset,min(offset+limit,len(ids))):
                cb=self.store.get(ids[index],'cb');rows.append({'chain_id':s['chain_id'],'membership_index':index,'cb_id':ids[index],'formed_on_chain':cb['chain_id'],'original_sequence':cb['sequence'],'predecessor':ids[index-1] if index else None,'successor':ids[index+1] if index+1<len(ids) else None,'wb_id':cb.get('wb_id'),'source_ids':cb['source_ids'],'excerpt':cb['body'][:160]})
            return {'rows':rows,'total':len(ids),'offset':offset,'next_offset':offset+len(rows) if offset+len(rows)<len(ids) else None}
        if op=='memory.locate':
            cb_id=p.get('cb_id');source_id=p.get('source_id');require(bool(cb_id)^bool(source_id),'specify one cb_id or source_id');rows=[]
            for index,cid in enumerate(hierarchy.all_cbs(s)):
                cb=self.store.get(cid,'cb')
                if cid==cb_id or source_id in cb['source_ids']:rows.append({'chain_id':s['chain_id'],'context_id':cb.get('context_id','main'),'level':cb.get('level'),'hierarchy_index_status':'indexed' if 'level' in cb else 'legacy-unindexed','wb_formation_order':'recorded' if 'level' in cb else 'unknown','cb_id':cid,'wb_id':cb.get('wb_id'),'source_ids':cb['source_ids'],'membership_index':hierarchy.level(s,cb.get('level',1),cb.get('context_id','main'))['cbs'].index(cid),'operation_count':len(cb['ops']),'unit_index':cb['index']})
            require(bool(rows) or source_id in {x for g in s['groups'].values() for x in g},'object not on chain','forbidden',403);return {'rows':rows,'pending':source_id in {x for root in s['pending'] for x in s['groups'][root]} if source_id else False}
        if op=='memory.page':return memory.page(self,s,p)
        if op=='memory.project':return memory.project(self,s,p.get('role','worker'),int(p.get('max_chars',s['config'].get('memory_chars',16384))),p.get('query',''))
        if op=='frontier.compare':
            from .frontier import compare
            return compare(self,s,actor,p)
        if op=='board.claim':
            board=self._board(s,actor,True,p.get('board_id'));require(p['expected_revision']==board['revision'],'board revision conflict');value=copy.deepcopy(board['value']);require(isinstance(value,dict),'claim requires object board');tasks=value.setdefault('tasks',{});task=tasks.get(p['task_id']);require(task is None or task.get('claimant')==s['chain_id'],'task already claimed')
            tasks[p['task_id']]={'description':p.get('description',task.get('description','') if task else ''),'claimant':s['chain_id'],'status':p.get('status','claimed')};return self._dispatch('board.update',s,actor,{'board_id':board['board_id'],'expected_revision':board['revision'],'value':value})
        if op=='runtime.recover':
            require(p.get('confirm') is True,'Explicit recovery confirmation required','forbidden',403);require(not self.runtime._lock(s['chain_id']).locked(),'Live runtime work must finish or pause before recovery','runtime_busy',409);action=p['action']
            if action=='abandon_editor':
                job=self._job(s,p);job['status']='abandoned'
                for attempt in job['attempts'].values():
                    if attempt['status']=='running':attempt['status']='indeterminate'
            elif action=='mark_worker_indeterminate':
                inflight=s.get('provider_inflight');require(inflight is not None,'No Worker attempt to recover');require(p.get('acknowledge_possible_duplicate') is True,'Explicit indeterminate outcome acknowledgement required','forbidden',403)
                existing=self.store.receipt(inflight['receipt_key'],inflight['fingerprint'])
                if existing is None:self.store.remember(inflight['receipt_key'],inflight['fingerprint'],{'call_id':inflight['call_id'],'status':'host-marked-indeterminate','automatic_retry':False})
                if not any(self.store.get(x,'usage')['call_id']==inflight['call_id'] for x in s['usage']):self._usage(s,{'call_id':inflight['call_id'],'role':'worker','model':'unknown-inflight','categories':[],'usage_status':'unknown','source_refs':{'request_id':inflight['request_id']}})
                s['provider_inflight']=None
            elif action=='cancel_collaboration':
                request=s.setdefault('collaborations',{}).get(p['request_id']);require(request is not None,'request not found');request['status']='cancelled'
                for branch in request['branches']:
                    if branch['status']!='completed':branch['status']='cancelled';child=self.store.state(branch['chain_id']);child['run_state']='stopped';self.store.save(child)
            elif action!='resume_queue':raise ValueError('unknown recovery action')
            s.setdefault('runtime',{})['enabled']=False;self.store.db.execute("UPDATE runtime_queue SET status='host-recovered' WHERE chain_id=? AND status IN ('running','failed','pending','recovery-required')",(s['chain_id'],));record=self.store.put('runtime_recovery',{'chain_id':s['chain_id'],'actor':actor,'action':action,'time':time.time()});self.store.save(s);return {'recovery_id':record,'automatic_model_call':False,'runtime_enabled':False,'next':'explicit runtime.run or reconfigure standing grant'}
        if op=='runtime.configure':
            allowed={'enabled','editor_mode','worker_mode','editor_workers','seal_chars','max_steps','worker_turns','editor_chars','worker_chars','branch_budget','max_pending_seconds'};require(not set(p)-allowed,'unknown runtime policy');policy=s.setdefault('runtime',{});policy.update(copy.deepcopy(p));require(policy.get('editor_mode','program') in ('program','model'),'invalid editor mode');require(policy.get('worker_mode','host') in ('host','automatic'),'invalid worker mode');require(all(isinstance(policy[k],int) and policy[k]>0 for k in ('editor_workers','seal_chars','max_steps','editor_chars','worker_chars','worker_turns','branch_budget') if k in policy),'positive runtime limits required');age=policy.setdefault('max_pending_seconds',1.0);require(isinstance(age,(int,float)) and not isinstance(age,bool) and math.isfinite(age) and 0<age<=86400,'max_pending_seconds must be finite and in (0,86400]');self.store.save(s);return {'policy':policy}
        if op=='runtime.get':return {'policy':s.get('runtime',{}),'events':s['events'],'pending':s['pending'],'run_state':s['run_state'],'last_step':s.get('runtime_last_step')}
        if op=='collaboration.status':return {'requests':s.get('collaborations',{})}
        if op=='collaboration.dispatch':
            return_lifecycle=p.get('return_lifecycle','V');require(return_lifecycle in ('P','V'),'return_lifecycle must be P or V')
            request_id=p['request_id'];fingerprint=digest(p);existing=s.setdefault('collaborations',{}).get(request_id)
            if existing:require(existing['fingerprint']==fingerprint,'collaboration request id conflict');return copy.deepcopy(existing)
            require(isinstance(p['question'],str) and bool(p['question']),'question required');require(bool(p['socket_ids']),'at least one socket required');require(len(p['socket_ids'])<=s.get('runtime',{}).get('branch_budget',4),'collaboration branch budget exceeded');branches=[]
            for socket_id in p['socket_ids']:
                branch=self._dispatch('socket.spawn',s,actor,{'socket_id':socket_id,'text':p['question'],'lifecycle':p.get('lifecycle','V'),'message_id':request_id+':'+socket_id});child=self.store.state(branch['chain_id']);child['runtime']={**s.get('runtime',{}),'worker_mode':'host'};child['runtime_model_authorized']=s.get('runtime_model_authorized',False);child['collaboration_task']={'sender':s['chain_id'],'request_id':request_id,'return_lifecycle':return_lifecycle,'question':p['question']};self.store.save(child);branches.append({'chain_id':branch['chain_id'],'socket_id':socket_id,'status':'pending','return_lifecycle':return_lifecycle})
            row={'request_id':request_id,'fingerprint':fingerprint,'branches':branches,'status':'pending','question':p['question'],'return_lifecycle':return_lifecycle};s['collaborations'][request_id]=row;self.store.save(s);return row
        return UNHANDLED
    def _run_all(self,context,p):
        from .service import require
        with self.store.transaction():
            s=self.store.state(context['chain_id']);require(context['actor'] in s['owners'],'chain permission denied','forbidden',403);job=self._job(s,p);editors=[a['editor'] for a in job['assignments'] if a['editor'] not in job['responses']];workers=min(len(editors),int(p.get('max_workers',4)))
        if not editors:return {'job_id':p['job_id'],'results':[],'already_complete':True}
        def run(editor):return self.operate('edit.run',context,{'job_id':p['job_id'],'editor':editor})['result']
        with ThreadPoolExecutor(max_workers=max(1,workers)) as executor:results=list(executor.map(run,editors))
        return {'job_id':p['job_id'],'results':results,'parallel_workers':workers,'_provider_called':bool(results)}
