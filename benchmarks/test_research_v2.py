"""Minimal legal structural fixtures are not official benchmark findings."""
import hashlib,importlib.util,json,sys,tempfile,unittest,os,copy
from pathlib import Path
from unittest.mock import patch
ROOT=Path(__file__).resolve().parents[1];sys.path[:0]=[str(ROOT),str(ROOT/'scripts')]
from scripts.research_scoring import score_annotations,score_answer,score_task_rubric,digest_text
from benchmarks import core_adapters
from benchmarks.prepare_core_lme import prepare
spec=importlib.util.spec_from_file_location('research_controls',ROOT/'research/compression/pack/controls.py');controls=importlib.util.module_from_spec(spec);spec.loader.exec_module(controls)

class ResearchPackage(unittest.TestCase):
    def test_adapter_reference_is_not_leaked_to_worker_material(self):
        case={'id':'x','context':'hello甲🙂world','protected':[],'locator':{'path':'benchmarks/data/fixture.jsonl'},'questions':[{'id':'q','question':'Which port?','reference':'SECRET_REF','rubric':None,'category':'fixture','scorer':'fixture'}]}
        with patch.object(core_adapters,'cases',return_value=iter([case])):task=next(core_adapters.tasks('babilong-qa1',chunk_bytes=4))
        self.assertEqual(''.join(task['blocks']),case['context']);self.assertNotIn('SECRET_REF',''.join(task['blocks'])+task['goal']);self.assertEqual(task['source_content_sha256'],digest_text(case['context']))
    def test_three_noise_types_and_legal_suggestion(self):
        source='Port 7193. Attempt failed. Suggest inspect config.';candidate='Port 8000. Succeeded. New fact: GPU. Suggest retry.'
        annotation={'source_sha256':digest_text(source),'candidate_sha256':digest_text(candidate),'reviewer':'independent-fixture-labels','coverage':'exhaustive','claims':[{'span':[0,10],'labels':['source_distortion'],'rationale':'Port changed','evidence':[{'span':[0,10]}]},{'span':[11,20],'labels':['source_distortion'],'rationale':'Failed became completed','evidence':[{'span':[11,25]}]},{'span':[21,35],'labels':['unsupported_fact'],'rationale':'GPU not in source','evidence':[]},{'span':[36,len(candidate)],'labels':['suggestion'],'rationale':'Explicitly proposed','evidence':[]}],'format_checks':[{'rule':'ops-json','status':'fail','rationale':'Plain prose not the declared envelope'}]}
        result=score_annotations(source,candidate,annotation);self.assertEqual(result['noise']['source_distortion']['count'],2);self.assertEqual(result['noise']['unsupported_fact']['count'],1);self.assertEqual(result['noise']['format_drift']['count'],1);self.assertEqual(result['legal_suggestions'],1)
        with self.assertRaises(ValueError):score_annotations(source,candidate+'!',annotation)
        self.assertEqual(score_annotations(source,candidate,annotation,fixture=True)['status'],'not_evaluated')
    def test_missing_answer_unknown_and_fixture_not_scored(self):
        self.assertIsNone(score_answer(None,'x','normalized_exact')['score']);self.assertEqual(score_answer('X','x','normalized_exact')['score'],1);self.assertIsNone(score_answer('x','x','normalized_exact',True)['score'])
    def test_task_rubric_requires_evidence_and_unknown_is_not_zero(self):
        a={'reviewer':'independent','answer_sha256':digest_text('answer'),'criteria':[{'criterion':'cross_level_traceability','status':'unknown','rationale':'not reviewed','evidence':[]}]}
        self.assertIsNone(score_task_rubric('answer',a)['score']);self.assertEqual(score_task_rubric('answer',a)['status'],'unknown')
        a['criteria'][0]['status']='pass'
        with self.assertRaises(ValueError):score_task_rubric('answer',a)
        a['criteria'][0]['evidence']=[{'artifact':'sic/trace.json','locator':'op memory.page at index 1'}]
        self.assertEqual(score_task_rubric('answer',a)['score'],1)
        self.assertIsNone(score_task_rubric('answer',a,True)['score'])
    def test_non_llm_controls_and_complete_unit_budget(self):
        units=[{'unit_id':'u1','text':'abc'},{'unit_id':'u2','text':'abc'},{'unit_id':'u3','text':'甲🙂'}]
        self.assertEqual(controls.unit_ops(units)[0]['target'],'u2');ops=controls.unit_ops(units,'langchain_trim',2);self.assertEqual([x['target'] for x in ops],['u1','u2'])
        with self.assertRaises(ValueError):controls.unit_ops(units,'unavailable-engine')
    def test_final_worker_consumes_edited_controls_and_differs_from_intermediate(self):
        from scripts.run_compression_research import arm
        pack=json.loads((ROOT/'research/compression/pack/control-pack.json').read_text(encoding='utf-8'))
        task={'task_id':'owned-final-consumption','goal':'Which port?','blocks':['Fact: 7193.','Proposal: inspect config.'],'final_question':'SIC_RESEARCH_FINAL_QUESTION\nWhich port?'}
        with tempfile.TemporaryDirectory() as d:
            for engine in ('structured_summary','program_exact'):
                folder=Path(d)/engine;arm(folder,task,engine,pack,None,[],4096,0)
                result=json.loads((folder/'result.json').read_text());ledger=json.loads((folder/'ledger.json').read_text())
                self.assertEqual(result['final_answer'],'OWNED_FINAL_POST_EDIT_RESPONSE')
                self.assertTrue(all(s['answer']=='OWNED_INTERMEDIATE_RESPONSE' for s in result['worker_steps']))
                self.assertTrue(result['final_consumption']['complete_selected_ids_visible'])
                self.assertTrue(result['final_consumption']['actually_visible_cb_ids'])
                self.assertEqual(sum(c['role']=='worker' for c in ledger['calls']),3)
                self.assertTrue(result['pending']);self.assertIn('no post-answer compression',result['endpoint_policy'])
    def test_lme_does_not_drop_media_or_change_member_order(self):
        with tempfile.TemporaryDirectory() as directory:
            d=Path(directory);(d/'haystacks').mkdir();(d/'haystacks/lme_v2_small.json').write_text(json.dumps({'q':['b','a']}));(d/'questions.jsonl').write_text(json.dumps({'id':'q','question':'Q','image':None,'answer':'A','eval_function':'exact'})+'\n');state={'state_index':0,'step':0,'url':'x','action':None,'accessibility_tree':'text','screenshot':'screenshots/x.png'}
            (d/'trajectories.jsonl').write_text('\n'.join(json.dumps({'id':i,'domain':'web','goal':'g','outcome':'success','states':[state]}) for i in ['a','b']))
            task=prepare(d);self.assertEqual([x['id'] for x in task['ordered_trajectories']],['b','a']);self.assertTrue(task['media_required']);self.assertEqual(len(task['missing_images']),2);self.assertEqual(task['execution_status'],'blocked-missing-images')



