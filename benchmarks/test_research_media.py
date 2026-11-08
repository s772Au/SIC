"""Native media task controls: legal synthetic structure, never official scores."""
import hashlib,json,sys,tempfile,unittest
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1];sys.path[:0]=[str(ROOT),str(ROOT/'scripts')]
from PIL import Image
from benchmarks.prepare_core_lme import prepare
from benchmarks.core_lme_carrier import load_prepared_trajectories
from scripts.run_compression_research import arm

class MediaResearchControls(unittest.TestCase):
 def make_data(self,d):
  (d/'haystacks').mkdir();(d/'screenshots').mkdir();(d/'question_screenshots').mkdir()
  for name,color in [('screenshots/a.png',(30,50,90)),('screenshots/b.png',(90,30,50)),('question_screenshots/q.png',(50,90,30))]:Image.new('RGB',(32,32),color).save(d/name)
  (d/'questions.jsonl').write_text(json.dumps({'id':'q','question':'QUESTION_ONLY_FINAL','image':'question_screenshots/q.png','answer':'WITHHELD_GOLD','eval_function':'exact'})+'\n')
  (d/'haystacks/lme_v2_small.json').write_text(json.dumps({'q':['b','a']}))
  (d/'trajectories.jsonl').write_text('\n'.join(json.dumps({'id':i,'domain':'web','goal':'remember history','outcome':'success','states':[{'state_index':0,'url':'https://example.test/'+i,'action':None,'thought':None,'accessibility_tree':'state '+i,'screenshot':'screenshots/'+i+'.png'}]}) for i in ['a','b'])+'\n')
  task=prepare(d);task['final_question']='SIC_RESEARCH_FINAL_QUESTION\nQUESTION_ONLY_FINAL';return task
 def test_program_control_final_consumer_retains_images_and_hides_question(self):
  with tempfile.TemporaryDirectory() as tmp:
   d=Path(tmp);task=self.make_data(d);out=d/'run';pack=json.loads((ROOT/'research/compression/pack/control-pack.json').read_text())
   arm(out,task,'program_exact',pack,None,[],4096,0,d)
   result=json.loads((out/'result.json').read_text());requests=json.loads((out/'model-http.json').read_text())
   self.assertEqual(len(result['final_consumption']['actual_image_sha256']),3);self.assertEqual(result['final_consumption']['actual_image_sha256'],result['final_consumption']['expected_image_sha256'])
   self.assertNotIn('QUESTION_ONLY_FINAL',json.dumps(requests[:-1]));self.assertNotIn('WITHHELD_GOLD',json.dumps(requests));self.assertEqual(result['final_answer'],'OWNED_FINAL_POST_EDIT_RESPONSE')
 def test_prepared_record_or_image_mutation_rejected(self):
  with tempfile.TemporaryDirectory() as tmp:
   d=Path(tmp);task=self.make_data(d);Image.new('RGB',(32,32),(1,2,3)).save(d/'screenshots/b.png')
   with self.assertRaisesRegex(ValueError,'Screenshot changed'):list(load_prepared_trajectories(task,d))
if __name__=='__main__':unittest.main()
