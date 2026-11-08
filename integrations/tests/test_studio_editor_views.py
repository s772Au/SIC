"""Selected Studio Editor views against real Core HTTP and official LangGraph SDK."""
import copy,json,os
from pathlib import Path
import sys,tempfile,unittest
ROOT=Path(__file__).resolve().parents[2]
sys.path[:0]=[str(ROOT),str(ROOT/'integrations'),str(ROOT/'scripts')]
from langgraph.checkpoint.memory import InMemorySaver
from langgraph.types import Command
from sic_workflow.domain_graphs import EditorForm,editor_request,make_graph
from sic_workflow.editor_views import editor_postprocess
from v2_experiment_fixture import OwnedFixture


class EditorSelectedViewTests(unittest.TestCase):
    evidence=[]
    def setUp(self):
        self.saved={key:os.environ.get(key) for key in ('SIC_CORE_URL','SIC_CORE_TOKEN','SIC_CORE_ACTOR')}
        self.temp=tempfile.TemporaryDirectory(prefix='sic-editor-selected-');self.fixture=OwnedFixture(Path(self.temp.name)/'core.db');self.fixture.__enter__();self.client=self.fixture.client
        os.environ.update(SIC_CORE_URL=self.client.origin,SIC_CORE_TOKEN=self.client.token,SIC_CORE_ACTOR='host')
        self.chain=self.client.call('chain.create')['chain_id'];self.client.call('config.update',{'memory_chars':65536,'editor_memory_chars':65536})
        profiles={}
        for editor in ('alpha','beta'):
            profiles[editor+'-frozen']={**self.fixture.profile,'model':'openai/'+editor+'-frozen-model','api_key':'SECRET_MODEL_CREDENTIAL','headers':{'Authorization':'SECRET_MODEL_CREDENTIAL'}}
        self.fixture.core.config['profiles']=profiles
        self.client.call('config.update',{'editor_profiles':{'alpha':'alpha-frozen','beta':'beta-frozen'}})
        def source(text,context='main'):
            return self.client.call('source.ingest' if context=='main' else 'hierarchy.context.ingest',{'context_id':context,'request':{'messages':[{'role':'user','content':text}]}})['source_id']
        def publish(sid,marker):
            job=self.client.call('edit.program',{'source_id':sid,'pvi':{'I':{'scope':marker}}})['job_id'];return self.client.call('cut.publish',{'job_id':job,'expected_cut':self.client.call('chain.get')['cut_seq'],'idempotency_key':job})['cb_id']
        self.main=publish(source('PUBLIC MAIN BACKGROUND'),'PUBLIC MAIN I')
        self.client.call('hierarchy.context.create',{'context_id':'alpha-private','shared_contexts':[]})
        member=publish(source('ALPHA PRIVATE ORIGINAL','alpha-private'),'ALPHA PRIVATE I1')
        for level in (2,3):member=publish(self.client.call('hierarchy.form',{'context_id':'alpha-private','level':level,'cb_ids':[member]})['source_id'],'ALPHA PRIVATE I'+str(level))
        self.private=member;self.client.call('hierarchy.select',{'context_id':'alpha-private','level':3,'roles':['editor:alpha']})
        self.client.call('hierarchy.select',{'level':1,'roles':['editor:beta']})
        self.source=self.client.call('source.ingest',{'request':{'messages':[{'role':'user','content':'alpha editable frozen unit'},{'role':'user','content':'unassigned unit stays implicit KEEP'}]}})['source_id']
        self.job=self.client.call('edit.prepare',{'source_id':self.source,'assignments':[{'editor':'alpha','unit_ids':['u1']},{'editor':'beta','unit_ids':[]}],'readonly_reads':[{'kind':'cb','object_id':self.main,'offset':0,'limit':512}]})['job_id']
        self.client.call('edit.submit',{'job_id':self.job,'editor':'alpha','ops':[{'op':'REPLACE','target':'u1','content':'ORIGINAL ALPHA DELIVERY'}],'pvi':{'I':{'delivery_note':'original delivery I'}}})
        self.client.call('edit.submit',{'job_id':self.job,'editor':'beta','ops':[]})
        self.original=self.client.call('chain.get')['jobs'][self.job]
        self.client.call('edit.candidate.update',{'job_id':self.job,'expected_revision':0,'ops':[{'op':'REPLACE','target':'u1','content':'MANUAL CANDIDATE DISTINCT FROM ORIGINAL'}],'pvi':{}})
        self.client.call('hierarchy.select',{'level':1,'roles':['editor:alpha']})
        self.fixture.core.config['profiles']['alpha-frozen']['model']='openai/changed-live-default'
        self.other=self.client.call('edit.prepare',{'source_id':self.source,'assignments':[{'editor':'alpha','unit_ids':['u1']}]})['job_id']
        self.graph=make_graph('selected_editor_view',EditorForm,'chain.get',editor_request,InMemorySaver(),postprocess=editor_postprocess)
    def tearDown(self):
        self.fixture.__exit__(None,None,None);self.temp.cleanup()
        for key,value in self.saved.items():
            if value is None:os.environ.pop(key,None)
            else:os.environ[key]=value
    @classmethod
    def tearDownClass(cls):
        report={'scope':'real owned Core HTTP + official LangGraph SDK; Studio browser reload still required','runs':cls.evidence}
        dest=ROOT/'.local/contract-rebuild/studio-editor-selected-evidence.json';dest.parent.mkdir(parents=True,exist_ok=True);dest.write_text(json.dumps(report,ensure_ascii=False,indent=2),encoding='utf-8')
    def run_form(self,form,thread):
        count=len(self.fixture.model_requests);state=self.client.call('chain.get');result=self.graph.invoke({'chain_id':self.chain,'form':form},{'configurable':{'thread_id':thread}})
        self.assertEqual(len(self.fixture.model_requests),count);self.assertEqual(self.client.call('chain.get'),state)
        self.evidence.append({'test':self.id(),'form':form,'result':result,'model_requests_during_read':0})
        return result
    def test_selected_job_frozen_scope_private_background_original_delivery_and_models(self):
        result=self.run_form({'action':'read','job_id':self.job},'selected')
        self.assertEqual(result['status'],'applied');self.assertTrue(result['result']['ok']);view=result['task_view']
        self.assertEqual(view['job_id'],self.job);self.assertNotEqual(view['job_id'],self.other)
        self.assertEqual([u['text'] for u in view['source_snapshot']['units']],['alpha editable frozen unit','unassigned unit stays implicit KEEP'])
        self.assertEqual(result['assignment_rows'][1]['unit_ids'],[]);self.assertEqual(result['scope_rows'][1]['assigned_editor'],'implicit KEEP')
        self.assertEqual([cb['cb_id'] for cb in view['editor_backgrounds']['alpha']['cbs']],[self.private])
        self.assertEqual([cb['cb_id'] for cb in view['editor_backgrounds']['beta']['cbs']],[self.main])
        self.assertEqual(view['editor_instants']['alpha']['scope'],'ALPHA PRIVATE I3');self.assertEqual(view['editor_instants']['beta']['scope'],'PUBLIC MAIN I')
        self.assertEqual(view['responses'],self.original['responses'])
        self.assertIn('ORIGINAL ALPHA DELIVERY',json.dumps(result['response_rows']));self.assertNotIn('MANUAL CANDIDATE DISTINCT',json.dumps(result['response_rows']))
        self.assertEqual(view['model_bindings']['alpha']['model'],'openai/alpha-frozen-model')
        self.assertNotIn('SECRET_MODEL_CREDENTIAL',json.dumps(result));self.assertNotIn('sidecar',view['source_snapshot'])
        self.assertIn('PUBLIC MAIN BACKGROUND',json.dumps(view['readonly_reads']))
        self.assertEqual(result['result']['context']['reviewed_cut'],self.original['base_cut'])
        self.assertEqual(result['current']['result']['task_view']['job_id'],self.job)
        self.assertEqual(result['after']['result']['task_view']['job_id'],self.job)
    def test_empty_job_returns_bounded_real_directory_and_can_page(self):
        first=self.run_form({'action':'read','directory_limit':2},'directory-first')
        second=self.run_form({'action':'read','directory_offset':2,'directory_limit':2},'directory-second')
        self.assertEqual(first['task_mode'],'directory');self.assertEqual(first['task_view'],{})
        self.assertEqual(len(first['task_directory']),2);self.assertEqual(first['task_navigation']['next_offset'],2)
        self.assertTrue({x['job_id'] for x in first['task_directory']}.isdisjoint({x['job_id'] for x in second['task_directory']}))
        self.assertEqual(first['result']['result']['task_directory'],first['task_directory'])
        self.assertNotIn('jobs',first['current']['result']);self.assertNotIn('ALPHA PRIVATE ORIGINAL',json.dumps(first))
    def test_nonexistent_or_foreign_job_visibly_fails_without_guessing_another(self):
        base=copy.deepcopy(self.client.context);foreign=self.client.call('chain.create')['chain_id']
        self.client.context['chain_id']=foreign;sid=self.client.call('source.ingest',{'request':{'messages':[{'role':'user','content':'FOREIGN JOB MATERIAL'}]}})['source_id'];foreign_job=self.client.call('edit.program',{'source_id':sid})['job_id'];self.client.context=base
        for requested in ('job-does-not-exist',foreign_job):
            result=self.run_form({'action':'read','job_id':requested},'invalid-'+requested)
            self.assertEqual(result['status'],'failed');self.assertFalse(result['result']['ok']);self.assertEqual(result['result']['error']['code'],'editor_job_not_found');self.assertEqual(result['task_view'],{});self.assertEqual(result['scope_rows'],[])
            self.assertTrue(result['result']['evidence']['core_read_succeeded']);self.assertNotIn('FOREIGN JOB MATERIAL',json.dumps(result))
    def test_prepare_still_uses_real_interrupt_and_route_bound_resume(self):
        config={'configurable':{'thread_id':'prepare-reviewed'}}
        form={'action':'prepare','source_id':self.source,'editor_one_name':'gamma','editor_one_units':['u1']}
        paused=self.graph.invoke({'chain_id':self.chain,'form':form},config);self.assertTrue(paused.get('__interrupt__'))
        self.assertNotIn('SECRET_MODEL_CREDENTIAL',json.dumps(paused,default=str))
        answer=paused['__interrupt__'][0].value['form']|{'allow_write':True}
        result=self.graph.invoke(Command(resume=answer),config)
        self.assertTrue(result['result']['ok']);self.assertEqual(result['result']['result']['job_id'],result['task_view']['job_id']);self.assertEqual(result['task_view']['assignments'],[{'editor':'gamma','unit_ids':['u1']}]);self.assertEqual(result['result']['context']['chain_id'],self.chain);self.assertEqual(self.fixture.model_requests,[])
        self.evidence.append({'test':self.id(),'actual_interrupt':True,'result':result,'model_requests':0})
        cfg={'configurable':{'thread_id':'route-injection-rejected'}};paused=self.graph.invoke({'chain_id':self.chain,'form':form},cfg)
        with self.assertRaises(ValueError):self.graph.invoke(Command(resume=paused['__interrupt__'][0].value['form']|{'chain_id':'other-route'}),cfg)

if __name__=='__main__':unittest.main()
