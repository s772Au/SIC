"""Fail-closed adapter to a separate official LLMLingua environment and verified CPU weights."""
import json,os,subprocess,tempfile
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]

def compress_units(units,evidence_dir=None):
    python=Path(os.environ.get('SIC_LLMLINGUA_PYTHON',str(ROOT/'.local/llmlingua-venv/Scripts/python.exe')))
    model=Path(os.environ.get('SIC_LLMLINGUA_MODEL_DIR',str(ROOT/'.local/models/llmlingua2-mbert')))
    cache=Path(os.environ.get('SIC_LLMLINGUA_TOKENIZER_CACHE',str(ROOT/'.local/llmlingua/tiktoken-cache')))
    if not python.is_file() or not model.is_dir():raise ValueError('Prepared isolated LLMLingua interpreter/weights missing; run prepare_llmlingua and install frozen dependencies. No fallback.')
    with tempfile.TemporaryDirectory(prefix='sic-local-compressor-') as directory:
        inp=Path(directory)/'input.json';out=Path(directory)/'output.json';inp.write_text(json.dumps({'units':units},ensure_ascii=False),encoding='utf-8')
        env=os.environ.copy();env.pop('PYTHONPATH',None)
        for key in list(env):
            if key.upper().endswith(('API_KEY','TOKEN','PASSWORD')) or key in ('HF_TOKEN','HUGGING_FACE_HUB_TOKEN'):env.pop(key,None)
        result=subprocess.run([str(python),str(ROOT/'scripts/llmlingua_worker.py'),'--input',str(inp),'--output',str(out),'--model-dir',str(model),'--cache-dir',str(cache)],env=env,cwd=ROOT,capture_output=True,text=True,encoding='utf-8',timeout=1800)
        if result.returncode or not out.is_file():raise RuntimeError('Official local LLMLingua process failed (no fallback): '+result.stderr[-1500:])
        value=json.loads(out.read_text(encoding='utf-8'))
    if not value.get('classifier_tensor_matches_weight_file') or value.get('model_forward_calls',0)<1:raise ValueError('Actual official weight inference evidence missing')
    if evidence_dir:
        evidence_dir=Path(evidence_dir);evidence_dir.mkdir(parents=True,exist_ok=True);files=list(evidence_dir.glob('llmlingua-*.json'));path=evidence_dir/f'llmlingua-{len(files):04}.json';path.write_text(json.dumps(value,ensure_ascii=False,indent=2)+'\n',encoding='utf-8')
    return value