class ExploratoryGatewayBoundaries(unittest.TestCase):
    def profile(self):
        role={'model':'openai/test-alias','model_revision':'declared-alias-not-fixed','api_base':'http://127.0.0.1:8341/v1','api_key_env':'SIC_RESEARCH_UNIT_ONLY','route_id':'unit-route','tokenizer_model':'gpt-4o','max_input_tokens':1024,'max_output_tokens':128,'context_window':4096,'sampling':{'temperature':0,'top_p':1}}
        return {'roles':{'worker':copy.deepcopy(role),'editor':copy.deepcopy(role)}}
    def test_default_gateway_denied_before_network(self):
        from scripts.research_provider import ConfiguredCore,public_config
        with tempfile.TemporaryDirectory() as d,patch('sic_core.server.make_server') as server:
            with self.assertRaisesRegex(ValueError,'allow-test-gateway'):ConfiguredCore(Path(d)/'db',self.profile()).__enter__()
            with self.assertRaisesRegex(ValueError,'allow-test-gateway'):public_config(self.profile())
            server.assert_not_called()
    def test_gateway_optin_constructs_only_mock_transport_and_still_rejects_url_credentials(self):
        from scripts.research_provider import ConfiguredCore,public_config
        with tempfile.TemporaryDirectory() as d,patch.dict(os.environ,{'SIC_RESEARCH_UNIT_ONLY':'non-secret-unit-value'}),patch('sic_core.server.make_server') as server,patch('sic_core.service.Core') as core,patch('sic_core.adapters.LiteLLMProvider') as provider,patch('scripts.research_provider.threading.Thread') as thread:
            server.return_value.server_port=12345
            with ConfiguredCore(Path(d)/'db',self.profile(),allow_test_gateway=True):pass
            provider.return_value.call.assert_not_called();server.assert_called_once();core.assert_called_once()
            disclosure=public_config(self.profile(),True);self.assertEqual(disclosure['route_status'],'nonfixed-exploratory');self.assertFalse(disclosure['quality_generalization']);self.assertFalse(disclosure['model_identity_attested']);self.assertEqual(disclosure['provider_revision_status'],'nonfixed-exploratory')
            bad=self.profile();bad['roles']['worker']['api_base']='http://user:pass@127.0.0.1:8341/v1'
            with self.assertRaisesRegex(ValueError,'credential-bearing'):ConfiguredCore(Path(d)/'bad',bad,allow_test_gateway=True).__enter__()
            with self.assertRaisesRegex(ValueError,'credential-bearing'):public_config(bad,True)
    def test_cli_propagates_optin_and_marks_manifest_without_model_or_network(self):
        from scripts import run_compression_research as runner
        with tempfile.TemporaryDirectory() as d:
            root=Path(d);profile=root/'profile.json';profile.write_text(json.dumps(self.profile()));task=root/'task.json';task.write_text(json.dumps({'task_id':'unit','goal':'test','blocks':['test'],'references':{'answer':'test'}}));out=root/'out'
            argv=['runner','--task',str(task),'--provider-config',str(profile),'--allow-provider','--allow-test-gateway','--engines','unchanged','--output',str(out)]
            with patch.object(sys,'argv',argv),patch.object(runner,'arm',return_value={'engine':'unchanged','summary':{},'status':'unit-mocked-no-inference'}) as arm:runner.main()
            self.assertTrue(arm.call_args.args[-1]);manifest=json.loads((out/'manifest.json').read_text());config=json.loads((out/'configuration.json').read_text());self.assertEqual(manifest['route_status'],'nonfixed-exploratory');self.assertFalse(manifest['quality_generalization']);self.assertFalse(config['quality_generalization']);self.assertFalse(config['profiles']['model_identity_attested'])




