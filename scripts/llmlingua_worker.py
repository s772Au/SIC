"""Isolated CPU LLMLingua-2 execution. Only verified local weights, no provider calls."""
import argparse,hashlib,importlib.metadata,json,os,time
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]

def sha(p):
 h=hashlib.sha256()
 with p.open('rb') as f:
  for b in iter(lambda:f.read(8*1024*1024),b''):h.update(b)
 return h.hexdigest()

def main():
 start=time.monotonic();cpu=time.process_time();p=argparse.ArgumentParser();p.add_argument('--input',type=Path,required=True);p.add_argument('--output',type=Path,required=True);p.add_argument('--model-dir',type=Path,default=ROOT/'.local/models/llmlingua2-mbert');p.add_argument('--cache-dir',type=Path,default=ROOT/'.local/llmlingua/tiktoken-cache');a=p.parse_args()
 for key in ['HF_TOKEN','HUGGING_FACE_HUB_TOKEN']:os.environ.pop(key,None)
 os.environ.update(HF_HOME=str(ROOT/'.local/llmlingua/hf-offline-home'),HF_HUB_CACHE=str(ROOT/'.local/llmlingua/hf-offline-home/hub'),HF_HUB_OFFLINE='1',TRANSFORMERS_OFFLINE='1',HF_HUB_DISABLE_IMPLICIT_TOKEN='1',TOKENIZERS_PARALLELISM='false',TIKTOKEN_CACHE_DIR=str(a.cache_dir))
 lock=json.loads((ROOT/'research/compression/pack/llmlingua-model.lock.json').read_text(encoding='utf-8'))
 for f in lock['files']:
  file=a.model_dir/f['path']
  if not file.is_file() or file.stat().st_size!=f['bytes'] or sha(file)!=f['sha256']:raise ValueError('Verified local model file missing/mismatched: '+f['path'])
 for f in lock['auxiliary_tokenizer']['files']:
  if not (a.cache_dir/f['path']).is_file() or sha(a.cache_dir/f['path'])!=f['sha256']:raise ValueError('Verified offline auxiliary tokenizer cache missing')
 if importlib.metadata.version('llmlingua')!='0.2.2':raise ValueError('Frozen LLMLingua version required')
 import socket
 network_attempts=[]
 def deny_network(self,address):
  network_attempts.append(str(address));raise RuntimeError('Offline local compressor forbids network connections')
 socket.socket.connect=deny_network
 import torch
 cfg=lock['parameters']
 torch.set_num_threads(cfg['torch_threads']);torch.set_num_interop_threads(1);torch.manual_seed(cfg['seed'])
 from llmlingua import PromptCompressor
 from safetensors import safe_open
 request=json.loads(a.input.read_text(encoding='utf-8'));units=request['units'];text='\n\n'.join(u['text'] for u in units)
 compressor=PromptCompressor(model_name=str(a.model_dir),device_map='cpu',model_config={'local_files_only':True,'trust_remote_code':False,'torch_dtype':torch.float32},use_llmlingua2=True,llmlingua2_config={'max_batch_size':cfg['max_batch_size'],'max_force_token':cfg['max_force_token']})
 # Preserve official logical family name for upstream word-boundary dispatch,
 # after strictly local verified-weight loading (no remote loading here).
 compressor.model_name=lock['model']
 compressor.model.eval()
 with safe_open(str(a.model_dir/'model.safetensors'),framework='pt',device='cpu') as f:
  key='classifier.weight';classifier_verified=torch.equal(f.get_tensor(key),compressor.model.state_dict()[key])
 if not classifier_verified:raise ValueError('Loaded classifier does not match official tensor weights')
 calls=[]
 def hook(module,args,out):calls.append({'shape':list(out.logits.shape),'logit_sample':out.logits.detach().flatten()[:8].tolist()})
 handle=compressor.model.register_forward_hook(hook)
 result=compressor.compress_prompt_llmlingua2(text,rate=cfg['rate'],force_tokens=cfg['force_tokens'],force_reserve_digit=cfg['force_reserve_digit'],drop_consecutive=cfg['drop_consecutive'],return_word_label=True)
 handle.remove()
 if not calls:raise ValueError('No actual model forward call; refusing substitute engine')
 content=result['compressed_prompt'];ops=[{'op':'COMPRESS','target':units[0]['unit_id'],'content':content,'reason':'Official local LLMLingua-2 token-classification output; semantic quality not inferred'}]+[{'op':'DELETE','target':u['unit_id'],'reason':'Combined source is represented by the first compressed text unit; native sidecar remains protected'} for u in units[1:]]
 import psutil
 evidence={'schema':'sic-llmlingua-execution-v1','engine':'official-llmlingua-0.2.2','model':lock['model'],'revision':lock['revision'],'weight_sha256':next(f['sha256'] for f in lock['files'] if f['path']=='model.safetensors'),'classifier_tensor_matches_weight_file':classifier_verified,'model_forward_calls':len(calls),'forward_evidence':calls,'parameters':lock['parameters'],'device':str(next(compressor.model.parameters()).device),'dtype':str(next(compressor.model.parameters()).dtype),'input_text':text,'input_sha256':hashlib.sha256(text.encode()).hexdigest(),'raw_compression':result,'ops':ops,'elapsed_seconds':time.monotonic()-start,'process_cpu_seconds':time.process_time()-cpu,'rss_bytes_at_end':psutil.Process().memory_info().rss,'real_provider_calls':0,'network_connect_attempts':len(network_attempts),'network_connections_blocked':True,'research_quality':'not_evaluated','versions':{n:importlib.metadata.version(n) for n in ['llmlingua','torch','transformers','accelerate','tokenizers','tiktoken','safetensors']}}
 a.output.write_text(json.dumps(evidence,ensure_ascii=False,indent=2)+'\n',encoding='utf-8');print(json.dumps({'status':'compressed-with-real-local-weights','forward_calls':len(calls),'origin_tokens':result['origin_tokens'],'compressed_tokens':result['compressed_tokens'],'elapsed_seconds':evidence['elapsed_seconds'],'cpu_seconds':evidence['process_cpu_seconds'],'rss_bytes':evidence['rss_bytes_at_end']}))
if __name__=='__main__':main()
