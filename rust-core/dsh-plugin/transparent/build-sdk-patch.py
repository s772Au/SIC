import argparse
from pathlib import Path
import json,difflib
parser=argparse.ArgumentParser()
parser.add_argument('--overlay',type=Path,required=True)
args=parser.parse_args()
root=Path(__file__).resolve().parent
records=json.loads(args.overlay.read_text(encoding='utf-8'))
source,patched=next(iter(records.items()));original=Path(source).read_text(encoding='utf-8');name='packages/llm/llm/src/index.ts'
patch=''.join(difflib.unified_diff(original.splitlines(True),patched.splitlines(True),fromfile='a/'+name,tofile='b/'+name)).replace('\n \n','\n\n')
(root/'harness-sdk-context.patch').write_text(patch,encoding='utf-8',newline='\n')
print('Generated common LlmRuntime native SDK context patch.')