class EditorOperationContract(unittest.TestCase):
    def test_canonical_prompt_schema_and_core_required_fields_agree(self):
        from scripts.prepare_compression_research import EDITOR_OPERATION_CONTRACT,editor_response_schema
        from sic_core.editing import validate_response
        from jsonschema import Draft202012Validator
        pack=ROOT/'research/compression/pack';schema=json.loads((pack/'editor-response.schema.json').read_text(encoding='utf-8'));self.assertEqual(schema,editor_response_schema());Draft202012Validator.check_schema(schema);validator=Draft202012Validator(schema)
        prompt=json.loads((pack/'control-pack.json').read_text(encoding='utf-8'))['prompts']['sic_editor'];self.assertIn(EDITOR_OPERATION_CONTRACT,prompt);self.assertIn('reason is optional explanatory metadata and NEVER substitutes for required content',prompt)
        valid=[{'op':'KEEP','target':'u1'},{'op':'DELETE','target':'u2'},{'op':'FLAG','target':'u1','content':'Only a proposal','reason':'Preserve status'},{'op':'REORDER','target':'u1','order':0},{'op':'COMPRESS','target':'u1','content':'shorter'},{'op':'REPLACE','target':'u1','content':'corrected'},{'op':'ADD','content':'new','anchor':{'target':'u1','position':'after'}},{'op':'FLAG','target':'u1','content':'next','lifecycle':'V'},{'op':'ADD','content':'current','lifecycle':'I','view_key':'status'}]
        pvi={'P':[],'V':[],'I':{}}
        for op in valid:
            with self.subTest(valid=op):validate_response(['u1','u2'],[op],pvi);validator.validate({'ops':[op],'pvi':pvi})
        invalid=[{'op':'FLAG','target':'u1','reason':'annotation mistakenly in reason'},{'op':'ADD'},{'op':'COMPRESS','target':'u1'},{'op':'REPLACE','target':'u1'},{'op':'KEEP'},{'op':'DELETE'},{'op':'REORDER','target':'u1','order':True},{'op':'ADD','content':'x','target':'u1'},{'op':'FLAG','target':'u1','content':'x','anchor':{'target':'u1','position':'before'}},{'op':'ADD','content':'x','anchor':{'target':'u1','position':'beside'}},{'op':'KEEP','target':'u1','lifecycle':'V'},{'op':'ADD','content':'x','lifecycle':'I'}]
        for op in invalid:
            with self.subTest(invalid=op):
                with self.assertRaises(ValueError):validate_response(['u1','u2'],[op],pvi)
                self.assertTrue(list(validator.iter_errors({'ops':[op],'pvi':pvi})))
    def test_complete_minimal_prompt_examples_parse_and_preserve_default_keep(self):
        from sic_core.editing import validate_response,materialize
        from jsonschema import Draft202012Validator
        pack=ROOT/'research/compression/pack';prompt=json.loads((pack/'control-pack.json').read_text(encoding='utf-8'))['prompts']['sic_editor'];validator=Draft202012Validator(json.loads((pack/'editor-response.schema.json').read_text(encoding='utf-8')))
        examples={name:json.loads(next(line.split(': ',1)[1] for line in prompt.splitlines() if line.startswith(name+': '))) for name in ['NO-OP EXAMPLE','FLAG EXAMPLE']}
        for envelope in examples.values():validator.validate(envelope);validate_response(['u1'],envelope['ops'],envelope['pvi'])
        source={'units':[{'unit_id':'u1','text':'Acknowledged.','anchor':{'message':0}}]};value=materialize(source,[{'editor':'editor-1','unit_ids':['u1']}],{'editor-1':examples['NO-OP EXAMPLE']});self.assertEqual(value['body'],'Acknowledged.');self.assertEqual(value['instant'],{});self.assertEqual(value['volatile'],[]);self.assertEqual(examples['FLAG EXAMPLE']['ops'][0]['content'],'Unverified proposal, not an executed action.')

    def test_reserved_decision_tree_example_matches_core_and_other_i_values_remain_free(self):
        from sic_core import decision
        from sic_core.editing import validate_response,materialize
        from jsonschema import Draft202012Validator
        pack=ROOT/'research/compression/pack';prompt=json.loads((pack/'control-pack.json').read_text(encoding='utf-8'))['prompts']['sic_editor'];schema=json.loads((pack/'editor-response.schema.json').read_text(encoding='utf-8'));validator=Draft202012Validator(schema)
        value=json.loads(next(line.split(': ',1)[1] for line in prompt.splitlines() if line.startswith('DECISION-TREE EXAMPLE: ')));validator.validate(value);validate_response([],value['ops'],value['pvi']);tree=value['pvi']['I']['decision_tree'];normalized=decision.from_editor(tree,decision.empty());self.assertEqual(normalized['views'],{name:[] for name in decision.VIEWS})
        tree['Working on']=['Read the assigned task',{'text':'Check the new evidence','evidence':['source#u1'],'id':'retained-id','status':'active'}];value['pvi']['I'].update(status='plain string view',number=3,arbitrary=[None,{'keep':'original shape'}]);validator.validate(value);normalized=decision.from_editor(tree,decision.empty());self.assertEqual(len(normalized['views']['Working on']),2);self.assertEqual(normalized['views']['Working on'][1]['evidence'],['source#u1'])
        instant=materialize({'units':[]},[{'editor':'editor-1','unit_ids':[]}],{'editor-1':value})['instant'];self.assertEqual(instant['arbitrary'],[None,{'keep':'original shape'}]);self.assertEqual(instant['status'],'plain string view')
        for invalid in ['prose decision tree',42,{'Working on':42,'Failed Before':[],'Not Tried Yet':[],'Overlooked':[]},{'Working on':[{'text':42,'evidence':[]}],'Failed Before':[],'Not Tried Yet':[],'Overlooked':[]}]:
            with self.subTest(invalid=invalid):
                with self.assertRaises(ValueError):decision.from_editor(invalid,decision.empty())
                self.assertTrue(list(validator.iter_errors({'ops':[],'pvi':{'I':{'decision_tree':invalid}}})))

    def test_reserved_primary_i_op_keeps_string_ledger_and_decodes_once(self):
        from sic_core.editing import validate_response,materialize
        from jsonschema import Draft202012Validator
        pack=ROOT/'research/compression/pack';prompt=json.loads((pack/'control-pack.json').read_text(encoding='utf-8'))['prompts']['sic_editor'];value=json.loads(next(line.split(': ',1)[1] for line in prompt.splitlines() if line.startswith('DECISION-TREE I-OP EXAMPLE: ')));Draft202012Validator(json.loads((pack/'editor-response.schema.json').read_text(encoding='utf-8'))).validate(value)
        validate_response([],value['ops'],value['pvi'],primary_decision=True);self.assertIsInstance(value['ops'][0]['content'],str);source={'units':[]};assignment=[{'editor':'editor-1','unit_ids':[]}];responses={'editor-1':value};primary=materialize(source,assignment,responses,primary_decision=True);self.assertIsInstance(primary['instant']['decision_tree'],dict);self.assertIsInstance(primary['ops'][0]['content'],str)
        private=materialize(source,assignment,responses);self.assertEqual(private['instant']['decision_tree'],value['ops'][0]['content'])
        invalid=[{'op':'ADD','lifecycle':'I','view_key':'decision_tree','content':'prose, not a decision object'},{'op':'ADD','lifecycle':'I','view_key':'decision_tree','content':'{"Working on":[]'}]
        for op in invalid:
            with self.assertRaises(ValueError):validate_response([],[op],{},primary_decision=True)
            validate_response([],[op],{})

    def test_pack_manifest_locks_current_prompt_and_schema(self):
        pack=ROOT/'research/compression/pack';manifest=json.loads((pack/'manifest.json').read_text(encoding='utf-8'))
        for name in ['control-pack.json','editor-response.schema.json']:
            path=pack/name;entry=manifest['files'][path.relative_to(ROOT).as_posix()];self.assertEqual(entry['bytes'],path.stat().st_size);self.assertEqual(entry['sha256'],hashlib.sha256(path.read_bytes()).hexdigest())

if __name__=='__main__':unittest.main()
