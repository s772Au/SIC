"""Reserved primary decision view is checked before adoption, without CAS mutation."""
import copy,json,unittest
from sic_core import Core,CoreError
from sic_core import decision

class InvalidEditor:
    def __init__(self):self.calls=0
    def capabilities(self):return {'test_double':True}
    def call(self,role,request):
        self.calls+=1
        return {'requested_model':'owned','returned_model':'owned','response':{'model':'owned','choices':[{'finish_reason':'stop','message':{'role':'assistant','content':json.dumps({'ops':[],'pvi':{'I':{'decision_tree':'invalid flat narrative'}}})}}]},'usage':{'prompt_tokens':1,'completion_tokens':1,'total_tokens':2}}

class DecisionValidationTests(unittest.TestCase):
    def setUp(self):
        self.provider=InvalidEditor();self.core=Core(provider=self.provider);self.chain=self.core.operate('chain.create',{'actor':'host','allow_write':True},{})['result']['chain_id']
    def tearDown(self):self.core.close()
    def op(self,operation,payload=None,model=False):
        return self.core.operate(operation,{'actor':'host','chain_id':self.chain,'allow_write':True,'allow_model':model},payload or {})['result']
    def prepare(self):
        source=self.op('source.ingest',{'request':{'messages':[{'role':'user','content':'editable owned material'}]}})['source_id'];return self.op('edit.prepare',{'source_id':source})['job_id']
    def state(self):return self.op('chain.get')
    def tree(self):return {'views':{'Working on':[{'text':'retained task','evidence':[]}],'Failed Before':[],'Not Tried Yet':[],'Overlooked':[]}}
    def publish(self,job):return self.op('cut.publish',{'job_id':job,'expected_cut':self.state()['cut_seq'],'idempotency_key':'publish-'+job})
    def test_submit_rejects_invalid_reserved_shape_atomically(self):
        job=self.prepare();before=self.state()
        for raw in ['narrative',[],{'views':[]},{'unknown':[]},{'views':{'Working on':[{'text':1}]}}]:
            with self.subTest(raw=raw):
                with self.assertRaises(CoreError):self.op('edit.submit',{'job_id':job,'editor':'editor-1','ops':[],'pvi':{'I':{'decision_tree':raw}}})
                self.assertEqual(self.state(),before)
        self.assertEqual(self.provider.calls,0)
    def test_model_invalid_response_is_retained_but_never_ready_or_adopted(self):
        job=self.prepare()
        with self.assertRaises(CoreError):self.op('edit.run',{'job_id':job,'editor':'editor-1'},model=True)
        s=self.state();self.assertEqual(s['jobs'][job]['status'],'failed');self.assertFalse(s['jobs'][job]['responses']);self.assertFalse(s['cbs']);self.assertEqual(self.provider.calls,1)
        with self.core.store.transaction():
            rows=self.core.store.db.execute("select data from objects where kind='provider_response'").fetchall()
        self.assertEqual(len(rows),1);self.assertIn('invalid flat narrative',rows[0][0]);self.assertEqual(len(self.op('ledger.export')['calls']),1)
    def test_program_and_candidate_replace_reject_same_reserved_shape(self):
        source=self.op('source.ingest',{'request':{'messages':[{'role':'user','content':'input'}]}})['source_id'];before=self.state()
        with self.assertRaises(CoreError):self.op('edit.program',{'source_id':source,'pvi':{'I':{'decision_tree':'invalid'}}})
        self.assertEqual(self.state(),before)
        job=self.op('edit.program',{'source_id':source})['job_id'];before=self.state()
        with self.assertRaises(CoreError):self.op('edit.candidate.replace',{'job_id':job,'expected_revision':0,'ops':[],'pvi':{'I':{'decision_tree':'invalid'}}})
        self.assertEqual(self.state(),before)
    def test_validate_rejects_legacy_ready_invalid_shape_without_mutating_state(self):
        job=self.prepare()
        # Simulate an already persisted candidate from the pre-fix build.
        with self.core.store.transaction():
            s=self.core.store.state(self.chain);s['jobs'][job]['responses']={'editor-1':{'ops':[],'pvi':{'I':{'decision_tree':'old invalid record'}},'origin':'submitted-editor'}};s['jobs'][job]['status']='ready';self.core.store.save(s)
        before=self.state()
        with self.assertRaises(CoreError) as rejected:self.op('edit.validate',{'job_id':job})
        self.assertIn('decision_tree must be a four-view object',str(rejected.exception));self.assertEqual(self.state(),before)
    def test_valid_pvi_preview_is_pure_and_publish_keeps_dynamic_revision_cas(self):
        job=self.prepare();current=self.op('decision.get');self.op('edit.submit',{'job_id':job,'editor':'editor-1','ops':[],'pvi':{'I':{'decision_tree':self.tree()}}});preview=self.op('edit.validate',{'job_id':job})
        self.assertEqual(preview['material']['instant']['decision_tree'],self.tree());self.assertEqual(self.op('decision.get'),current)
        self.op('decision.update',{'expected_revision':current['revision'],'events':[{'kind':'work','text':'new concurrent fact'}]});before=self.state()
        with self.assertRaises(CoreError) as rejected:self.publish(job)
        self.assertIn('decision view changed since Editor snapshot',str(rejected.exception));self.assertEqual(self.state(),before)
    def test_primary_i_op_keeps_string_contract_and_materializes_explicit_json_object(self):
        job=self.prepare();before=self.state();tree=self.tree()
        for content in ['not JSON','"still a string"','[]','{"Working on": [], "Working on": []}','{"Working on":[{"text":"x","evidence":[NaN]}]}']:
            with self.assertRaises(CoreError):self.op('edit.submit',{'job_id':job,'editor':'editor-1','ops':[{'op':'ADD','lifecycle':'I','view_key':'decision_tree','content':content}]})
            self.assertEqual(self.state(),before)
        encoded=json.dumps(tree);op={'op':'ADD','lifecycle':'I','view_key':'decision_tree','content':encoded};self.op('edit.submit',{'job_id':job,'editor':'editor-1','ops':[op]});preview=self.op('edit.validate',{'job_id':job})
        self.assertEqual(preview['material']['instant']['decision_tree'],tree);self.assertEqual(preview['material']['ops'][0]['content'],encoded)
        published=self.publish(job);self.assertEqual(self.op('decision.get')['views']['Working on'][0]['text'],'retained task')
        self.assertEqual(self.op('memory.replay',{'cb_id':published['cb_id']})['instant']['decision_tree'],tree)
    def test_higher_level_reserved_name_remains_general_text(self):
        job=self.prepare();self.op('edit.submit',{'job_id':job,'editor':'editor-1','ops':[]});published=self.publish(job)
        source=self.op('hierarchy.form',{'level':2,'cb_ids':[published['cb_id']]})['source_id'];job=self.op('edit.prepare',{'source_id':source})['job_id'];before=self.op('decision.get')
        self.op('edit.submit',{'job_id':job,'editor':'editor-1','ops':[],'pvi':{'I':{'decision_tree':'higher-level descriptive text'}}});self.publish(job)
        self.assertEqual(self.state()['hierarchy']['levels']['2']['instant']['decision_tree'],'higher-level descriptive text');self.assertEqual(self.op('decision.get'),before)
    def test_private_context_keeps_general_text_instant_semantics(self):
        self.op('hierarchy.context.create',{'context_id':'notes'});source=self.op('hierarchy.context.ingest',{'context_id':'notes','request':{'messages':[{'role':'user','content':'private work'}]}})['source_id'];job=self.op('edit.prepare',{'source_id':source})['job_id']
        self.op('edit.submit',{'job_id':job,'editor':'editor-1','ops':[{'op':'ADD','lifecycle':'I','view_key':'decision_tree','content':'private free text'}],'pvi':{'I':{'ordinary':'ordinary text'}}});self.publish(job)
        s=self.state();self.assertEqual(s['hierarchy']['contexts']['notes']['levels']['1']['instant']['decision_tree'],'private free text');self.assertEqual(self.op('decision.get'),decision.empty())

if __name__=='__main__':unittest.main()
