"""Recursive material, two chronologies, role adoption and historical isolation."""
import concurrent.futures,copy,json,sqlite3,tempfile,unittest
from pathlib import Path
from unittest.mock import patch
from sic_core import Core,CoreError


class HierarchyTests(unittest.TestCase):
    def setUp(self):
        self.core=Core(config={'revocation_actors':['host']});self.context={'actor':'host','allow_write':True,'allow_model':True};self.context['chain_id']=self.op('chain.create')['chain_id']
    def tearDown(self):self.core.close()
    def op(self,name,p=None,context=None):return self.core.operate(name,context or self.context,p or {})['result']
    def publish(self,sid,ops=None,pvi=None):
        job=self.op('edit.program' if ops is None else 'edit.prepare',{'source_id':sid,'rules':[],'pvi':pvi or {}})['job_id']
        if ops is not None:self.op('edit.submit',{'job_id':job,'editor':'editor-1','ops':ops,'pvi':pvi or {}})
        state=self.op('chain.get');return self.op('cut.publish',{'job_id':job,'expected_cut':state['cut_seq'],'idempotency_key':job})
    def raw(self,text):return self.op('source.ingest',{'request':{'messages':[{'role':'user','content':text}]}})['source_id']
    def upper(self,number,members,ops=None,pvi=None,context_id='main'):
        formed=self.op('hierarchy.form',{'context_id':context_id,'level':number,'cb_ids':members});return formed,self.publish(formed['source_id'],ops,pvi)
    def base(self,count):return [self.publish(self.raw('original detail '+str(i)))['cb_id'] for i in range(count)]

    def test_independent_two_chains_correspondence_and_full_recursive_content(self):
        one=self.base(4);before={cid:self.core.store.get(cid,'cb') for cid in one}
        f2,a=self.upper(2,one[:2],[{'op':'COMPRESS','target':'u1','content':'first abstraction'},{'op':'DELETE','target':'u2'}]);_,b=self.upper(2,one[2:])
        f3,c=self.upper(3,[a['cb_id'],b['cb_id']],[{'op':'REPLACE','target':'u1','content':'third level representation'}])
        for n,total in [(1,4),(2,2),(3,1)]:
            wb=self.op('hierarchy.timeline',{'level':n,'kind':'wb'});cb=self.op('hierarchy.timeline',{'level':n,'kind':'cb'})
            self.assertEqual((wb['total'],cb['total']),(total,total));self.assertEqual([r['cb_id'] for r in wb['rows']],[r['id'] for r in cb['rows']]);self.assertEqual([r['id'] for r in wb['rows']],[r['wb_id'] for r in cb['rows']])
        expanded=self.op('hierarchy.expand',{'cb_id':c['cb_id']});self.assertEqual([m['cb_id'] for m in expanded['members']],[a['cb_id'],b['cb_id']]);self.assertEqual(expanded['original_units'][0]['text'],self.core.store.get(a['cb_id'],'cb')['body'])
        expanded=self.op('hierarchy.expand',expanded['members'][0]['expand']);self.assertEqual([m['cb_id'] for m in expanded['members']],one[:2]);leaf=self.op('hierarchy.expand',expanded['members'][1]['expand'])
        self.assertEqual(leaf['original_units'][0]['text'],'original detail 1')
        self.assertEqual(before,{cid:self.core.store.get(cid,'cb') for cid in one})
        _,d=self.upper(4,[c['cb_id']]);self.assertEqual(d['level'],4)
        with self.assertRaises(sqlite3.IntegrityError):self.core.store.db.execute('UPDATE objects SET data=? WHERE id=?',('{}',a['cb_id']))

    def test_scope_order_and_route_guards(self):
        one=self.base(3)
        for members in ([one[1],one[0]],[one[0],one[2]],[one[0],one[0]]):
            with self.assertRaises(CoreError):self.op('hierarchy.form',{'level':2,'cb_ids':members})
        formed=self.op('hierarchy.form',{'level':2,'cb_ids':one[:2]});job=self.op('edit.prepare',{'source_id':formed['source_id'],'assignments':[{'editor':'left','unit_ids':['u1']},{'editor':'right','unit_ids':['u2']}]})['job_id']
        with self.assertRaises(CoreError):self.op('edit.submit',{'job_id':job,'editor':'left','ops':[{'op':'DELETE','target':'u2'}]})
        with self.assertRaises(CoreError):self.op('edit.submit',{'job_id':job,'editor':'left','ops':[{'op':'REPLACE','target':'u1','content':'bad','role':'system'}]})
        with self.assertRaises(CoreError):self.op('hierarchy.form',{'level':2,'cb_ids':one[:2]})
        other=self.op('chain.create')['chain_id'];ctx={**self.context,'chain_id':other}
        for name,p in [('hierarchy.expand',{'cb_id':one[0]}),('memory.page',{'kind':'cb','object_id':one[0]})]:
            with self.assertRaises(CoreError):self.op(name,p,ctx)

    def test_higher_pvi_candidate_original_and_fixed_ops(self):
        one=self.base(2);f=self.op('hierarchy.form',{'level':2,'cb_ids':one});job=self.op('edit.prepare',{'source_id':f['source_id']})['job_id']
        self.op('edit.submit',{'job_id':job,'editor':'editor-1','ops':[{'op':'COMPRESS','target':'u1','content':'proposed'}],'pvi':{'P':['old proposal']}})
        self.op('edit.candidate.update',{'job_id':job,'expected_revision':0,'ops':[{'op':'REPLACE','target':'u1','content':'reviewed'}],'pvi':{'P':['fixed upper evidence'],'V':['same level next work'],'I':{'status':'upper current'}}})
        p={'job_id':job,'expected_cut':self.op('chain.get')['cut_seq'],'expected_candidate_revision':1,'idempotency_key':'higher-pvi'};out=self.op('cut.publish',p);before=list(self.core.store.db.iterdump());self.assertEqual(self.op('cut.publish',p),out);self.assertEqual(before,list(self.core.store.db.iterdump()))
        cb=self.core.store.get(out['cb_id'],'cb');self.assertIn('fixed upper evidence',cb['body']);self.assertNotIn('same level next work',cb['body']);self.assertEqual(len(cb['proposal_refs']['original_responses']),1);self.assertEqual(len(cb['proposal_refs']['candidate_revisions']),1)
        h=self.op('hierarchy.get');self.assertIn(out['volatile_source_id'],h['levels']['2']['pending']);self.assertNotIn(out['volatile_source_id'],h['levels']['1']['pending']);self.assertEqual(h['levels']['2']['instant']['status'],'upper current');self.assertNotIn('status',h['levels']['1']['instant'])
        self.publish(out['volatile_source_id'],pvi={'I':{'status':'upper next'}});self.assertEqual(self.core.store.get(out['cb_id'],'cb'),cb);self.assertEqual(self.op('hierarchy.get')['levels']['2']['instant']['status'],'upper next')

    def test_atomic_higher_publication_stale_and_retry(self):
        one=self.base(2);formed=self.op('hierarchy.form',{'level':2,'cb_ids':one});job=self.op('edit.program',{'source_id':formed['source_id'],'rules':[],'pvi':{'P':['P'],'V':['V'],'I':{'current':'I'}}})['job_id'];p={'job_id':job,'expected_cut':self.op('chain.get')['cut_seq'],'idempotency_key':'atomic-upper'}
        snapshot=list(self.core.store.db.iterdump());original=self.core.store.save
        def fail(state):original(state);raise RuntimeError('injected after persistent save')
        with patch.object(self.core.store,'save',fail):
            with self.assertRaises(RuntimeError):self.op('cut.publish',p)
        self.assertEqual(snapshot,list(self.core.store.db.iterdump()));out=self.op('cut.publish',p);self.assertEqual(out['level'],2)
        stale_source=self.raw('next lower block');stale=self.op('edit.program',{'source_id':stale_source})['job_id'];self.op('hierarchy.select',{'level':2})
        with self.assertRaises(CoreError):self.op('cut.publish',{'job_id':stale,'expected_cut':self.op('chain.get')['cut_seq'],'idempotency_key':'stale'})

    def test_historical_fork_restores_full_layers_and_role_selection(self):
        one=self.base(2);_,upper=self.upper(2,one);self.op('hierarchy.select',{'level':2});checkpoint=self.op('chain.checkpoint');frozen=self.op('hierarchy.get');self.op('hierarchy.select',{'level':1});self.base(1)
        branch=self.op('chain.fork',{'checkpoint_id':checkpoint['checkpoint_id']});ctx={**self.context,'chain_id':branch['chain_id']};restored=self.op('hierarchy.get',context=ctx)
        self.assertEqual(restored['levels'],frozen['levels']);self.assertEqual(restored['role_views'],frozen['role_views']);self.assertNotEqual(restored['chain_id'],frozen['chain_id'])
        selected=self.op('memory.project',{'role':'worker'},ctx)['manifest']['selected_cb_ids'];self.assertEqual(selected,[upper['cb_id']])
        self.assertEqual(self.op('hierarchy.expand',{'cb_id':upper['cb_id']},ctx)['members'][0]['cb_id'],one[0])

    def test_recursive_dependency_revocation_blocks_upper_and_old_branch(self):
        sid=self.raw('withdrawn secret');one=self.publish(sid)['cb_id'];_,two=self.upper(2,[one]);_,three=self.upper(3,[two['cb_id']]);checkpoint=self.op('chain.checkpoint');fork=self.op('chain.fork',{'checkpoint_id':checkpoint['checkpoint_id']})
        self.op('security.revoke',{'source_ids':[sid],'confirm_revoke':True})
        for obj in (one,two['cb_id'],three['cb_id']):
            with self.assertRaises(ValueError):self.core.store.get(obj,'cb')
        for name,p in [('hierarchy.expand',{'cb_id':three['cb_id']}),('memory.page',{'kind':'cb','object_id':three['cb_id']}),('provider.preview',{})]:
            with self.assertRaises(CoreError):self.op(name,p)
            with self.assertRaises(CoreError):self.op(name,p,{**self.context,'chain_id':fork['chain_id']})

    def test_sustained_maintenance_reaches_new_levels_and_continues(self):
        self.base(8);result=self.op('hierarchy.maintain',{'fanout':2});self.assertTrue(any(x['level']==4 for x in result['actions']));self.assertEqual(len(self.op('chain.list')['routes']),1)
        before=self.op('hierarchy.get');old=list(before['levels']['4']['cbs']);self.base(8);self.op('hierarchy.maintain',{'fanout':2});after=self.op('hierarchy.get');self.assertEqual(after['levels']['4']['cbs'][:1],old);self.assertEqual(len(after['levels']['4']['cbs']),2);self.assertEqual(len(after['levels']['5']['cbs']),1)
        selected=self.op('memory.project',{'role':'worker'})['manifest']['selected_cb_ids'];self.assertEqual(selected,after['levels']['5']['cbs'])

    def test_internal_editor_context_recurses_and_individual_editor_adopts_shared_references(self):
        main=self.base(2);_,shared=self.upper(2,main);self.op('hierarchy.select',{'level':2,'roles':['worker','editor']})
        self.op('hierarchy.context.create',{'context_id':'editor-notes','shared_contexts':['main']})
        notes=[]
        for i in range(4):
            source=self.op('hierarchy.context.ingest',{'context_id':'editor-notes','request':{'messages':[{'role':'user','content':'private persistent note '+str(i)}]}})['source_id'];notes.append(self.publish(source)['cb_id'])
        _,a=self.upper(2,notes[:2],context_id='editor-notes');_,b=self.upper(2,notes[2:],context_id='editor-notes');_,top=self.upper(3,[a['cb_id'],b['cb_id']],context_id='editor-notes')
        self.op('hierarchy.select',{'context_id':'editor-notes','level':3,'roles':['editor:alpha']})
        sid=self.raw('new common task');prepared=self.op('edit.prepare',{'source_id':sid,'assignments':[{'editor':'alpha','unit_ids':['u1']},{'editor':'beta','unit_ids':[]}]})['job']
        alpha=prepared['editor_backgrounds']['alpha']['manifest']['selected_cb_ids'];beta=prepared['editor_backgrounds']['beta']['manifest']['selected_cb_ids'];self.assertIn(top['cb_id'],alpha);self.assertIn(shared['cb_id'],alpha);self.assertNotIn(top['cb_id'],beta)
        self.assertEqual(self.op('memory.project',{'role':'worker'})['manifest']['selected_cb_ids'],[shared['cb_id']]);self.assertEqual(len(self.op('chain.list')['routes']),1)

    def test_disk_restart_retains_hierarchy_pending_candidate_and_selection(self):
        with tempfile.TemporaryDirectory() as directory:
            path=str(Path(directory)/'core.sqlite');self.core.close();self.core=Core(path);self.context.pop('chain_id');self.context['chain_id']=self.op('chain.create')['chain_id'];one=self.base(2);_,two=self.upper(2,one);self.op('hierarchy.select',{'level':2});formed=self.op('hierarchy.form',{'level':3,'cb_ids':[two['cb_id']]});job=self.op('edit.program',{'source_id':formed['source_id']})['job_id'];before=self.op('hierarchy.get');self.core.close();self.core=Core(path)
            self.assertEqual(self.op('hierarchy.get'),before);out=self.op('cut.publish',{'job_id':job,'expected_cut':self.op('chain.get')['cut_seq'],'idempotency_key':'after-restart'});self.assertEqual(out['level'],3);self.core.close();self.core=Core()

    def test_large_higher_instant_pages_recover_selected_level_and_historical_snapshot(self):
        self.op('config.update',{'memory_chars':1024,'editor_memory_chars':1024});one=self.base(1);bulk={str(i):'higher-only-'+str(i) for i in range(150)};_,upper=self.upper(2,one,pvi={'I':bulk});selection=self.op('hierarchy.select',{'level':2});state=self.op('chain.get');state['_actor']='host';view=self.core._working_view(state,'worker');manifest=view['instant']['instant_projection'];self.assertEqual(manifest['role'],'worker');self.assertEqual(manifest['object_id'],selection['cut_id'])
        offset=0;parts=[]
        while True:
            page=self.op('memory.page',{'kind':'view','object_id':manifest['object_id'],'role':'worker','offset':offset,'limit':500});parts.append(page['value'])
            if page['next_offset'] is None:break
            offset=page['next_offset']
        restored=json.loads(''.join(parts));self.assertEqual({k:restored[k] for k in bulk},bulk)
        self.op('hierarchy.select',{'level':1});page=self.op('memory.page',{'kind':'view','object_id':selection['cut_id'],'context_id':'main','level':2,'limit':8192});self.assertEqual({k:json.loads(page['value'])[k] for k in bulk},bulk)

    def test_concurrent_upper_candidates_publish_exactly_one_same_level_pair(self):
        one=self.base(2);formed=self.op('hierarchy.form',{'level':2,'cb_ids':one});jobs=[self.op('edit.program',{'source_id':formed['source_id'],'pvi':{'P':['candidate '+str(i)]}})['job_id'] for i in range(2)];sequence=self.op('chain.get')['cut_seq']
        def publish(job):
            try:return self.op('cut.publish',{'job_id':job,'expected_cut':sequence,'idempotency_key':job})
            except CoreError as error:return error.code
        with concurrent.futures.ThreadPoolExecutor(max_workers=2) as executor:results=list(executor.map(publish,jobs))
        self.assertEqual(sum(isinstance(r,dict) for r in results),1);row=self.op('hierarchy.get')['levels']['2'];self.assertEqual(len(row['cbs']),1);self.assertEqual(row['pairs'],{formed['wb_id']:row['cbs'][0]});self.assertEqual(row['pending'],[])

    def test_multi_source_wb_archive_has_complete_ordered_pages(self):
        first=self.raw('first source');second=self.raw('second source');formed=self.op('wb.form',{'source_ids':[first,second]});published=self.publish(formed['source_id']);expanded=self.op('hierarchy.expand',{'cb_id':published['cb_id']});self.assertEqual([u['text'] for u in expanded['original_units']],['first source','second source']);self.assertEqual([x['object_id'] for x in expanded['complete_reads']],[first,second]);self.assertEqual(''.join(self.op('memory.page',r)['value'] for r in expanded['complete_reads']),'first sourcesecond source')

    def test_higher_public_replay_recovers_each_step_and_does_not_replay_pvi_views(self):
        one=self.base(2);formed,out=self.upper(2,one,[{'op':'REPLACE','target':'u1','content':'changed'},{'op':'DELETE','target':'u2'},{'op':'ADD','content':'new'}],{'P':['persistent'],'V':['next upper work'],'I':{'only-view':'current'}});cb=self.core.store.get(out['cb_id'],'cb');snapshot=list(self.core.store.db.iterdump());zero=self.op('memory.replay',{'cb_id':out['cb_id'],'step':0});self.assertEqual(zero['body'],'original detail 0\noriginal detail 1');self.assertIsNone(zero['body_matches_published_cb']);self.assertEqual(self.op('memory.replay',{'cb_id':out['cb_id'],'step':1})['body'],'changed\noriginal detail 1');last=self.op('memory.replay',{'cb_id':out['cb_id']});self.assertEqual(last['body'],cb['body']);self.assertTrue(last['body_matches_published_cb']);self.assertFalse(last['cut_instant_reconstructed']);self.assertEqual(last['instant'],{});self.assertEqual(list(self.core.store.db.iterdump()),snapshot)
        for step in (-1,len(cb['ops'])+1):
            with self.assertRaises(CoreError):self.op('memory.replay',{'cb_id':out['cb_id'],'step':step})
        other=self.op('chain.create')['chain_id']
        with self.assertRaises(CoreError):self.op('memory.replay',{'cb_id':out['cb_id']},context={**self.context,'chain_id':other})
