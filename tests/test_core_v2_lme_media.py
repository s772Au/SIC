"""Pinned official LME interface/harness/LAFS mechanical acceptance, never task-quality scores."""
import base64,copy,hashlib,importlib.util,json,os
from pathlib import Path
import subprocess,sys,tempfile,unittest
ROOT=Path(__file__).resolve().parents[1]
from benchmarks.prepare_core_lme import prepare
from benchmarks.run_core_lme import verify_official


@unittest.skipUnless(os.environ.get('SIC_LME_OFFICIAL_ROOT'),'explicit pinned official checkout needed')
class OfficialLMEMediaTests(unittest.TestCase):
    evidence=[]
    @classmethod
    def setUpClass(cls):
        cls.official=verify_official(os.environ['SIC_LME_OFFICIAL_ROOT'])
        paths=[str(cls.official)]
        if os.environ.get('SIC_LME_OFFICIAL_DEPS'):paths.insert(0,os.environ['SIC_LME_OFFICIAL_DEPS'])
        if os.environ.get('SIC_LME_TORCH_DEPS'):paths.insert(0,os.environ['SIC_LME_TORCH_DEPS'])
        sys.path[:0]=paths
        if os.environ.get('SIC_LME_TORCH_SUPPORT'):sys.path.append(os.environ['SIC_LME_TORCH_SUPPORT'])
        from benchmarks.core_lme_memory import SICCoreMemory
        from memory_modules.memory import build_memory,load_memory,save_memory
        from evaluation.harness import validate_memory_context_items,build_messages
        cls.Memory=SICCoreMemory;cls.build=staticmethod(build_memory);cls.load=staticmethod(load_memory);cls.save=staticmethod(save_memory);cls.validate=staticmethod(validate_memory_context_items);cls.messages=staticmethod(build_messages)
    @classmethod
    def tearDownClass(cls):
        path=os.environ.get('SIC_LME_EVIDENCE')
        if path:Path(path).write_text(json.dumps({'scope':'official interface+carrier+harness+LAFS mechanical fixture; no model-quality claim','runs':cls.evidence},ensure_ascii=False,indent=2),encoding='utf-8')
    def data(self,root):
        from PIL import Image
        root.mkdir(exist_ok=True);(root/'screenshots').mkdir();(root/'haystacks').mkdir()
        Image.new('RGB',(32,24),'red').save(root/'screenshots/a.png');Image.new('RGB',(24,32),'blue').save(root/'screenshots/b.png')
        trajectories=[{'id':'trajectory-'+str(i),'domain':'web','goal':'owned task '+str(i),'outcome':'observed','states':[{'state_index':0,'accessibility_tree':'unique fact '+str(i),'action':{'type':'click','target':'button'},'thought':'fixture suggestion, not performed extra action','url':'https://fixture.invalid/'+str(i),'screenshot':'screenshots/'+name}]} for i,name in enumerate(('a.png','b.png'))]
        (root/'trajectories.jsonl').write_text(''.join(json.dumps(x)+'\n' for x in trajectories));(root/'questions.jsonl').write_text(json.dumps({'id':'owned-q','domain':'web','image':'screenshots/a.png','question_type':'static-environment','question':'owned question','answer':'fixture gold kept outside memory','eval_function':'norm_phrase_set_match'})+'\n');(root/'haystacks/lme_v2_small.json').write_text(json.dumps({'owned-q':[x['id'] for x in trajectories]}))
        return trajectories
    def test_official_insert_query_persist_restore_and_reader_image_carrier(self):
        from benchmarks.core_lme_memory import load_prepared_trajectories
        with tempfile.TemporaryDirectory(prefix='sic-lme-interface-') as d:
            root=Path(d);data=root/'data';trajectories=self.data(data);task=prepare(data)
            memory=self.build({'memory_type':'sic_core_v01','memory_params':{'data_root':str(data),'fanout':2}})
            try:
                for trajectory in load_prepared_trajectories(task,data):memory.insert(trajectory)
                memory.insert(trajectories[0]);self.assertEqual(len(memory.inserted),2)
                context=self.validate(memory.query('unique fact'),question_id='owned-fixture-only')
                self.assertEqual(sum(x['type']=='image' for x in context),2)
                self.assertNotIn('fixture gold',json.dumps(context))
                messages,log=self.messages('official fixture system','owned question',None,context)
                actual=[x for x in messages[1]['content'] if x['type']=='image_url']
                original=[hashlib.sha256((data/'screenshots'/x).read_bytes()).hexdigest() for x in ('a.png','b.png')]
                self.assertEqual([hashlib.sha256(base64.b64decode(x['image_url']['url'].split(',',1)[1])).hexdigest() for x in actual],original)
                self.save(memory,root/'saved');restored=self.load(root/'saved')
                try:
                    restored.configure_runtime(data_root=root/'dataset-no-longer-mounted')
                    again=self.validate(restored.query('unique fact'),question_id='restored-fixture-only')
                    self.assertEqual([hashlib.sha256(Path(x['value']).read_bytes()).hexdigest() for x in again if x['type']=='image'],original)
                    self.assertEqual(restored.inserted,memory.inserted)
                finally:restored.close()
                changed=copy.deepcopy(trajectories[0]);changed['goal']='changed'
                with self.assertRaises(ValueError):memory.insert(changed)
                self.evidence.append({'test':self.id(),'prepared_source':task['source'],'images_sha256':original,'official_carrier':log,'metadata':memory.post_query_hook(),'research_evaluation':'not_evaluated'})
            finally:memory.close()
    def test_actual_official_harness_builds_saved_core_memory_without_reader_or_judge(self):
        with tempfile.TemporaryDirectory(prefix='sic-lme-harness-') as d:
            root=Path(d);data=root/'data';self.data(data);config=root/'memory.json';config.write_text(json.dumps({'memory_type':'sic_core_v01','memory_params':{'data_root':str(data)}}))
            env=dict(os.environ);env['PYTHONIOENCODING']='utf-8';env['PYTHONPATH']=os.pathsep.join([os.environ.get('SIC_LME_OFFICIAL_DEPS',''),str(ROOT)])
            command=[sys.executable,str(ROOT/'benchmarks/run_core_lme.py'),'--official-root',str(self.official),'harness','--domain','web','--questions-path',str(data/'questions.jsonl'),'--haystack-path',str(data/'haystacks/lme_v2_small.json'),'--trajectories-path',str(data/'trajectories.jsonl'),'--memory-config-path',str(config),'--output-dir',str(root/'run'),'--save-memory','--skip-evaluation']
            result=subprocess.run(command,env=env,capture_output=True,text=True,encoding='utf-8',timeout=120)
            self.assertEqual(result.returncode,0,result.stdout+'\n'+result.stderr)
            saved=self.load(root/'run/memory_state')
            try:self.assertEqual(len(saved.inserted),2);self.assertEqual(sum(x['type']=='image' for x in saved.query('unique')),2)
            finally:saved.close()
            self.evidence.append({'test':self.id(),'stdout':result.stdout,'stderr':result.stderr,'official_reader_called':False,'official_judge_called':False,'research_evaluation':'not_evaluated'})
    def test_official_model_editor_interface_uses_real_sdk_text_only_scope(self):
        from test_core_v2_media import MediaSDKTests
        fixture=MediaSDKTests('test_sdk_editor_readonly_media_high_order_worker_and_real_library_admission');fixture.setUp()
        try:
            with tempfile.TemporaryDirectory(prefix='sic-lme-model-') as d:
                root=Path(d);trajectories=self.data(root)
                memory=self.build({'memory_type':'sic_core_v01','memory_params':{'data_root':str(root),'editor_mode':'model'}})
                try:
                    memory.configure_runtime(core_config={'roles':{'worker':dict(fixture.profile),'editor':dict(fixture.profile)}})
                    trajectory=copy.deepcopy(trajectories[0]);second=copy.deepcopy(trajectory['states'][0]);second['state_index']=1;second['screenshot']='screenshots/b.png';trajectory['states'].append(second)
                    memory.insert(trajectory)
                    self.assertEqual(len(fixture.calls),2)
                    self.assertEqual(len(memory.inserted[trajectory['id']]['cb_ids']),2)
                    payload=json.loads(fixture.calls[0]['messages'][-1]['content'])
                    self.assertEqual(len(payload['readonly_media']),1);self.assertNotIn('base64',json.dumps(payload))
                    self.assertTrue(payload['editable_text_units']);self.assertEqual(sum(x['type']=='image' for x in memory.query('unique fact')),2)
                    self.evidence.append({'test':self.id(),'sdk_requests':fixture.calls,'ledger':memory._op('ledger.export'),'research_evaluation':'not_evaluated'})
                finally:memory.close()
        finally:fixture.tearDown()

    @unittest.skipUnless(os.environ.get('SIC_LME_PROCESSOR_PATH'),'explicit source-locked official tensor processor needed')
    def test_official_real_tensor_count_and_truncation(self):
        from evaluation import harness
        from benchmarks.run_core_lme import configure_processor_loader,verify_processor
        path=verify_processor(os.environ['SIC_LME_PROCESSOR_PATH']);configure_processor_loader(harness,path)
        with tempfile.TemporaryDirectory(prefix='sic-lme-tokenizer-') as d:
            root=Path(d);self.data(root)
            context=[{'type':'text','value':'mechanical official image count'},{'type':'image','value':str(root/'screenshots/a.png')}]
            images=harness.load_memory_context_images(context);count=harness.count_memory_context_tokens(context,images)
            truncated,original,kept=harness.truncate_memory_context(context,max_tokens=count-1,question_id='mechanical-only')
            self.assertEqual(original,count);self.assertEqual(len(truncated),1);self.assertLess(kept,count)
            self.evidence.append({'test':self.id(),'actual_official_tensor_count':count,'kept_tokens':kept,'kept_items':len(truncated),'billed_provider_tokens':None,'research_evaluation':'not_evaluated'})

    def test_prepared_task_tamper_path_and_official_source_tamper_refused(self):
        from benchmarks.core_lme_memory import load_prepared_trajectories,screenshot_block
        with tempfile.TemporaryDirectory(prefix='sic-lme-negative-') as d:
            root=Path(d);self.data(root);task=prepare(root)
            bad=copy.deepcopy(task);bad['ordered_trajectories'].reverse()
            with self.assertRaises(ValueError):list(load_prepared_trajectories(bad,root))
            (root/'screenshots/a.png').write_bytes(b'changed')
            with self.assertRaises(ValueError):list(load_prepared_trajectories(task,root))
            with self.assertRaises(ValueError):screenshot_block(root,'../secret.png')
            with self.assertRaises(ValueError):verify_official(root)
    def test_actual_official_public_preparation_keeps_question_image_and_haystack_order(self):
        from benchmarks.run_core_lme import materialize_official_inputs
        from benchmarks.core_lme_carrier import question_request
        with tempfile.TemporaryDirectory(prefix='sic-lme-public-') as d:
            root=Path(d);data=root/'data';self.data(data)
            result=materialize_official_inputs(self.official,data,'web','small',root/'runtime')
            self.assertEqual(result['question_images'],1);self.assertEqual(result['trajectory_count'],2)
            questions=json.loads((root/'runtime/questions.json').read_text(encoding='utf-8'))
            self.assertTrue(Path(questions[0]['question']['image']).is_file())
            task=prepare(data);request=question_request(task,data)
            self.assertEqual(len(request['messages'][0]['content']),2)
            self.assertEqual(task['question_image']['relative_path'],'screenshots/a.png')
            self.assertNotIn('fixture gold',json.dumps(request))
            self.evidence.append({'test':self.id(),'official_questions_image_preserved':True,'task_question_media_sha256':task['question_image']['sha256']})

    def test_actual_official_lafs_function_and_fixture_limit(self):
        from leaderboard.compute_lafs import Point,lafs,lafs_gain_for_submission,fixed_frontier_points
        self.assertEqual(lafs([Point('mechanical fixture',100,1)]),100)
        self.assertEqual(lafs_gain_for_submission(fixed_frontier_points['small'],[]),0)
        self.evidence.append({'test':self.id(),'official_function_executed':True,'fixture_algebra_only':True,'task_lafs':None,'research_evaluation':'not_evaluated'})

if __name__=='__main__':unittest.main()
