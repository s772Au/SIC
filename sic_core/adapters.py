"""Explicit text and immutable image ingress formats; LiteLLM owns outbound provider translation."""
import copy, json, os, importlib.util
from . import media

def digest(value):
    import hashlib
    return hashlib.sha256(json.dumps(value,sort_keys=True,ensure_ascii=False,separators=(',',':'),allow_nan=False).encode()).hexdigest()

def decode(fmt, request, known_calls=()):
    if fmt!='openai-chat':
        canonical=_to_chat(fmt,request);value=decode('openai-chat',canonical,known_calls);value['format']=fmt;value['sidecar']['native_request']=copy.deepcopy(request);return value
    if not isinstance(request,dict) or not isinstance(request.get('messages'),list):raise ValueError('messages array required')
    allowed={'messages','model','tools','tool_choice','parallel_tool_calls','temperature','top_p','max_tokens','max_completion_tokens','stop','presence_penalty','frequency_penalty','seed','response_format','user','store','reasoning_effort','prompt_cache_key','prompt_cache_retention'}
    if set(request)-allowed:raise ValueError('unsupported native control field; no silent drop')
    units=[]; images=[]; protected=[]; sidecar=copy.deepcopy(request); calls=set(known_calls); results=set()
    for mi,message in enumerate(request['messages']):
        if message.get('role') not in ('system','developer','user','assistant','tool'):raise ValueError('unsupported role')
        if calls-results and message['role']!='tool':raise ValueError('native tool results must close open calls before next message')
        content=message.get('content')
        if message['role'] in ('system','developer'):
            if units or images:raise ValueError('protected instructions must precede working material')
            protected.append(copy.deepcopy(message));continue
        if content is None:parts=[]
        elif isinstance(content,str):parts=[(None,content)]
        elif isinstance(content,list):
            parts=[]
            for pi,block in enumerate(content):
                if isinstance(block,dict) and block.get('type')=='text' and set(block)=={'type','text'} and isinstance(block.get('text'),str):parts.append((pi,block['text']))
                elif isinstance(block,dict) and block.get('type')=='image_url':
                    info=media.image_info(block);images.append({**info,'anchor':{'message':mi,'part':pi},'role':message['role'],'tool_call_id':message.get('tool_call_id'),'tool_group_ids':sorted((calls-results)|{c['id'] for c in message.get('tool_calls',[])})})
                else:raise ValueError('Unsupported content block; no silent multimodal drop')
        else:raise ValueError('content must be text, text blocks, or null')
        for pi,text in parts:units.append({'unit_id':'u'+str(len(units)+1),'text':text,'anchor':{'message':mi,'part':pi},'kind':'text'})
        for call in message.get('tool_calls') or []:
            call_id=call.get('id')
            if message['role']!='assistant' or not call_id or call_id in calls:raise ValueError('invalid/duplicate native tool call')
            calls.add(call_id)
        if message['role']=='tool':
            call_id=message.get('tool_call_id')
            if call_id not in calls or call_id in results:raise ValueError('orphan/duplicate native tool result')
            results.add(call_id)
    return {'units':units,'sidecar':sidecar,'protected':protected,'open_calls':sorted(calls-results),'format':fmt,**({'media':images} if images else {})}

def encode(view,fmt='openai-chat'):
    if fmt!='openai-chat':return _from_chat(fmt,encode(view,'openai-chat'))
    messages=copy.deepcopy(view.get('protected',[]))
    for cb_index,cb in enumerate(view['cbs']):
        # Body, persistent operation directory and anchors remain model-discoverable.
        messages.append({'role':'user','content':json.dumps({'sic_cb':cb['cb_id'],'wb':cb['source_id'],'body':cb['body'],'operations':cb['ops'],'index':cb['index'],'protocol_events':cb.get('protocol_events',[]),'projection':cb.get('archive_navigation',{'complete':True}),'proposal_refs':cb.get('proposal_refs',{})},ensure_ascii=False,sort_keys=True)})
        messages.extend(copy.deepcopy(view.get('media_carriers',[])[cb_index] if view.get('media_carriers') else []))
    if view.get('instant'):messages.append({'role':'user','content':json.dumps({'sic_instant':view['instant']},ensure_ascii=False,sort_keys=True)})
    for source in view['pending']:
        if source['open_calls']:raise ValueError('native tool group incomplete; legal provider frontier not reached')
        messages.extend(copy.deepcopy([m for m in source['sidecar']['messages'] if m['role'] not in ('system','developer')]))
    if not messages:messages=[{'role':'user','content':'SIC workspace initialized.'}]
    result=copy.deepcopy(view.get('controls',{}));result['messages']=messages
    schemas=view.get('tool_schemas',[])
    if schemas:result['tools']=schemas
    return result

