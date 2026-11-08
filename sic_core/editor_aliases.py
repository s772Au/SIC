"""Frozen per-assignment target aliases; exact translation, never text repair.

This optional pure seam does not change edit.submit or any Core endpoint. The
host persists the map and raw delivery, then validates translated canonical ops
with normal Core scope/operation/lifecycle rules. A map is not authorization.
"""
import copy
import hashlib
import json
from dataclasses import dataclass

VERSION = 'sic-editor-unit-aliases-v1'


def _scope(values):
    if not isinstance(values,(list,tuple)):
        raise ValueError('ordered unit scope required')
    try:units=tuple(values)
    except TypeError:raise ValueError('ordered unit scope required') from None
    if any(not isinstance(value,str) or not value for value in units):
        raise ValueError('canonical unit IDs must be nonempty strings')
    if len(units)!=len(set(units)):
        raise ValueError('duplicate canonical unit in assignment scope')
    return units


@dataclass(frozen=True,slots=True)
class FrozenAliasMap:
    job_id: str
    editor: str
    unit_ids: tuple

    def __post_init__(self):
        if not isinstance(self.job_id,str) or not self.job_id or not isinstance(self.editor,str) or not self.editor:
            raise ValueError('frozen job and Editor identity required')
        object.__setattr__(self,'unit_ids',_scope(self.unit_ids))

    def _body(self):
        return {'version':VERSION,'job_id':self.job_id,'editor':self.editor,'mapping':[{'alias':'e'+str(i+1),'unit_id':uid} for i,uid in enumerate(self.unit_ids)]}

    @property
    def map_id(self):
        body=json.dumps(self._body(),ensure_ascii=False,sort_keys=True,separators=(',',':'),allow_nan=False)
        return hashlib.sha256(body.encode('utf-8')).hexdigest()

    def as_dict(self):
        return {**self._body(),'map_id':self.map_id}

    @classmethod
    def from_dict(cls,value):
        if not isinstance(value,dict) or set(value)!={'version','job_id','editor','mapping','map_id'} or value['version']!=VERSION or not isinstance(value['mapping'],list):
            raise ValueError('invalid frozen alias map')
        rows=value['mapping']
        if any(not isinstance(row,dict) or set(row)!={'alias','unit_id'} or row['alias']!='e'+str(i+1) for i,row in enumerate(rows)):
            raise ValueError('noncanonical alias sequence')
        result=cls(value['job_id'],value['editor'],tuple(row['unit_id'] for row in rows))
        if value['map_id']!=result.map_id:raise ValueError('frozen alias map digest mismatch')
        return result


def freeze_aliases(job_id,editor,unit_ids):
    """Scope order determines e1, e2...; duplicates fail before any projection."""
    return FrozenAliasMap(job_id,editor,_scope(unit_ids))


def alias_materials(frozen,editable_units,readonly_provenance):
    """Copy only assigned rows, changing top-level unit_id in these two lists.

    Canonical source IDs, nested anchors, content and all arbitrary text remain
    untouched. Background material must not be passed as editable units. All
    assigned editable rows must be present once; provenance may be a subset.
    """
    if not isinstance(frozen,FrozenAliasMap):raise ValueError('frozen alias map required')
    by_id={uid:'e'+str(i+1) for i,uid in enumerate(frozen.unit_ids)}
    def project(rows,complete):
        if not isinstance(rows,(list,tuple)):raise ValueError('unit/provenance rows must be ordered lists')
        result=copy.deepcopy(list(rows));seen=set()
        for row in result:
            if not isinstance(row,dict) or not isinstance(row.get('unit_id'),str) or row['unit_id'] not in by_id:raise ValueError('unit row outside frozen assignment scope')
            uid=row['unit_id']
            if uid in seen:raise ValueError('duplicate unit row')
            seen.add(uid);row['unit_id']=by_id[uid]
        if complete and seen!=set(frozen.unit_ids):raise ValueError('editable unit rows differ from frozen assignment scope')
        return result
    return {'editable_units':project(editable_units,True),'readonly_provenance':project(readonly_provenance,False)}


def canonical_response(frozen,raw_response,*,job_id,editor):
    """Translate target fields only, using host-bound job/Editor, not reply claims.

    Canonical IDs are NOT a fallback input language. Unknown aliases, including
    close spellings, fail. The original parsed response is never modified.
    Content/reason/PVI/other strings are neither searched nor replaced. Core
    validation of operation fields, content and PVI remains mandatory after this
    translation; it is not performed or weakened here.
    """
    if not isinstance(frozen,FrozenAliasMap) or job_id!=frozen.job_id or editor!=frozen.editor:raise ValueError('alias map does not belong to current job/Editor')
    if not isinstance(raw_response,dict) or not isinstance(raw_response.get('ops'),list):raise ValueError('parsed response with ordered ops required')
    result=copy.deepcopy(raw_response);by_alias={'e'+str(i+1):uid for i,uid in enumerate(frozen.unit_ids)}
    def resolve(alias):
        if not isinstance(alias,str) or alias not in by_alias:raise ValueError('unknown alias or target outside frozen assignment scope')
        return by_alias[alias]
    for op in result['ops']:
        if not isinstance(op,dict):raise ValueError('operation object required')
        if op.get('op')=='ADD':
            if 'target' in op:raise ValueError('ADD must omit target')
            if 'anchor' in op:
                if not isinstance(op['anchor'],dict) or 'target' not in op['anchor']:raise ValueError('ADD anchor target required')
                op['anchor']['target']=resolve(op['anchor']['target'])
        else:
            op['target']=resolve(op.get('target'))
            if 'anchor' in op:raise ValueError('only ADD may have anchor')
    return result
