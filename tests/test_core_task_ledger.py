"""Task accounting fixtures use owned records, not model quality results."""
import concurrent.futures,copy,threading,time,unittest
from decimal import Decimal
from unittest.mock import patch
from sic_core import Core,CoreError
from sic_core import accounting

class TaskLedgerTests(unittest.TestCase):
    def setUp(self):
        self.core=Core(config={'revocation_actors':['host']});self.root=self.op(None,'chain.create')['chain_id']
        source=self.op(self.root,'source.ingest',{'request':{'messages':[{'role':'user','content':'owned initial material'}]}})['source_id']
        job=self.op(self.root,'edit.program',{'source_id':source})['job_id'];self.op(self.root,'cut.publish',{'job_id':job,'expected_cut':0,'idempotency_key':'base'})
    def tearDown(self):self.core.close()
    def op(self,chain,name,p=None):return self.core.operate(name,{'actor':'host','chain_id':chain,'allow_write':True,'allow_model':False},p or {})['result']
    def peer(self,parent,request):
        cp=self.op(parent,'chain.checkpoint')['checkpoint_id'];socket=self.op(parent,'socket.create',{'checkpoint_id':cp,'spawn_acl':['host']})['socket_id']
        result=self.op(parent,'collaboration.dispatch',{'socket_ids':[socket],'request_id':request,'question':'owned pending question'})
        return result['branches'][0]['chain_id']
    def prices(self,chain,rate):
        self.op(chain,'config.update',{'prices':[{'category':'input_uncached','unit':'token','unit_price':rate,'currency':'TEST','price_version':chain}]})
    def call(self,chain,identity,quantity=10,role='worker',start=0,end=3,**extra):
        record={'call_id':identity,'role':role,'model':'owned-model','route':'owned-route','raw_usage':{'owned_fixture':True},'categories':[{'category':'input_uncached','quantity':quantity,'unit':'token'}],'started_at':start,'ended_at':end,**extra}
        self.op(chain,'usage.record',record);return record
    def resources(self,covered):
        for kind,quantity in [('program',2),('storage',3)]:self.op(self.root,'resource.record',{'record_id':'shared-'+kind,'resource':kind,'quantity':quantity,'unit':'owned-unit','unit_price':'1','currency':'TEST','price_version':'fixture-v1','measurement_method':'owned fixture input','scope_chain_ids':covered})
    def export(self,**p):return self.op(self.root,'ledger.export',{'scope':'task',**p})
    def test_recursive_peer_higher_editor_prices_routes_and_union_time(self):
        child=self.peer(self.root,'first');grandchild=self.peer(child,'nested')
        for chain,rate in [(self.root,'.1'),(child,'.2'),(grandchild,'.3')]:self.prices(chain,rate)
        self.call(self.root,'worker');self.call(child,'higher',20,'editor',1,5,level=3,context_id='editor-private');self.call(grandchild,'nested',4,'worker',6,7)
        self.resources([self.root,child,grandchild]);result=self.export(task_interval=[0,8])
        self.assertEqual(result['scope']['included_chain_ids'],[self.root,child,grandchild]);self.assertEqual(result['summary']['total_cost'],'11.2')
        self.assertEqual(result['summary']['timing']['model_union_seconds'],6);self.assertEqual(result['summary']['timing']['model_cumulative_seconds'],8)
        self.assertEqual(result['summary']['timing']['task_wall_seconds'],8)
        self.assertEqual(result['call_contexts']['higher'][0]['level'],3);self.assertEqual(result['call_contexts']['higher'][0]['route'],'owned-route')
        self.assertEqual(result['prices_by_chain'][child]['prices'][0]['price_version'],child)
        self.assertTrue(result['scope']['unfinished_branches']);self.assertTrue(result['snapshot']['transaction_consistent'])
    def test_additional_route_and_exact_duplicate_call_and_resource_count_once(self):
        extra=self.op(None,'chain.create')['chain_id'];self.prices(self.root,'.1');self.prices(extra,'.1')
        # Same actual record may be referenced from more than one route; the
        # identical frozen rate is essential, even if model names match.
        self.op(extra,'config.update',{'prices':self.op(self.root,'config.get')['configuration']['prices']})
        record=self.call(self.root,'one-actual-call');self.op(extra,'usage.record',record)
        self.resources([self.root,extra])
        with self.core.store.transaction():
            root=self.core.store.state(self.root);other=self.core.store.state(extra);other['resources']=list(root['resources']);self.core.store.save(other)
        result=self.export(additional_chain_ids=[extra]);self.assertEqual(result['summary']['call_count'],1);self.assertEqual(Decimal(result['summary']['total_cost']),Decimal('6'))
        self.assertEqual(set(result['record_locations']['calls']['one-actual-call']),{self.root,extra})
        self.assertEqual(len(result['resource_records']),2)
    def test_duplicate_identity_different_raw_or_rate_is_rejected(self):
        child=self.peer(self.root,'first');self.prices(self.root,'.1');self.prices(child,'.2')
        record=self.call(self.root,'same');self.op(child,'usage.record',record)
        with self.assertRaises(CoreError) as caught:self.export()
        self.assertEqual(caught.exception.code,'ledger_identity_conflict')
    def test_conflicting_resource_identity_and_forged_branch_provenance_fail(self):
        child=self.peer(self.root,'first')
        for chain,quantity in [(self.root,1),(child,2)]:self.op(chain,'resource.record',{'record_id':'same-resource','resource':'program','quantity':quantity,'unit':'second','measurement_method':'owned fixture'})
        with self.assertRaises(CoreError) as caught:self.export()
        self.assertEqual(caught.exception.code,'ledger_identity_conflict')
        with self.core.store.transaction():state=self.core.store.state(child);state['collaboration_task']['sender']='different';self.core.store.save(state)
        with self.assertRaises(CoreError) as caught:self.export()
        self.assertEqual(caught.exception.code,'task_scope_conflict')
    def test_unfinished_failure_and_inflight_usage_remain_unknown(self):
        child=self.peer(self.root,'first');self.prices(self.root,'.1')
        self.call(self.root,'good')
        self.op(child,'usage.record',{'call_id':'failed','role':'editor','categories':[],'usage_status':'unknown','attempt_status':'failed-or-indeterminate'})
        with self.core.store.transaction():
            state=self.core.store.state(child);state['provider_inflight']={'call_id':'still-running','request_id':'fixture-reference'};self.core.store.save(state)
        result=self.export();self.assertEqual(result['summary']['call_count'],2);self.assertIsNone(result['summary']['total_cost']);self.assertFalse(result['summary']['complete_cost'])
        self.assertEqual(result['pending_attempts'][0]['call_id'],'still-running')
        self.assertTrue(any(x.get('call_id')=='failed' and x['reason']=='usage_incomplete' for x in result['summary']['unknown']))
        self.assertTrue(any(x.get('call_id')=='still-running' for x in result['summary']['unknown']))
    def test_unauthorized_peer_fails_whole_export(self):
        child=self.peer(self.root,'first')
        with self.core.store.transaction():state=self.core.store.state(child);state['owners']=['other'];self.core.store.save(state)
        with self.assertRaises(CoreError) as caught:self.export()
        self.assertEqual(caught.exception.code,'forbidden')
    def test_revoked_child_material_cannot_be_recovered_through_accounting(self):
        child=self.peer(self.root,'first');source=self.op(child,'chain.get')['pending'][0]
        self.op(child,'security.revoke',{'source_ids':[source],'confirm_revoke':True})
        with self.assertRaises(CoreError) as caught:self.export()
        self.assertEqual(caught.exception.code,'revoked_dependency')
    def test_multiple_currencies_and_missing_peer_prices_do_not_become_root_price(self):
        child=self.peer(self.root,'first');self.prices(self.root,'.1');self.call(self.root,'root');self.call(child,'peer')
        result=self.export();self.assertIsNone(result['summary']['total_cost'])
        self.assertTrue(any(x.get('call_id')=='peer' and x['reason']=='price_binding_missing' for x in result['summary']['unknown']))
        self.assertEqual(result['priced_calls'][1]['categories'][0].get('unit_price'),None)
        with self.assertRaises(CoreError):self.export(prices=[])
        self.resources([self.root,child]);result=self.export(prices_by_chain={child:[{'category':'input_uncached','unit':'token','unit_price':'2','currency':'OTHER','price_version':'peer-v1'}]})
        self.assertIsNone(result['summary']['total_cost']);self.assertIn('OTHER',result['summary']['totals_by_currency'])
        self.assertTrue(any(x['reason']=='multiple_currencies_no_conversion' for x in result['summary']['unknown']))
    def test_declared_interval_rejects_outside_calls_and_open_tool_remains_timing_unknown(self):
        self.prices(self.root,'.1');self.call(self.root,'time',start=1,end=4)
        with self.assertRaises(CoreError):self.export(task_interval=[0,3])
        self.op(self.root,'harness.event',{'type':'tool_start','call_id':'incomplete-native'})
        result=self.export();self.assertTrue(any(x.get('reason')=='tool_interval_not_complete' for x in result['summary']['timing']['unknown']))
        self.assertEqual(result['summary']['timing']['tool_union_seconds'],0)
    def test_export_snapshot_blocks_writer_and_never_mixes_versions(self):
        self.prices(self.root,'.1');self.call(self.root,'first');entered=threading.Event();release=threading.Event();writer_started=threading.Event()
        original=accounting.summarize_records
        def held(*a,**kw):entered.set();release.wait(2);return original(*a,**kw)
        def write():writer_started.set();self.call(self.root,'second')
        with concurrent.futures.ThreadPoolExecutor(2) as pool,patch('sic_core.task_ledger.accounting.summarize_records',side_effect=held):
            read=pool.submit(self.export);self.assertTrue(entered.wait(1));writer=pool.submit(write);self.assertTrue(writer_started.wait(1));time.sleep(.03);self.assertFalse(writer.done());release.set()
            snapshot=read.result(2);writer.result(2)
        self.assertEqual([x['call_id'] for x in snapshot['calls']],['first']);self.assertEqual(len(self.export()['calls']),2)
    def test_actual_higher_job_and_worker_request_cut_supply_level_evidence(self):
        cb=self.op(self.root,'chain.get')['cbs'][0]
        source=self.op(self.root,'hierarchy.form',{'level':2,'cb_ids':[cb]})['source_id']
        prepared=self.op(self.root,'edit.program',{'source_id':source});job=prepared['job_id']
        self.op(self.root,'cut.publish',{'job_id':job,'expected_cut':1,'idempotency_key':'higher'})
        self.call(self.root,'higher-editor',role='editor',source_refs={'job_id':job,'editor':'editor-1'})
        self.op(self.root,'hierarchy.select',{'level':2,'roles':['worker']})
        class OwnedWorker:
            def call(self,role,request):return {'requested_model':'owned','returned_model':'owned','usage':None,'response':{'choices':[{'finish_reason':'stop','message':{'role':'assistant','content':'owned continuation'}}]}}
        self.core.provider=OwnedWorker()
        self.core.operate('provider.continue',{'actor':'host','chain_id':self.root,'allow_write':True,'allow_model':True},{'idempotency_key':'higher-worker'})
        result=self.export();editor=result['call_contexts']['higher-editor'][0]
        self.assertEqual(editor['level'],2);self.assertEqual(editor['editing_level'],2)
        worker=next(context[0] for context in result['call_contexts'].values() if context[0]['role']=='worker')
        self.assertEqual(worker['level'],2);self.assertEqual(worker['representation_evidence'],'immutable-request-cut')
    def test_cross_route_intersection_is_not_a_same_agent_natural_window(self):
        child=self.peer(self.root,'first');self.call(self.root,'editor-A',role='editor',start=10,end=12)
        with self.core.store.transaction():
            state=self.core.store.state(child);state['events'].extend([{'type':'tool_start','call_id':'tool-B','time':10},{'type':'tool_end','call_id':'tool-B','time':12}]);self.core.store.save(state)
        result=self.export(task_interval=[0,13])
        self.assertEqual(result['summary']['timing']['editor_tool_overlap_seconds'],2)
        self.assertEqual(result['timing_by_chain'][self.root]['editor_tool_overlap_seconds'],0)
        self.assertEqual(result['timing_by_chain'][child]['editor_tool_overlap_seconds'],0)
        self.assertIn('cross routes',result['summary']['timing']['intersection_scope'])

if __name__=='__main__':unittest.main()
