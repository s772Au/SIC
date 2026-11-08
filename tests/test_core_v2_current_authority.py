import copy,unittest
from sic_core import Core,CoreError

class CurrentAuthorityTests(unittest.TestCase):
    def setUp(self):
        self.core=Core();self.actor='host';self.chain=self.op(None,'chain.create',{})['chain_id']
        self.tool={'name':'owned_tool','descriptor':'owned fixture','schema':{'type':'function','function':{'name':'owned_tool','parameters':{'type':'object','properties':{}}}},'discover_acl':['host'],'expand_acl':['host'],'execute_acl':['host'],'expanded':True}
        self.configure(self.chain,True)
        source=self.op(self.chain,'source.ingest',{'request':{'messages':[{'role':'user','content':'owned material'}]}})['source_id']
        job=self.op(self.chain,'edit.program',{'source_id':source})['job_id']
        self.op(self.chain,'cut.publish',{'job_id':job,'expected_cut':0,'idempotency_key':'first'})
        self.checkpoint=self.op(self.chain,'chain.checkpoint',{})['checkpoint_id']
    def tearDown(self):self.core.close()
    def op(self,chain,name,p):return self.core.operate(name,{'actor':self.actor,'chain_id':chain,'allow_write':True,'allow_model':False},p)['result']
    def configure(self,chain,allowed):
        tool=copy.deepcopy(self.tool)
        if not allowed:
            for layer in ('discover','expand','execute'):tool[layer+'_acl']=[]
        self.op(chain,'tools.configure',{'tools':[tool]})
    def fork(self,chain,checkpoint):return self.op(chain,'chain.fork',{'checkpoint_id':checkpoint})['chain_id']
    def denied(self,chain):
        for operation,p in [('tools.authorize',{'name':self.tool['name']}),('tools.invoke',{'name':self.tool['name'],'arguments':{}})]:
            with self.assertRaises(CoreError) as caught:self.op(chain,operation,p)
            self.assertEqual(caught.exception.code,'forbidden')
        self.assertEqual(self.op(chain,'tools.resolve',{})['tools'],[])
        with self.core.store.transaction():
            state=self.core.store.state(chain);state['_actor']=self.actor
            self.assertEqual(self.core._working_view(state,'worker')['tool_schemas'],[])
    def test_revocation_before_historical_fork(self):
        self.configure(self.chain,False);child=self.fork(self.chain,self.checkpoint);self.denied(child)
    def test_later_revocation_reaches_existing_child_and_grandchild(self):
        child=self.fork(self.chain,self.checkpoint)
        checkpoint=self.op(child,'chain.checkpoint',{})['checkpoint_id'];grandchild=self.fork(child,checkpoint)
        self.assertTrue(self.op(grandchild,'tools.authorize',{'name':self.tool['name']})['authorized'])
        self.configure(self.chain,False)
        self.denied(child);self.denied(grandchild)
    def test_blank_created_agent_has_independent_current_authority(self):
        blank=self.op(None,'chain.create',{'creators':[self.chain]})['chain_id'];self.configure(blank,True)
        self.configure(self.chain,False)
        self.assertTrue(self.op(blank,'tools.authorize',{'name':self.tool['name']})['authorized'])
    def test_frontier_declares_message_positions_and_checks_length_both_directions(self):
        source=self.op(self.chain,'source.ingest',{'request':{'messages':[{'role':'user','content':'x'*500}]}})['source_id']
        job=self.op(self.chain,'edit.prepare',{'source_id':source})['job_id']
        self.op(self.chain,'edit.submit',{'job_id':job,'editor':'editor-1','ops':[{'op':'REPLACE','target':'u1','content':'short'}]})
        comparison=self.op(self.chain,'frontier.compare',{'job_id':job,'expected_cut':1,'length_budget':0,'position_budget':0})
        self.assertLess(comparison['length_delta_bytes'],0)
        self.assertFalse(comparison['within_local_window'])
        self.assertEqual(comparison['position_delta_messages'],0)
        self.assertEqual(comparison['position_budget_unit'],'canonical-message-index')
        accepted=self.op(self.chain,'frontier.compare',{'job_id':job,'expected_cut':1,'length_budget':4096,'position_budget':0})
        self.assertTrue(accepted['within_local_window'])
    def test_scheduler_wait_is_pre_admission_without_provider_attempt_or_receipt(self):
        class NeverCalled:
            calls=0
            def call(self,*args):self.calls+=1;raise AssertionError('No provider admission expected')
        provider=NeverCalled();self.core.provider=provider
        source=self.op(self.chain,'source.ingest',{'request':{'messages':[{'role':'user','content':'new pending owned block'}]}})['source_id']
        job=self.op(self.chain,'edit.prepare',{'source_id':source})['job_id']
        for completed in (False,True):
            if completed:self.op(self.chain,'edit.submit',{'job_id':job,'editor':'editor-1','ops':[]})
            with self.assertRaises(CoreError) as caught:
                self.core.operate('provider.continue',{'actor':self.actor,'chain_id':self.chain,'allow_write':True,'allow_model':True},{'idempotency_key':'refused-admission','policy':'wait'})
            self.assertEqual(caught.exception.code,'worker_wait');self.assertEqual(caught.exception.status,409)
            self.assertEqual(provider.calls,0)
            with self.core.store.transaction():
                state=self.core.store.state(self.chain)
                self.assertIsNone(state.get('provider_inflight'));self.assertEqual(state['usage'],[])
                self.assertEqual(self.core.store.db.execute("SELECT count(*) FROM objects WHERE kind='provider_request'").fetchone()[0],0)
                self.assertEqual(self.core.store.db.execute("SELECT count(*) FROM receipts WHERE key LIKE '%:provider:%'").fetchone()[0],0)
                self.assertEqual(self.core.store.db.execute("SELECT count(*) FROM objects WHERE kind='call_failure'").fetchone()[0],0)

if __name__=='__main__':unittest.main()
