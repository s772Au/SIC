"""Explicit shared-prefix config patch and safe read-only metadata presentation."""
import copy,json
from urllib.parse import urlsplit
IDENTITY_KEYS={'model','provider_id','cache_namespace'}
LAYOUT_KEYS=('prefix_layout','shared_prefix_budget','shared_prefix_identity')
META_KEYS=('requested_layout','applied_layout','shared','mode','plan_id','plan_digest','prefix_bytes_sha256','provider_request_id','alias_map_id','physical_cache_hit','editor_delivery','identity_source','reason')

def identity(value):
    if not isinstance(value,dict) or not value:raise ValueError('共享身份声明须为非空对象')
    role_map=set(value)<= {'worker','editor'}
    entries=value.values() if role_map else [value]
    for row in entries:
        if not isinstance(row,dict) or set(row)!=IDENTITY_KEYS or any(not isinstance(v,str) or not v.strip() for v in row.values()):raise ValueError('身份仅接受model/provider_id/cache_namespace三个非空字符串；不能含api/key/env字段')
        for v in row.values():
            if '://' in v:
                url=urlsplit(v)
                if url.username or url.password or url.query or url.fragment:raise ValueError('共享身份不能填URL凭据或查询参数；请使用非敏感逻辑标识')
    return copy.deepcopy(value)

def prefix_patch(form):
    if not form.get('replace_prefix_layout',False):return {}
    layout=form.get('prefix_layout','separate')
    if layout not in ('separate','shared_i'):raise ValueError('prefix_layout须为separate/shared_i')
    result={'prefix_layout':layout};budget=form.get('shared_prefix_budget')
    if budget is not None:
        if type(budget) is not int or budget<512:raise ValueError('共享投影预算须为>=512的整数')
        result['shared_prefix_budget']=budget
    mode=form.get('prefix_identity_mode','keep')
    if mode=='shared':result['shared_prefix_identity']=identity({key:form.get('shared_'+key,'') for key in IDENTITY_KEYS})
    elif mode=='per_role':result['shared_prefix_identity']=identity(form.get('shared_prefix_identity',{}))
    elif mode!='keep':raise ValueError('未知身份声明修改范围')
    return result

def parse_identity(text):
    def pairs(items):
        value={}
        for key,item in items:
            if key in value:raise ValueError('共享身份JSON不能有重复键')
            value[key]=item
        return value
    return identity(json.loads(text,object_pairs_hook=pairs))

def safe_settings(value):
    result={key:copy.deepcopy(value[key]) for key in ('prefix_layout','shared_prefix_budget') if key in value}
    if 'shared_prefix_identity' in value:
        try:result['shared_prefix_identity']=identity(value['shared_prefix_identity'])
        except ValueError:result['shared_prefix_identity']='not displayed: invalid/non-public identity declaration'
    return result

def safe_meta(value):return {key:copy.deepcopy(value[key]) for key in META_KEYS if key in value}

def safe_aliases(value):
    return {key:copy.deepcopy(value[key]) for key in ('version','job_id','editor','map_id') if key in value}|{'mapping':[{key:copy.deepcopy(row[key]) for key in ('alias','unit_id') if key in row} for row in value.get('mapping',[])]}
