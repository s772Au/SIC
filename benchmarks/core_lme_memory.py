"""SIC Core plugin for the pinned official LongMemEval-V2 Memory interface.

Import this module after the official repository is on PYTHONPATH to register
``sic_core_v01``. No downstream question IDs, answers, or evaluators enter Core.
"""
import base64
import copy
import hashlib
import json
import mimetypes
import os
from pathlib import Path,PurePosixPath
import sqlite3
import tempfile
import threading
import time
from sic_core import Core
from sic_core import media
from memory_modules.memory import Memory,register_memory

from .core_lme_carrier import OFFICIAL_REVISION,DATA_REVISION,screenshot_block,trajectory_request,load_prepared_trajectories


@register_memory
class SICCoreMemory(Memory):
    memory_type='sic_core_v01'
    def __init__(self,memory_params):
        super().__init__(memory_params)
        allowed={'data_root','editor_mode','program_rules','memory_chars','fanout','active_level','image_detail','wb_state_count'}
        if set(memory_params)-allowed:raise ValueError('Unknown SIC LME memory parameter')
        self.data_root=Path(memory_params.get('data_root','benchmarks/data/longmemeval-v2'))
        self.editor_mode=memory_params.get('editor_mode','program')
        if self.editor_mode not in ('program','model'):raise ValueError('Editor mode must be program or model')
        self.wb_state_count=memory_params.get('wb_state_count',1)
        if not isinstance(self.wb_state_count,int) or isinstance(self.wb_state_count,bool) or self.wb_state_count<1:raise ValueError('wb_state_count must be a positive integer')
        self.detail=memory_params.get('image_detail','high')
        if self.detail not in ('low','high'):raise ValueError('LME screenshots require explicit low/high image detail')
        self._temporary=tempfile.TemporaryDirectory(prefix='sic-lme-core-');self.directory=Path(self._temporary.name)
        self._lock=threading.RLock();self.cancel_event=None;config_path=os.environ.get('SIC_LME_CORE_CONFIG_PATH');core_config=json.loads(Path(config_path).read_text(encoding='utf-8')) if config_path else {};self.core=Core(config=core_config);self.context={'actor':'lme-host','allow_write':True,'allow_model':self.editor_mode=='model'}
        self.context['chain_id']=self._op('chain.create')['chain_id'];self.inserted={};self.insertion_progress={};self.query_local=threading.local()
        self._op('config.update',{'memory_chars':int(memory_params.get('memory_chars',65536)),'editor_memory_chars':int(memory_params.get('memory_chars',65536))})
    def _op(self,name,payload=None):return self.core.operate(name,self.context,payload or {})['result']
    def configure_runtime(self,**kwargs):
        if set(kwargs)-{'core_config','data_root','query_trace_dir','generation_temperature','generation_top_p','cancel_event'}:raise ValueError('Unknown SIC runtime override')
        if 'cancel_event' in kwargs:self.cancel_event=kwargs['cancel_event']
        if 'core_config' in kwargs:
            if self.inserted or self.insertion_progress:raise ValueError('Configure frozen role models before inserting trajectories')
            self.core.config=copy.deepcopy(kwargs['core_config'])
            from sic_core.adapters import LiteLLMProvider
            self.core.provider=LiteLLMProvider(self.core.config.get('roles',{}))
        if 'data_root' in kwargs:self.data_root=Path(kwargs['data_root'])
    def insert(self,trajectory):
        if self.cancel_event is not None and self.cancel_event.is_set():raise RuntimeError('Official harness cancelled memory work')
        request=trajectory_request(trajectory,self.data_root,self.detail);fingerprint=hashlib.sha256(json.dumps(request,ensure_ascii=False,sort_keys=True).encode()).hexdigest();identity=trajectory['id']
        with self._lock:
            if identity in self.inserted:
                if self.inserted[identity]['sha256']!=fingerprint:raise ValueError('Trajectory identity reused with changed original evidence')
                return
            progress=self.insertion_progress.setdefault(identity,{'sha256':fingerprint,'source_ids':[],'cb_ids':[],'completed_chunks':0})
            if progress['sha256']!=fingerprint:raise ValueError('Partial trajectory identity reused with changed evidence')
            states=request['messages'][1:]
            chunks=[{'messages':([request['messages'][0]] if start==0 else [])+states[start:start+self.wb_state_count]} for start in range(0,len(states),self.wb_state_count)] or [request]
            for index,chunk in enumerate(chunks):
                if index<progress['completed_chunks']:continue
                sid=self._op('source.ingest',{'request':chunk,'idempotency_key':'lme:'+identity+':'+str(index)})['source_id']
                prepared=self._op('edit.prepare' if self.editor_mode=='model' else 'edit.program',{'source_id':sid,'rules':self.memory_params.get('program_rules',['deduplicate_exact'])})
                if self.editor_mode=='model':self._op('edit.run',{'job_id':prepared['job_id'],'editor':'editor-1'})
                published=self._op('cut.publish',{'job_id':prepared['job_id'],'expected_cut':self._op('chain.get')['cut_seq'],'idempotency_key':'lme:'+identity+':'+str(index)})
                progress['source_ids'].append(sid);progress['cb_ids'].append(published['cb_id']);progress['completed_chunks']=index+1
                if self.memory_params.get('fanout'):
                    self._op('hierarchy.maintain',{'fanout':self.memory_params['fanout'],'roles':['worker'],'rules':self.memory_params.get('program_rules',['deduplicate_exact'])})
            self.inserted[identity]={'sha256':fingerprint,'source_id':progress['source_ids'][0],'cb_id':progress['cb_ids'][-1],'source_ids':list(progress['source_ids']),'cb_ids':list(progress['cb_ids']),'wb_state_count':self.wb_state_count}
            self.insertion_progress.pop(identity)
            if self.memory_params.get('active_level'):
                self._op('hierarchy.select',{'level':self.memory_params['active_level'],'roles':['worker']})
    def query(self,query,query_image=None):
        if self.cancel_event is not None and self.cancel_event.is_set():raise RuntimeError('Official harness cancelled memory query')
        if not isinstance(query,str):raise ValueError('Query must be text')
        if query_image is not None and not Path(query_image).is_file():raise ValueError('Query image is missing')
        started=time.perf_counter()
        with self._lock,self.core.store.transaction():
            state=self.core.store.state(self.context['chain_id']);state['_actor']='lme-host';state['config']['task_goal']=query
            view=self.core._working_view(state,'worker');items=[]
            for cb,carriers in zip(view['cbs'],view['media_carriers']):
                items.append({'type':'text','value':json.dumps({'cb_id':cb['cb_id'],'level':cb.get('level'),'body':cb['body'],'operations':cb['ops'],'archive_navigation':cb.get('archive_navigation')},ensure_ascii=False)})
                for message in carriers:
                    items.append({'type':'text','value':json.dumps({'original_role':message['role'],'tool_call_id':message.get('tool_call_id'),'tool_calls':message.get('tool_calls',[])},ensure_ascii=False)})
                    content=message.get('content')
                    if isinstance(content,str):items.append({'type':'text','value':content})
                    elif isinstance(content,list):
                        for block in content:
                            if block['type']=='text':items.append({'type':'text','value':block['text']})
                            else:
                                info=media.image_info(block);suffix=mimetypes.guess_extension(info['mime_type']) or '.image';path=self.directory/(info['sha256']+suffix);raw=base64.b64decode(block['image_url']['url'].split(',',1)[1])
                                if path.exists():
                                    if hashlib.sha256(path.read_bytes()).hexdigest()!=info['sha256']:raise ValueError('Cached immutable screenshot changed')
                                else:path.write_bytes(raw)
                                items.append({'type':'image','value':str(path)})
            if view['instant'].get('memory_projection'):items.append({'type':'text','value':json.dumps({'sic_projection':view['instant']['memory_projection']},ensure_ascii=False)})
            self.query_local.last={'query_seconds':time.perf_counter()-started,'selected_cb_ids':[x['cb_id'] for x in view['cbs']],'context_items':len(items),'image_items':sum(x['type']=='image' for x in items),'query_image_policy':'question image remains reader input; retrieval ranking uses query text','physical_cache_hit':'unknown','quality':'not_evaluated'}
            return items
    def post_query_hook(self,**kwargs):return copy.deepcopy(getattr(self.query_local,'last',{}))
    def _save_backend(self,output_dir):
        with self._lock:
            destination=sqlite3.connect(str(Path(output_dir)/'sic-core.sqlite'));self.core.store.db.backup(destination);destination.close()
            (Path(output_dir)/'sic-state.json').write_text(json.dumps({'context':self.context,'inserted':self.inserted,'insertion_progress':self.insertion_progress,'official_revision':OFFICIAL_REVISION,'data_revision':DATA_REVISION},ensure_ascii=False),encoding='utf-8')
    def _load_backend(self,input_dir):
        with self._lock:
            saved=json.loads((Path(input_dir)/'sic-state.json').read_text(encoding='utf-8'))
            if saved['official_revision']!=OFFICIAL_REVISION or saved['data_revision']!=DATA_REVISION:raise ValueError('Official interface/data revision mismatch')
            source=sqlite3.connect(str(Path(input_dir)/'sic-core.sqlite'));source.backup(self.core.store.db);source.close();self.context=saved['context'];self.inserted=saved['inserted'];self.insertion_progress=saved.get('insertion_progress',{})
    def close(self):self.core.close();self._temporary.cleanup()
