"""Local engine setup failures must never become substitute compression results."""
import os,sys,tempfile,unittest
from pathlib import Path
from unittest.mock import patch
ROOT=Path(__file__).resolve().parents[1];sys.path[:0]=[str(ROOT),str(ROOT/'scripts')]
from scripts.llmlingua_adapter import compress_units
class LocalCompressorBoundaries(unittest.TestCase):
 def test_missing_isolated_interpreter_rejected_without_subprocess(self):
  with tempfile.TemporaryDirectory() as d,patch.dict(os.environ,{'SIC_LLMLINGUA_PYTHON':str(Path(d)/'missing.exe')}),patch('scripts.llmlingua_adapter.subprocess.run') as run:
   with self.assertRaisesRegex(ValueError,'No fallback'):compress_units([{'unit_id':'u1','text':'text'}])
   run.assert_not_called()
 def test_missing_model_rejected_without_download(self):
  with tempfile.TemporaryDirectory() as d,patch.dict(os.environ,{'SIC_LLMLINGUA_PYTHON':sys.executable,'SIC_LLMLINGUA_MODEL_DIR':str(Path(d)/'missing-model')}),patch('scripts.llmlingua_adapter.subprocess.run') as run:
   with self.assertRaisesRegex(ValueError,'No fallback'):compress_units([{'unit_id':'u1','text':'text'}])
   run.assert_not_called()
if __name__=='__main__':unittest.main()
