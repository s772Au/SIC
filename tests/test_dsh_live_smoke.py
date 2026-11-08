"""Verifier tests use fabricated evidence explicitly; no provider/harness claim."""
import importlib.util,json,tempfile,unittest
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
spec=importlib.util.spec_from_file_location('dsh_live_verifier',ROOT/'scripts/verify_dsh_live_run.py');mod=importlib.util.module_from_spec(spec);spec.loader.exec_module(mod)

class DshLiveSmokeVerifierTests(unittest.TestCase):
    def setUp(self):self.temp=tempfile.TemporaryDirectory();self.addCleanup(self.temp.cleanup);self.output=Path(self.temp.name)/'prepared';mod.prepare(mod.DEFAULT_TASKS,self.output);self.tasks=mod.read_json(mod.DEFAULT_TASKS)
    def fake_logs(self,case):
        logs={};materials=self.output/case['id']/'materials'
        for index,stage in enumerate(case['stages']):
            rows=[{'type':'session','sessionId':'session-unit-test-only'}]
            for n,name in enumerate(stage['required_reads']):
                call='unit-'+str(index)+'-'+str(n);rows += [{'type':'tool_call','tool':'read','callId':call,'input':{'file_path':str(materials/name)}},{'type':'tool_result','callId':call,'status':'completed','result':{'content':[{'type':'text','text':(materials/name).read_text(encoding='utf-8')}]}}]
            rows.append({'type':'final','text':json.dumps(case['answer']) if index==len(case['stages'])-1 else '{"phase":"ready"}'})
            path=Path(self.temp.name)/(stage['id']+'.jsonl');path.write_text('\n'.join(json.dumps(row) for row in rows),encoding='utf-8');logs[stage['id']]=path
        return logs
    def test_prepare_no_gold_in_controller_or_prompts_and_all_fixture_hashes(self):
        plan=mod.read_json(self.output/'controller-plan.json');self.assertFalse(plan['contains_ground_truth']);self.assertNotIn('answer',plan)
        self.assertNotIn('seal-N904',(self.output/'retention/answer.prompt.txt').read_text())
        self.assertNotIn('violet-station',(self.output/'late-correction/corrected.prompt.txt').read_text())
        self.assertNotIn('B-804',(self.output/'multi-hop/resolve.prompt.txt').read_text())
        for case in plan['cases']:
            for name,sha in case['fixture_sha256'].items():self.assertEqual(mod.digest(Path(case['working_directory'])/name),sha)
    def test_all_three_mechanical_fixtures_never_receive_quality_scores(self):
        for case in self.tasks['cases']:
            result=mod.verify(mod.DEFAULT_TASKS,case['id'],self.output,self.fake_logs(case),'mechanical-fixture');self.assertTrue(result['mechanical_passed']);self.assertEqual(result['quality']['status'],'not_evaluated')
    def test_scoring_exact_fields_sets_and_bool_types(self):
        for case in self.tasks['cases']:self.assertTrue(mod.answer_score(case,json.dumps(case['answer']))['exact_match'])
        case=self.tasks['cases'][2];answer=dict(case['answer']);answer['can_ship']=1;self.assertFalse(mod.answer_score(case,json.dumps(answer))['exact_match'])
        case=self.tasks['cases'][0];answer=dict(case['answer']);answer['evidence']=answer['evidence']*2;self.assertFalse(mod.answer_score(case,json.dumps(answer))['exact_match'])
    def test_old_constraint_extra_fields_and_fenced_json_fail(self):
        case=self.tasks['cases'][1];answer=dict(case['answer']);answer['courier']='ivy';self.assertFalse(mod.answer_score(case,json.dumps(answer))['exact_match']);answer=dict(case['answer']);answer['explanation']='extra';self.assertFalse(mod.answer_score(case,json.dumps(answer))['exact_match']);self.assertFalse(mod.answer_score(case,'```json\n'+json.dumps(case['answer'])+'\n```')['format_valid'])
    def test_altered_fixtures_and_missing_reads_fail(self):
        case=self.tasks['cases'][0];logs=self.fake_logs(case);path=self.output/'retention/materials/intake.txt';path.write_text('changed',encoding='utf-8')
        with self.assertRaisesRegex(ValueError,'fixture file changed'):mod.verify(mod.DEFAULT_TASKS,case['id'],self.output,logs,'mechanical-fixture')
    def test_new_session_on_correction_and_invented_tool_result_rejected(self):
        case=self.tasks['cases'][1];logs=self.fake_logs(case);path=logs['corrected'];rows=mod.cli_records(path);rows[0]['sessionId']='session-different';path.write_text('\n'.join(json.dumps(row) for row in rows),encoding='utf-8')
        with self.assertRaisesRegex(ValueError,'same native session'):mod.verify(mod.DEFAULT_TASKS,case['id'],self.output,logs,'mechanical-fixture')
        logs=self.fake_logs(case);rows=mod.cli_records(logs['corrected']);rows[2]['result']={'text':'invented'};logs['corrected'].write_text('\n'.join(json.dumps(row) for row in rows),encoding='utf-8')
        with self.assertRaisesRegex(ValueError,'actual fixture'):mod.verify(mod.DEFAULT_TASKS,case['id'],self.output,logs,'mechanical-fixture')
    def test_real_mode_requires_native_evidence(self):
        case=self.tasks['cases'][0]
        with self.assertRaisesRegex(ValueError,'isolated native session evidence'):mod.verify(mod.DEFAULT_TASKS,case['id'],self.output,self.fake_logs(case),'nonfixed-gateway-exploratory')

