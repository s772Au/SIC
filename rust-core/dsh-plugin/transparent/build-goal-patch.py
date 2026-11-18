"""Regenerate the reviewed DSH source patch without changing its checkout."""
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
script = (
    'import {prepareGoalRuntime} from ' + json.dumps((root / 'prepare-goal-runtime.mjs').as_uri()) + ';'
    'const {files,original}=await prepareGoalRuntime(process.argv[1],process.argv[2]);'
    'process.stdout.write(JSON.stringify({files,original}));'
)
result = subprocess.run(['node', '--input-type=module', '-e', script, args.dsh_root, args.fixture_root],
                        check=True, capture_output=True, encoding='utf-8')
changes = json.loads(result.stdout)
patch = ''
for key, text in changes['files'].items():
    old = changes['original'][key]
    if text == old:
        continue
    package, filename = key.split('/')
    name = f'packages/goal/{package}/src/{filename}'
    patch += ''.join(difflib.unified_diff(old.splitlines(True), text.splitlines(True),
                                        fromfile='a/' + name, tofile='b/' + name))
(root / 'goal-external-state.patch').write_text(patch, encoding='utf-8', newline='\n')
print('Generated the selected-agent GoalService, native driver, command, and receipt-projection patch.')
