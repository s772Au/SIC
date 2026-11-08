"""F35/F13/F14/F70 owned in-process requests; no network or physical KV claims."""
import copy
import hashlib
import json
import os
from pathlib import Path
import unittest
from sic_core import Core, CoreError


class OwnedFrontierProvider:
    def __init__(self):self.calls=[];self.pvi={}
    def capabilities(self):return {'owned_fixture':True}
    def call(self,role,request):
        self.calls.append({'role':role,'request':copy.deepcopy(request)})
        if role=='editor':
            payload=json.loads(request['messages'][-1]['content'])
            content=json.dumps({'ops':[{'op':'COMPRESS','target':u['unit_id'],'content':'REPRESENTATION-'+str(len(u['text']))} for u in payload['editable_text_units']],'pvi':self.pvi})
            self.pvi={}
        else:content='owned continuation'
        return {'requested_model':'owned-frontier','returned_model':'owned-frontier','response':{'choices':[{'finish_reason':'stop','message':{'role':'assistant','content':content}}]},'usage':{'prompt_tokens':10,'completion_tokens':5,'total_tokens':15}}


class FrontierHierarchyTests(unittest.TestCase):
    evidence=[]
    def setUp(self):
        self.provider=OwnedFrontierProvider();self.core=Core(provider=self.provider)
        self.ctx={'actor':'host','allow_write':True,'allow_model':True}
        self.ctx['chain_id']=self.op('chain.create')['chain_id']
        self.op('config.update',{'memory_chars':65536,'editor_memory_chars':65536})
        self.record={'test':self.id(),'comparisons':[]};self.evidence.append(self.record)
    def tearDown(self):
        self.record['calls']=copy.deepcopy(self.provider.calls)
        self.record['state']=self.op('chain.get')
        self.core.close()
    @classmethod
    def tearDownClass(cls):
        dest=os.environ.get('SIC_FRONTIER_EVIDENCE')
        if dest:
            root=Path(__file__).resolve().parents[1]
            paths=['sic_core/frontier.py','sic_core/mechanisms.py','sic_core/service.py','sic_core/hierarchy.py','sic_core/memory.py','tests/test_core_v2_frontier_hierarchy.py']
            payload={'scope':'owned Core fixture, no external provider, quality not_evaluated, physical cache unknown','source_sha256':{p:hashlib.sha256((root/p).read_bytes()).hexdigest() for p in paths},'runs':cls.evidence}
            target=Path(dest);target.parent.mkdir(parents=True,exist_ok=True);target.write_text(json.dumps(payload,ensure_ascii=False,indent=2),encoding='utf-8')
    def op(self,name,p=None):return self.core.operate(name,self.ctx,p or {})['result']
    def source(self,text,context='main'):
        name='source.ingest' if context=='main' else 'hierarchy.context.ingest'
        return self.op(name,{'context_id':context,'request':{'messages':[{'role':'user','content':text}]}})['source_id']
    def publish(self,sid,pvi=None):
        job=self.op('edit.prepare',{'source_id':sid})['job_id'];self.provider.pvi=pvi or {}
        self.op('edit.run',{'job_id':job,'editor':'editor-1'})
        return self.op('cut.publish',{'job_id':job,'expected_cut':self.op('chain.get')['cut_seq'],'idempotency_key':job})['cb_id']
    def hierarchy(self,context='main'):
        first=self.publish(self.source('ORIGINAL-NEEDLE-'+context+'x'*1200,context),{'I':{'only_first':'FIRST-I-'+context}})
        second=self.publish(self.op('hierarchy.form',{'level':2,'context_id':context,'cb_ids':[first]})['source_id'],{'I':{'only_second':'SECOND-I-'+context}})
        third=self.publish(self.op('hierarchy.form',{'level':3,'context_id':context,'cb_ids':[second]})['source_id'],{'I':{'only_third':'THIRD-I-'+context}})
        return first,second,third
    def candidate(self,text='new editable material',pvi=None,source=None):
        sid=source or self.source(text);job=self.op('edit.prepare',{'source_id':sid})['job_id']
        self.op('edit.submit',{'job_id':job,'editor':'editor-1','ops':[{'op':'REPLACE','target':'u1','content':'edited'}],'pvi':pvi or {}})
        return job
    def compare(self,job,budget=100000):
        result=self.op('frontier.compare',{'job_id':job,'expected_cut':self.op('chain.get')['cut_seq'],'length_budget':budget,'position_budget':0})
        self.record['comparisons'].append(result);return result
    @staticmethod
    def cbs(request):
        rows=[]
        for message in request['messages']:
            try:value=json.loads(message.get('content',''))
            except (ValueError,TypeError):continue
            if isinstance(value,dict) and 'sic_cb' in value:rows.append(value['sic_cb'])
        return rows
    def test_same_cut_high_worker_comparison_matches_actual_adopted_prefix(self):
        one,two,three=self.hierarchy();self.op('hierarchy.select',{'level':3})
        actual=self.op('provider.continue',{'idempotency_key':'high-prefix'})
        # Actual response becomes the next WB and is then compared on the same cut.
        job=self.candidate(source=actual['source_id']);result=self.compare(job)
        self.record['worker_result']=actual
        observed=self.provider.calls[-1]['request']
        self.assertEqual(self.cbs(observed),[three])
        self.assertEqual(self.cbs(result['necessary_request']),[three,'same-pending-slot'])
        self.assertEqual(self.cbs(result['additional_request']),[three,'same-pending-slot'])
        frontier=result['structured_frontier_message_index']
        self.assertEqual(result['necessary_request']['messages'][:frontier],observed['messages'][:frontier])
        self.assertEqual(result['physical_cache_hit'],'unknown')
    def test_high_order_job_keeps_unrelated_primary_pending_and_no_duplicate_low_cb(self):
        one=self.publish(self.source('LOW-ORIGINAL-'+'z'*500))
        unrelated=self.source('UNRELATED-NEXT-WB-DO-NOT-CONSUME')
        high=self.op('hierarchy.form',{'level':2,'cb_ids':[one]})['source_id']
        job=self.candidate(source=high);result=self.compare(job)
        self.assertFalse(result['request_affected_by_candidate'])
        self.assertEqual(self.cbs(result['additional_request']),[one])
        for request in (result['necessary_request'],result['additional_request']):
            self.assertIn('UNRELATED-NEXT-WB-DO-NOT-CONSUME',json.dumps(request))
        self.assertIn(unrelated,self.op('chain.get')['pending'])
    def test_private_editor_third_order_actual_payload_distinct_from_worker(self):
        main=self.hierarchy();self.op('hierarchy.context.create',{'context_id':'private','shared_contexts':[]})
        private=self.hierarchy('private')
        self.op('hierarchy.select',{'level':3,'roles':['worker']})
        self.op('hierarchy.select',{'level':3,'context_id':'private','roles':['editor:alpha']})
        sid=self.source('WHOLE-NEW-WB');job=self.op('edit.prepare',{'source_id':sid,'assignments':[{'editor':'alpha','unit_ids':['u1']}]})['job_id']
        self.op('edit.run',{'job_id':job,'editor':'alpha'})
        payload=json.loads(self.provider.calls[-1]['request']['messages'][-1]['content'])
        self.assertEqual([x['cb_id'] for x in payload['read_only_cb']],[private[2]])
        self.assertEqual([x['text'] for x in payload['editable_text_units']],['WHOLE-NEW-WB'])
        self.assertNotIn('FIRST-I-main',json.dumps(payload['read_only_instant']))
        result=self.compare(job)
        self.assertEqual(self.cbs(result['necessary_request']),[main[2],'same-pending-slot'])
        self.assertNotIn(private[2],self.cbs(result['necessary_request']))
    def test_selected_instant_does_not_mix_unselected_level(self):
        one,two,three=self.hierarchy();self.op('hierarchy.select',{'level':3,'roles':['worker']})
        result=self.op('provider.continue',{'idempotency_key':'instant-clean'})
        request=self.provider.calls[-1]['request'];instant=[json.loads(m['content'])['sic_instant'] for m in request['messages'] if '"sic_instant"' in (m.get('content') or '')][0]
        self.assertEqual(instant['only_third'],'THIRD-I-main')
        self.assertNotIn('only_first',instant)
        self.assertNotIn('only_second',instant)
        cut=self.op('chain.get')['current_cut']
        archived=json.loads(self.op('memory.page',{'kind':'view','object_id':cut,'role':'worker','limit':8192})['value'])
        self.assertEqual(archived['only_third'],'THIRD-I-main')
        self.assertNotIn('only_first',archived)
        self.op('hierarchy.select',{'level':1,'roles':['worker']})
        historical=json.loads(self.op('memory.page',{'kind':'view','object_id':cut,'role':'worker','limit':8192})['value'])
        self.assertEqual(archived,historical)
    def test_editor_comparison_matches_actual_next_private_role_request_and_is_readonly(self):
        main=self.hierarchy();self.op('hierarchy.context.create',{'context_id':'private','shared_contexts':[]})
        private=self.hierarchy('private')
        self.op('hierarchy.select',{'level':3,'context_id':'private','roles':['editor:alpha']})
        high=self.op('hierarchy.form',{'level':4,'context_id':'private','cb_ids':[private[2]]})['source_id']
        initial=self.op('edit.program',{'source_id':high,'pvi':{'V':['PRIVATE-FOURTH-CONTINUATION']}})['job_id']
        first=self.op('cut.publish',{'job_id':initial,'expected_cut':self.op('chain.get')['cut_seq'],'idempotency_key':'first-fourth'})
        self.op('hierarchy.select',{'level':4,'context_id':'private','roles':['editor:alpha']})
        high_job=self.op('edit.prepare',{'source_id':first['volatile_source_id']})['job_id']
        self.op('edit.submit',{'job_id':high_job,'editor':'editor-1','ops':[{'op':'REPLACE','target':'u1','content':'private fourth'}],'pvi':{'I':{'frontier_only':'I-FOURTH'},'V':['PRIVATE-NEXT-WB']}})
        next_sid=self.source('EXACT-NEXT-SCOPE')
        before_state=self.op('chain.get');before_rows=list(self.core.store.db.execute('SELECT id,kind,data,digest FROM objects ORDER BY id'));before_receipts=list(self.core.store.db.execute('SELECT * FROM receipts ORDER BY key'))
        result=self.op('frontier.compare',{'job_id':high_job,'expected_cut':before_state['cut_seq'],'role':'editor:alpha','next_source_id':next_sid,'next_unit_ids':['u1'],'length_budget':100000,'position_budget':0})
        self.record['comparisons'].append(result)
        self.assertEqual(before_state,self.op('chain.get'));self.assertEqual(before_rows,list(self.core.store.db.execute('SELECT id,kind,data,digest FROM objects ORDER BY id')));self.assertEqual(before_receipts,list(self.core.store.db.execute('SELECT * FROM receipts ORDER BY key')))
        predicted=json.loads(result['additional_request']['messages'][-1]['content'])
        self.assertTrue(result['within_local_window'])
        published=self.op('cut.publish',{'job_id':high_job,'expected_cut':before_state['cut_seq'],'idempotency_key':'actual-fourth'})
        prepared=self.op('edit.prepare',{'source_id':next_sid,'assignments':[{'editor':'alpha','unit_ids':['u1']}]})
        self.op('edit.run',{'job_id':prepared['job_id'],'editor':'alpha'})
        actual=json.loads(self.provider.calls[-1]['request']['messages'][-1]['content'])
        self.assertEqual([u['text'] for u in actual['editable_text_units']],['EXACT-NEXT-SCOPE'])
        self.assertEqual([x['body'] for x in actual['read_only_cb']],[x['body'] for x in predicted['read_only_cb']])
        self.assertEqual([x['ops'] for x in actual['read_only_cb']],[x['ops'] for x in predicted['read_only_cb']])
        self.assertEqual(actual['read_only_instant'],predicted['read_only_instant'])
        self.assertEqual(actual['read_only_instant']['frontier_only'],'I-FOURTH')
        self.assertNotIn('I-FOURTH',json.dumps(self.core.store.get(published['cb_id'],'cb')))
        expanded=self.op('hierarchy.expand',{'cb_id':published['cb_id']})
        self.assertNotIn('I-FOURTH',json.dumps(expanded['original_units']))
        level=self.op('hierarchy.timeline',{'context_id':'private','level':4,'kind':'wb'})
        self.assertGreaterEqual(level['total'],2)

    def test_projection_can_move_additional_slot_and_position_budget_is_not_assumed(self):
        self.publish(self.source('previous short stable'))
        self.op('config.update',{'memory_chars':1700})
        # The candidate's long op body can omit its projected CB while KEEP fits.
        source=self.source('short input')
        job=self.op('edit.prepare',{'source_id':source})['job_id']
        self.op('edit.submit',{'job_id':job,'editor':'editor-1','ops':[{'op':'REPLACE','target':'u1','content':'LONG-'*4000}]})
        result=self.compare(job)
        self.assertNotEqual(self.cbs(result['necessary_request']),self.cbs(result['additional_request']))
        self.assertFalse(result['position_within_budget'])
        self.assertFalse(result['within_local_window'])
        self.assertGreater(result['position_delta_messages'],0)
    def test_failed_editor_scope_simulation_rolls_back_every_row(self):
        self.publish(self.source('prior'))
        job=self.candidate();next_sid=self.source('next')
        state=self.op('chain.get');dump=self.core.store.db.serialize()
        with self.assertRaises(CoreError):
            self.op('frontier.compare',{'job_id':job,'expected_cut':state['cut_seq'],'role':'editor:alpha','next_source_id':next_sid,'next_unit_ids':['INVALID-UNIT']})
        self.assertEqual(state,self.op('chain.get'))
        self.assertEqual(dump,self.core.store.db.serialize())

    def test_instant_refresh_never_becomes_high_order_editable_source(self):
        first=self.publish(self.source('first source'),{'P':['P-ONE'],'I':{'status':'OLD-I'}})
        frozen=self.core.store.get(first,'cb')
        second=self.publish(self.source('second source'),{'I':{'status':'NEW-I'},'V':['NEXT-V']})
        self.op('provider.continue',{'idempotency_key':'after-i-refresh'})
        actual=json.dumps(self.provider.calls[-1]['request'])
        self.assertIn('NEW-I',actual);self.assertNotIn('OLD-I',actual);self.assertIn('NEXT-V',actual)
        high=self.op('hierarchy.form',{'level':2,'cb_ids':[first,second]})['source_id']
        job=self.op('edit.prepare',{'source_id':high})['job_id'];self.op('edit.run',{'job_id':job,'editor':'editor-1'})
        payload=json.loads(self.provider.calls[-1]['request']['messages'][-1]['content'])
        self.assertNotIn('NEW-I',json.dumps(payload['editable_text_units']));self.assertNotIn('OLD-I',json.dumps(payload['editable_text_units']))
        self.assertEqual(payload['read_only_instant']['status'],'NEW-I')
        self.assertEqual(frozen,self.core.store.get(first,'cb'))
    def test_stable_digest_hashes_exact_utf8_prefix_bytes(self):
        self.publish(self.source('中文😀 prior'))
        job=self.candidate('中文😁 upcoming')
        result=self.compare(job)
        encoded=json.dumps(result['necessary_request'],ensure_ascii=False,sort_keys=True).encode()
        self.assertEqual(result['stable_segment_digest'],hashlib.sha256(encoded[:result['necessary_rebuild_prefix_bytes']]).hexdigest())
        # The primitive must retain byte prefixes even inside a shared UTF-8 lead.
        from sic_core.frontier import common
        left='中'.encode();right='丰'.encode();count=common(left,right)
        self.assertEqual(count,2)
        self.assertNotEqual(hashlib.sha256(left[:count]).hexdigest(),hashlib.sha256(left[:count].decode('utf-8',errors='replace').encode()).hexdigest())

    def test_length_budget_control_and_teaching_negative_examples(self):
        self.publish(self.source('first'))
        job=self.candidate('a'*1200,{'I':{'latest':'I-ONLY-NOT-PERSISTENT'}})
        zero=self.compare(job,0);self.assertFalse(zero['within_local_window'])
        accepted=self.compare(job);self.assertTrue(accepted['within_local_window'])
        for cid in self.op('chain.get')['cbs']:self.assertNotIn('I-ONLY-NOT-PERSISTENT',json.dumps(self.core.store.get(cid,'cb')))
        self.op('config.update',{'worker_prompt':'EARLIER-TEACHING-CHANGE'})
        changed=self.compare(job);self.assertFalse(changed['within_local_window']);self.assertTrue(changed['teaching_change_against_previous'])
        self.op('tools.configure',{'tools':[]})
        changed=self.compare(job);self.assertFalse(changed['within_local_window']);self.assertTrue(changed['control_change_against_previous'])
        with self.assertRaises(CoreError):self.op('frontier.compare',{'job_id':job,'expected_cut':-1})

if __name__=='__main__':unittest.main()
