"""Immutable image carriers and text-only editing provenance.

Images are native read-only payloads. No OCR, semantic image compression, or
provider-token estimate is invented here.
"""
import base64
import copy
import hashlib
import io
import re


def image_info(block):
    if not isinstance(block,dict) or set(block)!={'type','image_url'} or block['type']!='image_url':raise ValueError('Unsupported media block; expected image_url')
    value=block['image_url']
    if not isinstance(value,dict) or set(value)-{'url','detail'}:raise ValueError('image_url requires url and optional detail')
    url=value.get('url');detail=value.get('detail','auto')
    if detail not in ('low','high','auto'):raise ValueError('Invalid image detail')
    if not isinstance(url,str):raise ValueError('Image URL must be a string')
    match=re.fullmatch(r'data:(image/(?:png|jpeg|webp|gif));base64,([A-Za-z0-9+/=\r\n]+)',url)
    if not match:raise ValueError('Image bytes must be snapshotted in a PNG/JPEG/WEBP/GIF base64 data URI; mutable remote URLs are not immutable source evidence')
    try:
        raw=base64.b64decode(match.group(2),validate=True)
        from PIL import Image
        with Image.open(io.BytesIO(raw)) as image:
            image.verify()
        with Image.open(io.BytesIO(raw)) as image:
            width,height=image.size;actual=Image.MIME.get(image.format);frames=getattr(image,'n_frames',1)
        if actual!=match.group(1):raise ValueError('Image MIME does not match actual bytes')
        if width<=0 or height<=0:raise ValueError('Image dimensions invalid')
    except Exception as error:raise ValueError('Image content cannot be verified') from error
    return {'media_id':'sha256:'+hashlib.sha256(raw).hexdigest(),'sha256':hashlib.sha256(raw).hexdigest(),'mime_type':match.group(1),'bytes':len(raw),'width':width,'height':height,'frames':frames,'detail':detail}


def descriptors(source,source_id=None):
    return [{**copy.deepcopy(item),**({'source_id':source_id} if source_id else {})} for item in source.get('media',[])]


def source_ids(core,source):
    refs=list(source.get('media_source_ids',[]));parts=[(sid,core.store.get(sid,'source')) for sid in source.get('source_ids',[])]
    # A native assistant/tool group can be split across ingest records. Keep its
    # entire closed skeleton whenever any member carries an image.
    if any(part.get('media') for sid,part in parts):refs.extend(sid for sid,part in parts)
    for sid,part in parts:refs.extend(part.get('media_source_ids',[]))
    return list(dict.fromkeys(refs))


def readonly(core,source):
    result=[]
    for sid in source_ids(core,source):result.extend(descriptors(core.store.get(sid,'source'),sid))
    return result


def cb_manifest(core,cb):
    items=[]
    for sid in cb.get('media_source_ids',[]):items.extend(descriptors(core.store.get(sid,'source'),sid))
    encoded=__import__('json').dumps(items,ensure_ascii=False,sort_keys=True,separators=(',',':')).encode()
    return {'image_count':len(items),'source_count':len(cb.get('media_source_ids',[])),'media_digest':hashlib.sha256(encoded).hexdigest(),'sample':items[:4],'sample_complete':len(items)<=4,'complete_read':{'operation':'source.read','page_kind':'media','source_ids_field':'media_source_ids'},'editing_policy':'images are immutable readonly carriers; only assigned text units are editable'}


def replay_messages(source,source_id):
    """Replay native role/tool skeleton and media in original order.

    Edited text lives in the CB; an explicit source anchor replaces every
    original text fragment here, preserving its position around the media.
    """
    messages=[]
    for mi,original in enumerate(source['sidecar']['messages']):
        if original['role'] in ('system','developer'):continue
        message=copy.deepcopy(original);content=message.get('content')
        anchor=lambda part:f'[SIC edited text in CB; source={source_id}; message={mi}; part={part}]'
        if isinstance(content,str):message['content']=anchor(None)
        elif isinstance(content,list):
            message['content']=[({'type':'text','text':anchor(pi)} if block['type']=='text' else copy.deepcopy(block)) for pi,block in enumerate(content)]
        messages.append(message)
    return messages


