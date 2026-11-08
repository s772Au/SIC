"""Semantic hierarchy forms shared by official Langflow and Studio."""
import json
from typing import Literal
from pydantic import BaseModel, Field


class HierarchyForm(BaseModel):
    action: Literal['overview', 'wb_timeline', 'cb_timeline', 'form', 'expand_cb', 'expand_wb', 'select', 'maintain', 'create_context', 'ingest_context', 'legacy', 'replay'] = 'overview'
    context_id: str = 'main'
    level: int = Field(default=1, ge=1)
    cb_ids: list[str] = Field(default_factory=list, description='Complete consecutive lower-level members in their recorded order')
    object_id: str = ''
    roles: list[str] = Field(default_factory=lambda: ['worker', 'editor'], description='worker, editor or specific editor:alpha/editor:beta')
    shared_contexts: list[str] = Field(default_factory=lambda: ['main'])
    working_text: str = ''
    fanout: int = Field(default=2, ge=2)
    rules: list[Literal['deduplicate_exact', 'drop_empty', 'normalize_newlines']] = Field(default_factory=lambda: ['deduplicate_exact'])
    offset: int = Field(default=0, ge=0)
    limit: int = Field(default=20, ge=1, le=100)
    step: int = Field(default=-1, ge=-1, description='-1 means complete operation prefix; 0 is original retained content')
    allow_write: bool = False
    allow_model: bool = False


def hierarchy_request(form):
    f = HierarchyForm.model_validate(form).model_dump()
    a, ctx = f['action'], {'context_id': f['context_id']}
    if a == 'overview': return 'hierarchy.get', {}
    if a in {'wb_timeline', 'cb_timeline'}:
        return 'hierarchy.timeline', {**ctx, 'level': f['level'], 'kind': 'wb' if a == 'wb_timeline' else 'cb', 'offset': f['offset'], 'limit': f['limit']}
    if a == 'form': return 'hierarchy.form', {**ctx, 'level': f['level'], 'cb_ids': f['cb_ids']}
    if a in {'expand_cb', 'expand_wb'}:
        return 'hierarchy.expand', {('cb_id' if a == 'expand_cb' else 'wb_id'): f['object_id']}
    if a == 'select': return 'hierarchy.select', {**ctx, 'level': f['level'], 'roles': f['roles']}
    if a == 'maintain': return 'hierarchy.maintain', {**ctx, 'fanout': f['fanout'], 'rules': f['rules'], 'roles': f['roles']}
    if a == 'create_context': return 'hierarchy.context.create', {**ctx, 'shared_contexts': f['shared_contexts']}
    if a == 'ingest_context':
        return 'hierarchy.context.ingest', {**ctx, 'request': {'messages': [{'role': 'user', 'content': f['working_text']}]}}
    if a == 'legacy': return 'hierarchy.legacy', {'offset': f['offset'], 'limit': f['limit']}
    return 'memory.replay', {'cb_id': f['object_id'], **({'step': f['step']} if f['step'] >= 0 else {})}


def compact(value): return json.dumps(value, ensure_ascii=False, sort_keys=True)


def hierarchy_rows(body):
    rows = []
    if 'levels' in body:
        contexts = {'main': {'levels': body['levels'], 'active': body.get('active', {}), 'shared_contexts': []}, **body.get('contexts', {})}
        for cid, ctx in contexts.items():
            for level, records in ctx.get('levels', {}).items():
                rows.append({'内部上下文': cid, '阶次': level, 'WB形成数量': len(records.get('wbs', [])), 'CB发布数量': len(records.get('cbs', [])),
                             '同阶对应': compact(records.get('pairs', {})), '待编辑': compact(records.get('pending', [])),
                             '共享引用': compact(ctx.get('shared_contexts', [])), '默认活动选择': compact(ctx.get('active', {}))})
        for role, selection in body.get('role_views', {}).items():
            rows.append({'内部上下文': selection.get('context_id'), '阶次': selection.get('level'), '角色独立选择': role})
    for row in body.get('rows', []):
        legacy = row.get('hierarchy_index_status') == 'legacy-unindexed'
        rows.append({'内部上下文': row.get('context_id', 'main'), '阶次': None if legacy else row.get('level', body.get('level')),
                     '链类型':body.get('kind','legacy-cb' if legacy else 'unknown'),
                     'WB': row.get('wb_id'), 'CB': row.get('cb_id'), '记录': row.get('id'),
                     '形成位置': 'unknown' if legacy else row.get('membership_index'),
                     'WB形成顺序': row.get('wb_formation_order', 'recorded'),
                     '已保存CB顺序': compact(row.get('known_cb_order', row.get('saved_cb_sequence', {'membership_index':row.get('membership_index'),'predecessor':row.get('predecessor'),'successor':row.get('successor')} if body.get('kind')=='cb' else None))),
                     '前一块': row.get('predecessor'), '后一块': row.get('successor'),
                     '下阶有序成员': compact(row.get('members', [])), '索引状态': row.get('hierarchy_index_status', 'indexed')})
    if 'wb_id' in body:
        rows.append({'内部上下文': body.get('context_id'), '阶次': body.get('level'), 'WB': body['wb_id'], 'CB': body.get('cb_id'),
                     'WB形成顺序': body.get('wb_formation_order', 'recorded'), '索引状态': body.get('hierarchy_index_status', 'indexed'),
                     '已保存CB顺序': compact(body.get('known_cb_order')), '完整读取': compact(body.get('complete_reads', body.get('complete_read', {}))),
                     '发布补充来源（非Editor已见）': compact(body.get('publication_supplement_sources', []))})
    for i, member in enumerate(body.get('members', [])):
        rows.append({'成员序号': i, '下阶有序成员': compact(member), '只读': True})
    for unit in body.get('original_units', body.get('units', [])):
        rows.append({'原单元': unit['unit_id'], '归档正文（只读）': unit['text'], '来源锚': compact(unit.get('anchor', {}))})
    if 'step' in body:
        rows.append({'回放步': body['step'], '操作总数': body.get('operation_count'), '回放正文': body.get('body'),
                     '与固定正文一致': body.get('body_matches_published_cb'), '外部副作用重放': body.get('external_effects_replayed'),
                     '完整cut/I已重建': False, '说明': '仅正文op前缀回放；不冒称完整PVI-only视图恢复'})
    if body.get('legacy_unindexed_count') is not None:
        rows.append({'旧档案未编层级数': body['legacy_unindexed_count'], '旧WB形成顺序': 'unknown', '下页': body.get('next_offset')})
    for action in body.get('actions', []):
        rows.append({'实际程序升阶': compact(action), '说明': '已形成并编辑发布；不保证语义压缩或质量'})
    return rows or [{'结果': compact(body)}]
