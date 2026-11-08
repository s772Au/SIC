"""Package only verified public model/tokenizer assets; never private Core configuration."""
import hashlib,json,zipfile
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
def sha(p):
 h=hashlib.sha256()
 with p.open('rb') as f:
  for b in iter(lambda:f.read(8*1024*1024),b''):h.update(b)
 return h.hexdigest()
def main():
 pack=ROOT/'research/compression/pack';lock=json.loads((pack/'llmlingua-model.lock.json').read_text());model=ROOT/'.local/models/llmlingua2-mbert';cache=ROOT/'.local/llmlingua/tiktoken-cache';entries=[]
 for f in lock['files']:
  p=model/f['path']
  if p.stat().st_size!=f['bytes'] or sha(p)!=f['sha256']:raise ValueError('Model verification failed')
  entries.append((p,'models/llmlingua2-mbert/'+f['path']))
 for f in lock['auxiliary_tokenizer']['files']:
  p=cache/f['path']
  if sha(p)!=f['sha256']:raise ValueError('Tokenizer verification failed')
  entries.append((p,'tokenizer-cache/'+f['path']))
 for p in (pack/'llmlingua-licenses').iterdir():entries.append((p,'licenses/'+p.name))
 entries.extend([(pack/'llmlingua-model.lock.json','llmlingua-model.lock.json'),(pack/'llmlingua-licenses.json','llmlingua-licenses.json'),(ROOT/'research/compression/llmlingua-requirements.lock.txt','llmlingua-requirements.lock.txt')])
 out=ROOT/'benchmarks/packages/sic-llmlingua2-mbert-model.zip';temp=out.with_suffix('.zip.part')
 with zipfile.ZipFile(temp,'w',zipfile.ZIP_STORED,allowZip64=True) as z:
  for p,name in entries:z.write(p,name)
 temp.replace(out)
 report={'schema':'sic-public-local-model-bundle-v1','path':out.relative_to(ROOT).as_posix(),'bytes':out.stat().st_size,'sha256':sha(out),'revision':lock['revision'],'weight_license':'Apache-2.0','code_license':'MIT','contains_private_configuration':False,'entries':len(entries),'restoration':'Extract in any private directory; set SIC_LLMLINGUA_MODEL_DIR to models/llmlingua2-mbert and SIC_LLMLINGUA_TOKENIZER_CACHE to tokenizer-cache; install hashed dependencies into separate Python3.12 x86_64 environment.'}
 (pack/'llmlingua-bundle.json').write_text(json.dumps(report,indent=2)+'\n');print(json.dumps(report))
if __name__=='__main__':main()