def project_carriers(core,cbs):
    seen=set();result=[]
    for cb in cbs:
        carriers=[]
        for sid in cb.get('media_source_ids',[]):
            if sid in seen:continue
            seen.add(sid);source=core.store.get(sid,'source')
            carriers.extend(replay_messages(source,sid))
        result.append(carriers)
    return result


def admission(litellm,cfg,request):
    images=[block for m in request['messages'] for block in (m.get('content') if isinstance(m.get('content'),list) else []) if block.get('type')=='image_url']
    if not images:return {'media_count':0,'media_counting':'not-applicable'}
    model=cfg['model'].removeprefix('openai/')
    known={'gpt-4o','gpt-4o-2024-05-13','gpt-4o-2024-08-06','gpt-4o-2024-11-20','gpt-4-turbo','gpt-4-turbo-2024-04-09'}
    if model not in known or cfg['tokenizer_model'] not in known:raise ValueError('Unknown model-specific image token accounting; multimodal admission refused')
    infos=[image_info(block) for block in images]
    if any(info['detail']=='auto' or info['frames']!=1 for info in infos):raise ValueError('Automatic image detail or animated media has unknown exact admission count; choose explicit low/high static image')
    from litellm.litellm_core_utils.token_counter import get_image_dimensions
    for block,info in zip(images,infos):
        if info['detail']=='high' and get_image_dimensions(block['image_url']['url'])!=(info['width'],info['height']):raise ValueError('LiteLLM image dimensions disagree with verified source; no fallback admission')
    # This installed LiteLLM implementation uses the documented 85 base plus
    # 170 per high-resolution tile for the explicitly supported model family.
    return {'media_count':len(infos),'media_counting':'LiteLLM token_counter, validated static image dimensions and explicit detail; OpenAI 85/170 image policy','media_ids':[i['media_id'] for i in infos],'media_detail':[i['detail'] for i in infos]}


def responses_to_chat(block):
    kind=block.get('type')
    if kind in ('input_text','output_text'):
        if set(block)!={'type','text'}:raise ValueError('Unsupported Responses text field')
        return {'type':'text','text':block['text']}
    if kind=='input_image':
        if set(block)-{'type','image_url','detail'}:raise ValueError('Unsupported Responses image field')
        result={'type':'image_url','image_url':{'url':block['image_url']}}
        if 'detail' in block:result['image_url']['detail']=block['detail']
        image_info(result);return result
    raise ValueError('Unsupported Responses content block')


def chat_to_responses(block,role):
    if block['type']=='text':return {'type':'output_text' if role=='assistant' else 'input_text','text':block['text']}
    if block['type']=='image_url':
        if role=='assistant':raise ValueError('Responses assistant image output is not a supported input carrier')
        return {'type':'input_image','image_url':block['image_url']['url'],**({'detail':block['image_url']['detail']} if 'detail' in block['image_url'] else {})}
    raise ValueError('Unsupported Chat content for Responses')


def anthropic_to_chat(block):
    if block['type']=='text':
        if set(block)!={'type','text'}:raise ValueError('Unsupported Anthropic text field')
        return {'type':'text','text':block['text']}
    if block['type']=='image':
        if set(block)!={'type','source'}:raise ValueError('Unsupported Anthropic image field')
        source=block['source']
        if set(source)!={'type','media_type','data'} or source['type']!='base64':raise ValueError('Anthropic image must contain immutable base64 source')
        result={'type':'image_url','image_url':{'url':'data:'+source['media_type']+';base64,'+source['data']}}
        image_info(result);return result
    raise ValueError('Unsupported Anthropic content block')


def chat_to_anthropic(block):
    if block['type']=='text':return copy.deepcopy(block)
    if block['type']=='image_url':
        info=image_info(block)
        if info['detail']!='auto':raise ValueError('Anthropic native image carrier cannot preserve OpenAI explicit detail; no silent conversion')
        return {'type':'image','source':{'type':'base64','media_type':info['mime_type'],'data':block['image_url']['url'].split(',',1)[1]}}
    raise ValueError('Unsupported Chat block for Anthropic')