class CoreConsumptionProofTests(unittest.TestCase):
    def setUp(self):
        from sic_core import Core
        self.temp=tempfile.TemporaryDirectory();self.addCleanup(self.temp.cleanup);self.db=Path(self.temp.name)/'core.db';self.core=Core(db=str(self.db));self.addCleanup(self.core.close);self.chain=self.op('chain.create')['chain_id'];self.text='[E1] Complete original fact that may be compressed away.';self.call='read-evidence-call'
        self.source=self.op('source.ingest',{'request':{'messages':[{'role':'assistant','content':None,'tool_calls':[{'id':self.call,'type':'function','function':{'name':'read','arguments':'{"file_path":"fixture.txt"}'}}]},{'role':'tool','tool_call_id':self.call,'content':self.text}]}})['source_id']
        self.evidence=[{'file':'fixture.txt','call_id':self.call,'lines':[self.text]}]
    def op(self,name,p=None):return self.core.operate(name,{'actor':'host','chain_id':getattr(self,'chain',None),'allow_write':True,'allow_model':False},p or {})['result']
    def publish(self,source):
        job=self.op('edit.prepare',{'source_id':source})['job_id'];self.op('edit.submit',{'job_id':job,'editor':'editor-1','ops':[{'op':'COMPRESS','target':'u1','content':'short retained representation'}]});state=self.op('chain.get');return self.op('cut.publish',{'job_id':job,'expected_cut':state['cut_seq'],'idempotency_key':job})
    def projection(self):
        actual=self.op('provider.preview');request=actual['request']
        with self.core.store.transaction():
            rid=self.core.store.put('provider_request',{'chain_id':self.chain,'role':'worker','request':request,'cut_id':actual['cut_id']});response=self.core.store.put('provider_response',{'sic_context':{'request_id':rid,'chain_id':self.chain}})
        return {'request_id':rid,'response_id':response,'request':request,'request_digest':mod.object_digest(request),'cut_id':actual['cut_id']}
    def proof(self,projection):
        proof=mod.CoreConsumptionProof(self.db)
        try:return proof.prove([projection],self.evidence)
        finally:proof.close()
    def test_raw_pending_result_consumption_without_cb(self):self.assertEqual(self.proof(self.projection())['reads'][0]['mode'],'original-pending-WB')
    def test_legal_compress_has_exact_source_to_published_cb_evidence(self):
        published=self.publish(self.source);projection=self.projection();self.assertNotIn(self.text,json.dumps(projection['request']));before=mod.digest(self.db);result=self.proof(projection);self.assertEqual(result['reads'][0]['mode'],'published-CB');self.assertEqual(result['reads'][0]['adopted_cb_id'],published['cb_id']);self.assertEqual(mod.digest(self.db),before)
    def test_higher_cb_ancestor_is_actual_selected_representation(self):
        first=self.publish(self.source);formed=self.op('hierarchy.form',{'level':2,'cb_ids':[first['cb_id']]});second=self.publish(formed['source_id']);self.op('hierarchy.select',{'level':2,'roles':['worker']});result=self.proof(self.projection());self.assertEqual(result['reads'][0]['mode'],'published-higher-CB-ancestor');self.assertEqual(result['reads'][0]['cb_path'],[second['cb_id'],first['cb_id']])
    def test_unadopted_or_modified_projection_rejected(self):
        self.publish(self.source);projection=self.projection();projection['request']={'messages':[{'role':'user','content':'unrelated fabricated request'}]}
        with self.assertRaisesRegex(ValueError,'differs from immutable actual request'):self.proof(projection)
    def test_immutable_digest_tampering_rejected(self):
        self.publish(self.source);projection=self.projection()
        with self.core.store.transaction():
            self.core.store.db.execute('DROP TRIGGER immutable_objects_update');self.core.store.db.execute('UPDATE objects SET digest=? WHERE id=?',('bad',self.source))
        with self.assertRaisesRegex(ValueError,'immutable object digest mismatch'):self.proof(projection)
    def test_read_source_revocation_rejected(self):
        self.publish(self.source);projection=self.projection()
        with self.core.store.transaction():self.core.store.db.execute('INSERT INTO revocations VALUES(?,?)',(self.source,'{}'))
        with self.assertRaisesRegex(ValueError,'revoked'):self.proof(projection)

class DshRunnerSessionTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        spec=importlib.util.spec_from_file_location('dsh_live_runner_unit',ROOT/'scripts/run_dsh_live_tests.py');cls.runner=importlib.util.module_from_spec(spec);spec.loader.exec_module(cls.runner)
    def test_first_session_and_same_session_resume(self):
        events=[{'type':'status','phase':'turn_start'},{'type':'session','sessionId':'session-unit-A'}]
        self.assertEqual(self.runner.validate_stage_session(events),'session-unit-A');self.assertEqual(self.runner.validate_stage_session(events,'session-unit-A'),'session-unit-A')
    def test_missing_multiple_or_changed_session_rejected(self):
        for events,previous in [([],None),([{'type':'session','sessionId':'session-unit-A'},{'type':'session','sessionId':'session-unit-B'}],None),([{'type':'session','sessionId':'session-unit-B'}],'session-unit-A')]:
            with self.subTest(events=events,previous=previous):
                with self.assertRaises((AssertionError,ValueError)):self.runner.validate_stage_session(events,previous)

if __name__=='__main__':unittest.main()
