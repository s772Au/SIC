"""New-core-only acceptance entrypoint. No legacy scores or real model calls."""
import argparse,hashlib,importlib.util,json,os,sys,time,unittest
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
sys.path.insert(0,str(ROOT))
def main():
    p=argparse.ArgumentParser();p.add_argument('--require-sdk',action='store_true');p.add_argument('--json-output');args=p.parse_args()
    available=importlib.util.find_spec('litellm') is not None
    if args.require_sdk and not available:p.error('Optional pinned LiteLLM SDK is required for this acceptance run')
    os.environ.setdefault('LITELLM_LOCAL_MODEL_COST_MAP','true');os.environ.setdefault('LITELLM_TELEMETRY','false');started=time.monotonic()
    suite=unittest.defaultTestLoader.discover(str(ROOT/'tests'),pattern='test_core*.py');result=unittest.TextTestRunner(verbosity=2).run(suite)
    from importlib.metadata import version
    report={'implementation':'sic-core-v2','tests_run':result.testsRun,'failures':len(result.failures),'errors':len(result.errors),'skipped':len(result.skipped),'successful':result.wasSuccessful(),'duration_seconds':time.monotonic()-started,'sdk_version':version('litellm') if available else None,'provider_scope':'owned-loopback-mock-only; no live provider validation','source_sha256':{str(path.relative_to(ROOT)).replace('\\','/'):hashlib.sha256(path.read_bytes()).hexdigest() for path in sorted((ROOT/'sic_core').glob('*')) if path.is_file()}}
    if args.json_output:
        target=Path(args.json_output);target.parent.mkdir(parents=True,exist_ok=True);target.write_text(json.dumps(report,ensure_ascii=False,indent=2),encoding='utf-8')
    return 0 if result.wasSuccessful() else 1
if __name__=='__main__':raise SystemExit(main())
