"""Typed ledger scope and presentation; all arithmetic stays in the Core."""
import math
from typing import Literal
from pydantic import BaseModel,ConfigDict,Field,model_validator,field_validator

class Price(BaseModel):
    model_config=ConfigDict(extra='forbid')
    category:str
    unit:str
    unit_price:str
    currency:str
    per_quantity:str='1'
    price_version:str
    price_basis:str|None=None
    model:str|None=None
    role:str|None=None

class LedgerScope(BaseModel):
    model_config=ConfigDict(extra='forbid')
    scope:Literal['chain','task']='chain'
    additional_chain_ids:list[str]=Field(default_factory=list)
    prices:list[Price]=Field(default_factory=list)
    prices_by_chain:dict[str,list[Price]]=Field(default_factory=dict)
    task_interval:list[float]=Field(default_factory=list)

    @field_validator('task_interval',mode='before')
    @classmethod
    def interval_numbers(cls,value):
        if not isinstance(value,list) or any(isinstance(x,bool) or not isinstance(x,(int,float)) for x in value):raise ValueError('task_interval must contain numeric seconds, not booleans or aliases')
        return value

    @model_validator(mode='after')
    def check_scope(self):
        if len(self.additional_chain_ids)!=len(set(self.additional_chain_ids)) or any(not x for x in self.additional_chain_ids):raise ValueError('Supplemental route IDs must be nonempty and unique')
        if self.task_interval and (len(self.task_interval)!=2 or any(not math.isfinite(x) for x in self.task_interval) or self.task_interval[1]<self.task_interval[0]):raise ValueError('task_interval must be [] or finite [start,end]')
        if self.scope=='chain' and (self.additional_chain_ids or self.prices_by_chain):raise ValueError('Additional routes and per-route prices require task scope')
        if self.scope=='task' and self.prices:raise ValueError('Task scope uses prices_by_chain; a global price must not override every route')
        return self

    def payload(self):
        data=self.model_dump(exclude_none=True)
        payload={'scope':self.scope}
        if self.scope=='task':payload.update(additional_chain_ids=data['additional_chain_ids'],prices_by_chain=data['prices_by_chain'])
        elif self.prices:payload['prices']=data['prices']
        if self.task_interval:payload['task_interval']=data['task_interval']
        return payload

def ledger_payload(fields):
    return LedgerScope.model_validate({key:fields[key] for key in LedgerScope.model_fields if key in fields}).payload()

def ledger_tables(body):
    """Return actual Core fields, never recalculate fees or cache benefit."""
    scope=body.get('scope',{})
    scope_rows=[{'kind':'scope','field':key,'value':value} for key,value in scope.items()]
    price_rows=[]
    for chain,binding in body.get('prices_by_chain',{}).items():
        price_rows.extend({'chain_id':chain,'binding_source':binding['source'],**price} for price in binding['prices'])
        if not binding['prices']:price_rows.append({'chain_id':chain,'binding_source':binding['source'],'price_status':'unknown-no-declared-prices'})
    contexts=[{'call_id':identity,**item} for identity,items in body.get('call_contexts',{}).items() for item in items]
    timing=[]
    summaries=[(name,body[name]) for name in ('baseline','sic') if name in body] or [('current',body.get('summary',body))]
    for name,summary in summaries:
        timing.extend({'workflow':name,'timing_scope':'task-wide' if scope.get('kind')=='task' else 'chain','chain_id':'','metric':key,'value':value} for key,value in summary.get('timing',{}).items())
    for chain,values in body.get('timing_by_chain',{}).items():
        timing.extend({'workflow':'current','timing_scope':'same-chain','chain_id':chain,'metric':key,'value':value} for key,value in values.items())
    return {'scope_rows':scope_rows,'price_rows':price_rows,'context_rows':contexts,'timing_rows':timing}