class LiteLLMProvider:
    """Fixed model configuration, no Router/fallback; secret env names only in config."""
    def __init__(self,roles=None):self.roles=roles or {}
    def capabilities(self):
        available=importlib.util.find_spec('litellm') is not None
        return {'implementation':'litellm-in-process','sdk_installed':available,'ingress_formats':['openai-chat','openai-responses','anthropic-messages'],'projection_formats':['openai-chat','openai-responses','anthropic-messages'],'multimodal':{'images':'immutable data URI image_url; Editor text-only; explicit model-specific admission','audio':False,'video':False},'streaming':False,'physical_cache':'unknown-until-provider-evidence','roles':{k:{'configured':bool(v.get('model')),'model':v.get('model'),'api_key_available':bool(os.environ.get(v.get('api_key_env','')))} for k,v in self.roles.items()},'live_provider_verified':False,'silent_failover':False}
    def call(self,role,request):
        cfg=self.roles.get(role)
        if not cfg or not cfg.get('model'):raise ValueError('fixed role model is not configured')
        if not importlib.util.find_spec('litellm'):raise ValueError('optional LiteLLM dependency is not installed')
        os.environ.setdefault('LITELLM_LOCAL_MODEL_COST_MAP','true')
        os.environ.setdefault('LITELLM_TELEMETRY','false')
        import litellm
        required=('tokenizer_model','max_input_tokens','max_output_tokens','context_window')
        if any(not cfg.get(key) for key in required):raise ValueError('exact tokenizer and input/output/context budgets required for configured model')
        tokenizer_model=cfg['tokenizer_model']
        # Host must select a known tokenizer explicitly; never accept LiteLLM unknown-model fallback.
        if tokenizer_model not in litellm.model_cost:raise ValueError('unknown tokenizer model; cannot admit full input precisely')
        media_admission=media.admission(litellm,cfg,request)
        count=litellm.token_counter(model=tokenizer_model,messages=request['messages'],tools=request.get('tools'),tool_choice=request.get('tool_choice'),use_default_image_token_count=False)
        output=request.get('max_completion_tokens',request.get('max_tokens',cfg['max_output_tokens']))
        if not isinstance(output,int) or output<=0 or output>cfg['max_output_tokens']:raise ValueError('output budget exceeded')
        if count>cfg['max_input_tokens'] or count+output>cfg['context_window']:raise ValueError('complete input exceeds model profile budget; no truncation')
        kwargs=copy.deepcopy(request);kwargs.update(model=cfg['model'],num_retries=0,drop_params=False,stream=False,timeout=cfg.get('timeout_seconds',120))
        if cfg.get('api_base'):kwargs['api_base']=cfg['api_base']
        if cfg.get('api_key_env'):
            key=os.environ.get(cfg['api_key_env'])
            if not key:raise ValueError('configured provider key environment variable is absent')
            kwargs['api_key']=key
        if 'max_tokens' not in kwargs and 'max_completion_tokens' not in kwargs:kwargs['max_tokens']=output
        response=litellm.completion(**kwargs)
        value=response.model_dump(mode='json')
        return {'response':value,'requested_model':cfg['model'],'returned_model':value.get('model'),'expected_returned_models':cfg.get('expected_returned_models',[cfg['model'],cfg['model'].split('/',1)[-1]]),'usage':value.get('usage'),'cache_evidence':'provider-reported-only','route':cfg.get('route_id',cfg['model']),'admission':{'input_tokens':count,'output_budget':output,'tokenizer_model':tokenizer_model,'tokenizer_status':cfg.get('tokenizer_status','host-selected-not-provider-verified'),'context_window':cfg['context_window'],**media_admission}}
