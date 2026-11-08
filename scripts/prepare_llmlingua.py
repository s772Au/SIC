"""Public official fixed-revision LLMLingua-2 preparation; no login or provider keys."""
import hashlib,json,re,time
from pathlib import Path
from urllib.request import Request,urlopen
ROOT=Path(__file__).resolve().parents[1]
MODEL='microsoft/llmlingua-2-bert-base-multilingual-cased-meetingbank'
REV='5f0c82792b7ea14c6484e015b6a072009496b7f2'
WEIGHT_SHA='22b9ecde52fec5c97e8c54a293be768727df95a81c6c8dccb03f262a50c58324'
WEIGHT_BYTES=709388104

def sha(path):
 h=hashlib.sha256()
 with path.open('rb') as f:
  for b in iter(lambda:f.read(8*1024*1024),b''):h.update(b)
 return h.hexdigest()

def download(url,target,size,expected=None):
 target.parent.mkdir(parents=True,exist_ok=True)
 if target.is_file() and target.stat().st_size==size and (not expected or sha(target)==expected):return
 partial=target.with_suffix(target.suffix+'.part')
 for attempt in range(6):
  offset=partial.stat().st_size if partial.exists() else 0
  if offset==size:break
  if offset>size:raise ValueError('Partial exceeds locked file size')
  try:
   req=Request(url+'?download=true&sic_resume='+str(offset)+'&attempt='+str(attempt)+'&sic_fetch='+str(time.time_ns()),headers={'Range':f'bytes={offset}-{size-1}','User-Agent':'SIC-public-model-preparation'})
   with urlopen(req,timeout=60) as response:
    status=response.status;content_range=response.headers.get('Content-Range')
    if status==206:
     if content_range!=f'bytes {offset}-{size-1}/{size}':raise ValueError('Unexpected Content-Range: '+str(content_range))
    elif status!=200 or offset!=0:raise ValueError('Resume requires exact206; received '+str(status))
    with partial.open('ab' if offset else 'wb') as f:
     last=time.monotonic()
     while True:
      block=response.read(4*1024*1024)
      if not block:break
      f.write(block);offset+=len(block)
      if offset>size:raise ValueError('Oversized download')
      if time.monotonic()-last>10:print(json.dumps({'file':target.name,'bytes':offset,'total':size}),flush=True);last=time.monotonic()
   if partial.stat().st_size==size:break
  except Exception as e:
   print(json.dumps({'file':target.name,'attempt':attempt+1,'error':type(e).__name__,'detail':str(e)[:180]}),flush=True)
 else:raise RuntimeError('Public download exhausted retries; partial retained')
 if partial.stat().st_size!=size:raise ValueError('Incomplete download')
 if expected and sha(partial)!=expected:raise ValueError('Final weight SHA mismatch; not accepted')
 partial.replace(target)

def main():
 out=ROOT/'.local/models/llmlingua2-mbert';files=['README.md','config.json','special_tokens_map.json','tokenizer.json','tokenizer_config.json','vocab.txt']
 info=json.load(urlopen('https://huggingface.co/api/models/'+MODEL+'/revision/'+REV+'?blobs=true',timeout=60));assert info['sha']==REV
 existing=ROOT/'research/compression/pack/llmlingua-model.lock.json'
 pinned={f['path']:f['sha256'] for f in json.loads(existing.read_text(encoding='utf-8')).get('files',[])} if existing.is_file() else {}
 locked=[]
 for name in files+['model.safetensors']:
  entry=next(x for x in info['siblings'] if x['rfilename']==name);size=entry.get('size') or entry['lfs']['size'];expected=entry.get('lfs',{}).get('sha256') or pinned.get(name);path=out/name
  download('https://huggingface.co/'+MODEL+'/resolve/'+REV+'/'+name,path,size,expected)
  locked.append({'path':name,'bytes':path.stat().st_size,'sha256':sha(path),'official_lfs_sha256':expected})
 assert next(x for x in locked if x['path']=='model.safetensors')['sha256']==WEIGHT_SHA
 tokenizer_url='https://openaipublic.blob.core.windows.net/encodings/cl100k_base.tiktoken'
 cache=ROOT/'.local/llmlingua/tiktoken-cache';cache_file=cache/hashlib.sha1(tokenizer_url.encode()).hexdigest()
 download(tokenizer_url,cache_file,1681126,'223921b76ee99bde995b7ff738513eef100fb51d18c93597a113bcffe865b2a7')
 lock={'schema':'sic-local-compressor-model-v1','model':MODEL,'revision':REV,'format':'safetensors','weight_license':'Apache-2.0 as declared by official model card','code_license':'MIT','public_token_used':False,'remote_code':False,'files':locked,'parameters':{'use_llmlingua2':True,'device_map':'cpu','dtype':'float32','torch_threads':2,'rate':0.5,'force_tokens':['\n','.'],'force_reserve_digit':True,'drop_consecutive':True,'max_batch_size':1,'max_force_token':100,'seed':42},'local_default':'PROJECT_ROOT/.local/models/llmlingua2-mbert','auxiliary_tokenizer':{'name':'cl100k_base','purpose':'upstream origin/compressed counting, separate from mBERT classifier tokenizer; not provider billing','source':tokenizer_url,'files':[{'path':cache_file.name,'bytes':cache_file.stat().st_size,'sha256':sha(cache_file)}]}}
 p=ROOT/'research/compression/pack/llmlingua-model.lock.json';p.write_text(json.dumps(lock,ensure_ascii=False,indent=2)+'\n',encoding='utf-8');print(json.dumps({'status':'model-files-verified','revision':REV,'files':len(locked),'weight_sha256':WEIGHT_SHA}),flush=True)
if __name__=='__main__':main()
