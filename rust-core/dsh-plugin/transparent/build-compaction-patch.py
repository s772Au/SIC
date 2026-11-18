"""Build a reviewable native patch without modifying the DSH checkout."""
from pathlib import Path
import argparse
import difflib
import json
import subprocess

parser = argparse.ArgumentParser()
parser.add_argument('--dsh-root', required=True)
parser.add_argument('--fixture-root', required=True)
args = parser.parse_args()

root = Path(__file__).resolve().parent
script = ('import {prepareCompactionRuntime} from ' + json.dumps((root/'prepare-compaction-runtime.mjs').as_uri())
          + ';process.stdout.write(JSON.stringify(await prepareCompactionRuntime(process.argv[1],process.argv[2])));')
result = subprocess.run(['node','--input-type=module','-e',script,args.dsh_root,args.fixture_root],
    check=True,capture_output=True,encoding='utf-8')
changes=json.loads(result.stdout)
patch=''
for key,text in changes['files'].items():
    old=changes['original'][key]
    if text==old: continue
    pkg,name=key.split('/')
    path=f'packages/{"core" if pkg=="agent-loop" else "compaction"}/{pkg}/src/{name}'
    patch+=''.join(difflib.unified_diff(old.splitlines(True),text.splitlines(True),fromfile='a/'+path,tofile='b/'+path))
(root/'compaction-external-state.patch').write_text(patch,encoding='utf-8',newline='\n')
print('Generated external BasicCompaction, native pruner and original Agent request resolver patch.')