# Native ingress adapters normalize only explicitly supported text/image/tool fields.
# Original objects remain in sidecars; mature LiteLLM handles provider translation.
def _to_chat(fmt,request):
    if fmt=='openai-responses':
        allowed={'model','input','instructions','tools','temperature','top_p','max_output_tokens','tool_choice','parallel_tool_calls'}
        if set(request)-allowed:raise ValueError('unsupported Responses control field')
        messages=[]
        if request.get('instructions'):messages.append({'role':'system','content':request['instructions']})
        items=request.get('input',[])
        if isinstance(items,str):items=[{'role':'user','content':items}]
        for item in items:
            kind=item.get('type','message')
            if kind=='function_call':
                call={'id':item['call_id'],'type':'function','function':{'name':item['name'],'arguments':item['arguments']}}
                if messages and messages[-1]['role']=='assistant' and messages[-1].get('tool_calls'):messages[-1]['tool_calls'].append(call)
                else:messages.append({'role':'assistant','content':None,'tool_calls':[call]})
            elif kind=='function_call_output':messages.append({'role':'tool','tool_call_id':item['call_id'],'content':item['output']})
            elif kind=='message':
                content=item['content']
                if isinstance(content,list):
                    content=[media.responses_to_chat(x) for x in content]
                messages.append({'role':item['role'],'content':content})
            else:raise ValueError('unsupported Responses item')
        if 'tool_choice' in request and request['tool_choice'] not in ('auto','none','required'):raise ValueError('only auto/none/required Responses tool_choice supported')
        out={k:v for k,v in request.items() if k in ('model','temperature','top_p','tool_choice','parallel_tool_calls')};out['messages']=messages
        if 'max_output_tokens' in request:out['max_tokens']=request['max_output_tokens']
        if 'tools' in request:
            if any(x.get('type')!='function' for x in request['tools']):raise ValueError('nonfunction Responses tools unsupported')
            out['tools']=[{'type':'function','function':{k:v for k,v in x.items() if k!='type'}} for x in request['tools']]
        return out
    if fmt=='anthropic-messages':
        allowed={'model','system','messages','tools','temperature','top_p','max_tokens','stop_sequences'}
        if set(request)-allowed:raise ValueError('unsupported Anthropic control field')
        messages=[]
        if request.get('system'):messages.append({'role':'system','content':request['system']})
        for message in request.get('messages',[]):
            content=message.get('content')
            if isinstance(content,str):messages.append({'role':message['role'],'content':content});continue
            texts=[];calls=[];results=[]
            for block in content:
                if block['type'] in ('text','image'):texts.append(media.anthropic_to_chat(block))
                elif block['type']=='tool_use':calls.append({'id':block['id'],'type':'function','function':{'name':block['name'],'arguments':json.dumps(block['input'],ensure_ascii=False)}})
                elif block['type']=='tool_result':results.append({'role':'tool','tool_call_id':block['tool_use_id'],'content':[media.anthropic_to_chat(x) for x in block['content']] if isinstance(block['content'],list) else block['content']})
                else:raise ValueError('unsupported Anthropic nontext/thinking block')
            messages.extend(results)
            if texts or calls:
                value={'role':message['role'],'content':texts or None}
                if calls:value['tool_calls']=calls
                messages.append(value)
        out={k:v for k,v in request.items() if k in ('model','temperature','top_p','max_tokens')};out['messages']=messages
        if 'stop_sequences' in request:out['stop']=request['stop_sequences']
        if 'tools' in request:out['tools']=[{'type':'function','function':{'name':t['name'],'description':t.get('description',''),'parameters':t['input_schema']}} for t in request['tools']]
        return out
    raise ValueError('unsupported ingress API format')

def _from_chat(fmt,value):
    if fmt=='openai-chat':return value
    if fmt=='openai-responses':
        if set(value)-{'messages','tools','temperature','top_p','tool_choice','parallel_tool_calls','max_tokens','max_completion_tokens'}:raise ValueError('Chat control cannot be represented in supported Responses projection')
        if 'tool_choice' in value and value['tool_choice'] not in ('auto','none','required'):raise ValueError('named tool_choice projection unsupported')
        out={k:v for k,v in value.items() if k in ('temperature','top_p','tool_choice','parallel_tool_calls')};items=[]
        for message in value['messages']:
            if message['role']=='tool':
                if isinstance(message.get('content'),list):raise ValueError('Responses function output cannot preserve native image blocks in this carrier')
                items.append({'type':'function_call_output','call_id':message['tool_call_id'],'output':message['content']})
            else:
                if message.get('content') is not None:
                    content=message['content']
                    if isinstance(content,list):content=[media.chat_to_responses(x,message['role']) for x in content]
                    items.append({'type':'message','role':message['role'],'content':content})
                for call in message.get('tool_calls') or []:items.append({'type':'function_call','call_id':call['id'],**call['function']})
        out['input']=items
        if 'max_tokens' in value or 'max_completion_tokens' in value:out['max_output_tokens']=value.get('max_completion_tokens',value.get('max_tokens'))
        if 'tools' in value:out['tools']=[{'type':'function',**t['function']} for t in value['tools']]
        return out
    if fmt=='anthropic-messages':
        if set(value)-{'messages','tools','temperature','top_p','max_tokens','stop'}:raise ValueError('Chat control cannot be represented in supported Anthropic projection')
        out={k:v for k,v in value.items() if k in ('temperature','top_p','max_tokens')};messages=[];system=[]
        for m in value['messages']:
            if m['role'] in ('system','developer'):
                if not isinstance(m['content'],str):raise ValueError('Anthropic protected block projection needs text system prompt')
                system.append(m['content']);continue
            if m['role']=='tool':messages.append({'role':'user','content':[{'type':'tool_result','tool_use_id':m['tool_call_id'],'content':[media.chat_to_anthropic(x) for x in m['content']] if isinstance(m.get('content'),list) else m['content']}]});continue
            content=m.get('content');blocks=([{'type':'text','text':content}] if isinstance(content,str) and content else [media.chat_to_anthropic(block) for block in (content or [])])
            for call in m.get('tool_calls') or []:blocks.append({'type':'tool_use','id':call['id'],'name':call['function']['name'],'input':json.loads(call['function']['arguments'])})
            messages.append({'role':m['role'],'content':blocks})
        out['messages']=messages
        if system:out['system']='\n\n'.join(system)
        if 'tools' in value:out['tools']=[{'name':t['function']['name'],'description':t['function'].get('description',''),'input_schema':t['function'].get('parameters',{})} for t in value['tools']]
        if 'stop' in value:out['stop_sequences']=value['stop'] if isinstance(value['stop'],list) else [value['stop']]
        return out
    raise ValueError('unsupported projection API format')
