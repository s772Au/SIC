"""Build the reviewed native producer/control patch without modifying DSH."""
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
script = ('import {prepareJobsRuntime} from ' + json.dumps((root / 'prepare-jobs-runtime.mjs').as_uri()) + ';'
          'const {files,original}=await prepareJobsRuntime(process.argv[1],process.argv[2]);'
          'process.stdout.write(JSON.stringify({files,original}));')
result = subprocess.run(['node', '--input-type=module', '-e', script, args.dsh_root, args.fixture_root],
                        check=True, capture_output=True, encoding='utf-8')
changes = json.loads(result.stdout)
patch = ''
for key, text in changes['files'].items():
    old = changes['original'][key]
    if old == text:
        continue
    group, package, *path = key.split('/')
    name = 'packages/' + group + '/' + package + '/src/' + '/'.join(path)
    patch += ''.join(difflib.unified_diff(old.splitlines(True), text.splitlines(True),
                                        fromfile='a/' + name, tofile='b/' + name))
(root / 'jobs-external-lifecycle.patch').write_text(patch, encoding='utf-8', newline='\n')
print('Generated native jobs, controllers, shell, subagent, and workflow lifecycle patch.')
