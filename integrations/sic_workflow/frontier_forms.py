"""Typed runtime comparison mapping and read-only presentation shared by both GUIs."""
import json


def frontier_payload(form):
    payload={key:form[key] for key in ('job_id','expected_cut','position_budget','length_budget')}
    role=form.get('comparison_role','worker')
    if role not in ('worker','editor'):raise ValueError('Comparison role must be worker or editor')
    if role=='editor':
        editor=form.get('frontier_editor','').strip();source=form.get('next_source_id','').strip()
        if not editor or not source:raise ValueError('Editor comparison requires an Editor identity and next WB source')
        payload.update(role='editor:'+editor,next_source_id=source)
        units=form.get('next_unit_ids')
        if units is not None:
            if not isinstance(units,list) or any(not isinstance(unit,str) or not unit.strip() for unit in units) or len(units)!=len(set(units)):
                raise ValueError('Next WB scope must contain unique nonempty unit IDs')
            payload['next_unit_ids']=list(units)
    return payload


def frontier_rows(result):
    if 'necessary_request' not in result:return []
    text=lambda value:json.dumps(value,ensure_ascii=False,sort_keys=True)
    rows=[{'对照':label,'角色':result.get('role','worker'),'相同cut':result.get('same_cut'),'只读请求':text(result.get(key))}
          for key,label in [('previous_request','当前角色布局（必要替换前）'),('necessary_request','仅必要KEEP替换'),('additional_request','同次候选附加编辑')]]
    keys=('edited_level','edited_context','request_affected_by_candidate','comparison_ids','structured_frontier_message_index','necessary_rebuild_prefix_bytes','canonical_common_prefix_bytes','stable_segment_digest','earlier_prefix_equal','necessary_slot_message_index','additional_slot_message_index','position_delta_messages','position_budget','position_within_budget','length_delta_bytes','length_budget_bytes','controls_equal','control_change_against_previous','teaching_change_against_previous','necessary_rebuild_may_start_before_messages','within_local_window','physical_cache_hit','provider_internal_layout','simulation')
    rows.extend({'对照':key,'结果':text(result.get(key,'unknown'))} for key in keys)
    rows.append({'对照':'证据边界','结果':'受控ID只标识回滚演算对象；旧布局不是另一历史请求组成的对照臂。局部窗口不证明切阶免费、物理命中、总成本或质量。'})
    return rows
