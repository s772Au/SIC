"""Versioned, provider-aware accounting. No network, credentials or fixed prices.

Quantities and money are Decimal-compatible inputs; money leaves as decimal text.
Provider usage meanings are from the primary API references:
https://developers.openai.com/api/reference/resources/chat/subresources/completions/methods/create
https://platform.claude.com/docs/en/build-with-claude/prompt-caching
Input/cache and output/reasoning totals are never charged alongside their parts.
"""
import copy
import math
from decimal import Decimal, InvalidOperation

VERSION = 'sic-accounting-v1'
INPUT_PARTS = {'input_uncached', 'cache_read', 'cache_write', 'cache_write_5m', 'cache_write_1h'}
INPUT_TOTALS = {'input_total', 'input_cache_split_unknown'}
OUTPUT_PARTS = {'output_nonreasoning', 'output_reasoning'}
OUTPUT_TOTALS = {'output_total', 'output_unsplit'}


def number(value, name='quantity', *, signed=False):
    if isinstance(value, bool) or value is None:
        raise ValueError(f'{name} must be a finite decimal')
    try:
        result = Decimal(str(value))
    except (InvalidOperation, ValueError, TypeError) as error:
        raise ValueError(f'{name} must be a finite decimal') from error
    if not result.is_finite() or (not signed and result < 0):
        raise ValueError(f'{name} must be finite and nonnegative')
    return result


def decimal_text(value):
    return format(value, 'f')


def validate_categories(categories):
    if not isinstance(categories, list):
        raise ValueError('categories must be an array')
    names = set()
    for item in categories:
        if not isinstance(item, dict) or not isinstance(item.get('category'), str):
            raise ValueError('category identity required')
        name = item['category']
        if name in names:
            raise ValueError('duplicate billing category')
        names.add(name)
        number(item.get('quantity'))
        if item.get('unit_price') is not None:
            number(item['unit_price'], 'unit_price')
    if len(names & INPUT_TOTALS) > 1 or (names & INPUT_TOTALS and names & INPUT_PARTS):
        raise ValueError('overlapping input aggregate and split categories')
    if len(names & OUTPUT_TOTALS) > 1 or (names & OUTPUT_TOTALS and names & OUTPUT_PARTS):
        raise ValueError('overlapping output aggregate and split categories')
    if 'cache_write' in names and names & {'cache_write_5m', 'cache_write_1h'}:
        raise ValueError('overlapping cache-write total and TTL parts')
    return copy.deepcopy(categories)


def normalize_usage(raw_usage, provider_format='openai-chat', policy=None):
    """Normalize supported text usage; preserve raw fields and explicit unknowns.

    policy may declare input_billing='unified' for a provider whose input is
    charged at one rate. The default preserves an unknown cache split rather
    than inventing zero hits. Missing input/output quantities remain unknown.
    """
    if raw_usage is None:
        raw_usage = {}
    if not isinstance(raw_usage, dict):
        raise ValueError('raw_usage must be an object or null')
    policy = policy or {}
    categories, unknown = [], []

    def quantity(container, field):
        value = container.get(field)
        if value is None:
            return None
        result = number(value, field)
        if result != result.to_integral_value():
            raise ValueError(f'{field} must be an integer token count')
        return int(result)

    def add(category, value):
        if value is not None:
            categories.append({'category': category, 'quantity': value, 'unit': 'token'})

    if provider_format in ('openai-chat', 'openai-responses'):
        is_responses = provider_format == 'openai-responses'
        ikey, okey = ('input_tokens', 'output_tokens') if is_responses else ('prompt_tokens', 'completion_tokens')
        idetail = raw_usage.get('input_tokens_details' if is_responses else 'prompt_tokens_details') or {}
        odetail = raw_usage.get('output_tokens_details' if is_responses else 'completion_tokens_details') or {}
        if not isinstance(idetail, dict) or not isinstance(odetail, dict):
            raise ValueError('usage detail must be an object')
        total, output = quantity(raw_usage, ikey), quantity(raw_usage, okey)
        cached, written = quantity(idetail, 'cached_tokens'), quantity(idetail, 'cache_write_tokens')
        if total is None:
            unknown.append('input_usage')
        elif policy.get('input_billing') == 'unified':
            add('input_total', total)
        elif cached is None:
            add('input_cache_split_unknown', total)
            unknown.append('input_cache_split')
        else:
            # A missing cache-write detail is not a claim of a paid write tier.
            written = written or 0
            if cached + written > total:
                raise ValueError('cache input exceeds reported total input')
            add('input_uncached', total - cached - written)
            add('cache_read', cached)
            if 'cache_write_tokens' in idetail:
                add('cache_write', written)
        reasoning = quantity(odetail, 'reasoning_tokens')
        if output is None:
            unknown.append('output_usage')
        elif reasoning is None:
            add('output_unsplit', output)
        else:
            if reasoning > output:
                raise ValueError('reasoning output exceeds reported total output')
            add('output_nonreasoning', output - reasoning)
            add('output_reasoning', reasoning)
        if raw_usage.get('total_tokens') is not None and total is not None and output is not None:
            if quantity(raw_usage, 'total_tokens') != total + output:
                raise ValueError('usage total does not reconcile')
        for detail in (idetail, odetail):
            if quantity(detail, 'audio_tokens') not in (None, 0):
                unknown.append('unsupported_audio_billing_split')
    elif provider_format == 'anthropic-messages':
        # Anthropic input_tokens excludes the two cache categories.
        ordinary = quantity(raw_usage, 'input_tokens')
        read = quantity(raw_usage, 'cache_read_input_tokens')
        written = quantity(raw_usage, 'cache_creation_input_tokens')
        output = quantity(raw_usage, 'output_tokens')
        if ordinary is None:
            unknown.append('input_usage')
        add('input_uncached', ordinary)
        add('cache_read', read)
        ttl = raw_usage.get('cache_creation') or {}
        if not isinstance(ttl, dict):
            raise ValueError('cache_creation detail must be an object')
        five, hour = quantity(ttl, 'ephemeral_5m_input_tokens'), quantity(ttl, 'ephemeral_1h_input_tokens')
        if five is not None or hour is not None:
            if written is None or (five or 0) + (hour or 0) != written:
                raise ValueError('cache-write TTL parts do not reconcile')
            add('cache_write_5m', five or 0)
            add('cache_write_1h', hour or 0)
        else:
            add('cache_write', written)
        if read is None or written is None:
            unknown.append('cache_usage')
        if output is None:
            unknown.append('output_usage')
        add('output_unsplit', output)
    else:
        raise ValueError('unsupported provider usage format; supply an explicit mapping')
    validate_categories(categories)
    return {'normalization_version': VERSION, 'provider_format': provider_format,
            'raw_usage': copy.deepcopy(raw_usage), 'categories': categories,
            'usage_status': 'unknown' if not categories else ('partial' if unknown else 'provider_reported'),
            'unknown_components': unknown, 'policy': copy.deepcopy(policy)}


def _interval(value):
    if not isinstance(value, (list, tuple)) or len(value) != 2:
        raise ValueError('interval must be [start,end] on one declared clock')
    a, b = value
    if any(isinstance(x, bool) or not isinstance(x, (int, float)) or not math.isfinite(x) for x in (a, b)) or b < a:
        raise ValueError('invalid time interval')
    return float(a), float(b)


def _union(intervals):
    merged = []
    for a, b in sorted(_interval(x) for x in intervals):
        if merged and a <= merged[-1][1]:
            merged[-1] = (merged[-1][0], max(merged[-1][1], b))
        else:
            merged.append((a, b))
    return merged


def _length(intervals):
    return sum(b - a for a, b in _union(intervals))


def timing_summary(calls=(), events=(), task_interval=None):
    """Union/intersect real intervals; never sum parallel calls as wall time."""
    groups = {'model': [], 'editor': [], 'worker': [], 'tool': [], 'serial_wait': [], 'interleave': []}
    missing = []
    cumulative = 0.0
    for call in calls:
        if call.get('started_at') is not None and call.get('ended_at') is not None:
            interval = _interval([call['started_at'], call['ended_at']])
            groups['model'].append(interval)
            role = call.get('role', '')
            if role == 'worker':
                groups['worker'].append(interval)
            elif role == 'editor' or role.startswith(('editor-', 'editor:')):
                groups['editor'].append(interval)
            cumulative += interval[1] - interval[0]
        elif call.get('call_duration_seconds') is not None:
            cumulative += float(number(call['call_duration_seconds'], 'call duration'))
            missing.append({'call_id': call.get('call_id'), 'reason': 'interval_missing'})
        else:
            missing.append({'call_id': call.get('call_id'), 'reason': 'duration_missing'})
    for event in events:
        kind = event.get('kind')
        if kind not in ('tool', 'serial_wait', 'interleave'):
            raise ValueError('event interval kind must be tool/serial_wait/interleave')
        groups[kind].append(_interval([event['start'], event['end']]))
    task = _interval(task_interval) if task_interval is not None else None
    if task:
        for intervals in groups.values():
            if any(a < task[0] or b > task[1] for a, b in intervals):
                raise ValueError('event lies outside declared task interval')
    overlap = sum(max(0.0, min(b, d) - max(a, c))
                  for a, b in _union(groups['editor']) for c, d in _union(groups['tool']))
    return {'task_wall_seconds': task[1] - task[0] if task else None,
            'model_cumulative_seconds': cumulative,
            'model_union_seconds': _length(groups['model']) if not missing else None,
            'editor_tool_overlap_seconds': overlap if not missing else None,
            'tool_union_seconds': _length(groups['tool']),
            'serial_wait_seconds': _length(groups['serial_wait']),
            'interleave_interval_seconds': _length(groups['interleave']),
            'unknown': missing + ([] if task else [{'reason': 'task_interval_missing'}])}


def _price_for(item, record, prices):
    if item.get('unit_price') is not None:
        rate = copy.deepcopy(item)
        for key in ('currency', 'price_version', 'price_basis', 'per_quantity'):
            rate.setdefault(key, record.get(key))
        return rate
    choices = []
    for rate in prices:
        if rate.get('category') != item['category']:
            continue
        if rate.get('model') not in (None, record.get('model')) or rate.get('role') not in (None, record.get('role')):
            continue
        if record.get('price_version') and rate.get('price_version') != record['price_version']:
            continue
        if rate.get('unit', item.get('unit', 'token')) != item.get('unit', 'token'):
            continue
        choices.append(rate)
    if len(choices) > 1:
        raise ValueError('ambiguous price binding; freeze model/role/price_version')
    return choices[0] if choices else None


def summarize_records(calls, resource_records=(), task_interval=None, prices=(), events=(),
                      required_resources=('program', 'storage')):
    """Price a complete ledger while exposing unpriced/missing measurements.

    Price rows: category, unit_price, per_quantity (default 1), unit, currency,
    price_version, optional model/role. Resource records additionally specify
    resource='program'/'storage'/an explicit paid service, measurement_method.
    """
    rows, resource_rows, unknown = [], [], []
    totals = {}; subtotals = {'llm': {}, 'program': {}, 'storage': {}, 'other': {}}
    seen = set()
    for record in calls:
        identity = record.get('call_id')
        if not isinstance(identity, str) or not identity or identity in seen:
            raise ValueError('each actual call requires a unique call_id')
        seen.add(identity)
        categories = validate_categories(record.get('categories', []))
        if not categories or record.get('usage_status') in ('unknown', 'partial'):
            unknown.append({'call_id': identity, 'reason': 'usage_incomplete', 'components': record.get('unknown_components', [])})
        for item in categories:
            rows.append(_priced_row(item, record, prices, 'llm', unknown, totals, subtotals))
    measured = set()
    resource_ids = set()
    for record in resource_records:
        rid = record.get('record_id')
        if not isinstance(rid, str) or not rid or rid in resource_ids:
            raise ValueError('each resource measurement requires a unique record_id')
        resource_ids.add(rid)
        resource = record.get('resource')
        if not isinstance(resource, str) or not record.get('measurement_method'):
            raise ValueError('resource and measurement_method required')
        if record.get('quantity') is None:
            unknown.append({'record_id': rid, 'resource': resource, 'reason': 'measurement_missing'})
            continue
        measured.add(resource)
        item = {**record, 'category': record.get('category', resource)}
        resource_rows.append(_priced_row(item, record, prices, resource if resource in ('program', 'storage') else 'other', unknown, totals, subtotals))
    for resource in required_resources:
        if resource not in measured:
            unknown.append({'resource': resource, 'reason': 'resource_measurement_missing'})
    currency = next(iter(totals)) if len(totals) == 1 else None
    if len(totals) > 1:
        unknown.append({'reason': 'multiple_currencies_no_conversion', 'currencies': sorted(totals)})
    complete = not unknown
    return {'schema': VERSION, 'rows': rows, 'resource_rows': resource_rows,
            'totals_by_currency': {k: decimal_text(v) for k, v in totals.items()},
            'known_llm_cost_by_currency': {k: decimal_text(v) for k, v in subtotals['llm'].items()},
            'subtotals_by_currency': {group: {k: decimal_text(v) for k, v in values.items()} for group, values in subtotals.items()},
            'total_cost': decimal_text(totals[currency]) if complete and currency else None,
            'currency': currency, 'complete_cost': complete, 'unknown': unknown,
            'call_count': len(calls), 'timing': timing_summary(calls, events, task_interval)}


def _priced_row(item, record, prices, group, unknown, totals, subtotals):
    quantity = number(item.get('quantity'))
    rate = _price_for(item, record, prices)
    row = {'call_id': record.get('call_id'), 'record_id': record.get('record_id'),
           'role': record.get('role'), 'model': record.get('model'), 'resource': record.get('resource'),
           'category': item['category'], 'quantity': decimal_text(quantity), 'unit': item.get('unit', 'token'),
           'raw_usage': copy.deepcopy(record.get('raw_usage')), 'cost': None,
           'source_refs': copy.deepcopy(record.get('source_refs', {}))}
    if not rate or rate.get('unit_price') is None or not rate.get('currency') or not rate.get('price_version'):
        unknown.append({'call_id': record.get('call_id'), 'record_id': record.get('record_id'), 'category': item['category'], 'reason': 'price_binding_missing'})
        return row
    per = number(1 if rate.get('per_quantity') is None else rate['per_quantity'], 'price denominator')
    if not per:
        raise ValueError('price denominator must be positive')
    cost = quantity * number(rate['unit_price'], 'unit price') / per
    currency = rate['currency']
    totals[currency] = totals.get(currency, Decimal(0)) + cost
    subtotals[group][currency] = subtotals[group].get(currency, Decimal(0)) + cost
    row.update(cost=decimal_text(cost), currency=currency, unit_price=decimal_text(number(rate['unit_price'])),
               per_quantity=decimal_text(per), price_version=rate['price_version'], price_basis=rate.get('price_basis', 'host-declared'))
    return row


aggregate_ledger = summarize_records


def representation_metrics(*, wb_body, cb_body, persistent_operations, presented_index,
                           editor_billed_output, count, counter_id, tokenizer_model,
                           declared_source=None):
    """Count separate bodies and the actual presented background on one counter.

    count is a caller-supplied tokenizer/counting function. It must count the
    same serialization passed here; no byte estimate is presented as tokens.
    """
    if not counter_id or not tokenizer_model:
        raise ValueError('counter identity and tokenizer/model label required')
    def counted(text):
        if not isinstance(text, str):
            raise ValueError('counting inputs must be the actual serialized text')
        return number(count(text), 'counted length')
    wb, cb = counted(wb_body), counted(cb_body)
    op, index = counted(persistent_operations), counted(presented_index)
    source = counted(wb_body if declared_source is None else declared_source)
    billed = None if editor_billed_output is None else number(editor_billed_output, 'billed output')
    # c_i must count the combined serialization, since tokenization is not additive.
    background = counted(cb_body + persistent_operations + presented_index)
    return {'counter_id': counter_id, 'tokenizer_model': tokenizer_model,
            'wb_body_count': decimal_text(wb), 'cb_body_count': decimal_text(cb),
            'persistent_operations_count': decimal_text(op), 'presented_index_count': decimal_text(index),
            'background_count': decimal_text(background), 'editor_billed_output_count': decimal_text(billed) if billed is not None else None,
            'rho_content': decimal_text(cb / wb) if wb else None,
            'rho_billed': decimal_text(billed / source) if source and billed is not None else None,
            'rho_content_definition': 'CB visible body / WB body on declared counter',
            'rho_billed_definition': 'Editor billed output / declared source on declared counter',
            'background_definition': 'actual concatenation of CB body + persistent operations + presented index'}


def controlled_cost(terms):
    """Evaluate the paper's declared-assumption model, not a cache-hit claim."""
    required = ('same_source_read_once', 'same_editor_model', 'same_source_input_pricing',
                'first_background_uncached', 'later_background_cached')
    assumptions = terms.get('assumptions', {})
    missing = [key for key in required if assumptions.get(key) is not True]
    if missing:
        return {'applicable': False, 'reason': 'control_conditions_not_established', 'conditions_missing': missing}
    sizes = [number(x, 'background count') for x in terms['background_counts']]
    n = len(sizes)
    first = sum(sizes[:-1], Decimal(0))
    hit = sum((sizes[i] * (n - i - 2) for i in range(max(0, n - 2))), Decimal(0))
    u, h, o = [number(terms[k], k) for k in ('p_u', 'p_h', 'p_o')]
    os, ob = [number(terms[k], k) for k in ('O_S', 'O_B')]
    sw = number(terms['S_W'], 'S_W', signed=True)
    remainder = number(terms['D'], 'D', signed=True)
    delta = u * first + h * hit + o * (os - ob) - sw + remainder
    return {'applicable': True, 'assumptions': copy.deepcopy(assumptions), 'N': n,
            'F': decimal_text(first), 'H': decimal_text(hit), 'background_read_total': decimal_text(first + hit),
            'delta_cost': decimal_text(delta), 'currency': terms.get('currency'),
            'S_W': decimal_text(sw), 'D': decimal_text(remainder),
            'cache_status': terms.get('cache_status', 'declared_assumption_not_provider_verified'),
            'meaning': 'conditional algebra; not an observed parity or task-quality result'}


def compare_workflows(baseline, sic, controlled_terms=None):
    """Compare priced ledgers. Quality is populated only by a separate scorer."""
    delta = None
    reason = None
    if not baseline.get('complete_cost') or not sic.get('complete_cost'):
        reason = 'incomplete_total_cost'
    elif not baseline.get('currency') or baseline.get('currency') != sic.get('currency'):
        reason = 'currency_mismatch'
    else:
        delta = decimal_text(number(sic['total_cost']) - number(baseline['total_cost']))
    bt, st = baseline.get('timing', {}).get('task_wall_seconds'), sic.get('timing', {}).get('task_wall_seconds')
    control = controlled_cost(controlled_terms) if controlled_terms is not None else {'applicable': False, 'reason': 'not_requested'}
    if control.get('applicable') and delta is not None:
        control['ledger_residual'] = decimal_text(number(delta, signed=True) - number(control['delta_cost'], signed=True))
        control['reconciled_with_ledger'] = control['ledger_residual'] == '0'
    return {'schema': VERSION, 'baseline': baseline, 'sic': sic, 'delta_cost': delta,
            'delta_cost_reason': reason, 'currency': baseline.get('currency') if delta is not None else None,
            'delta_wall_seconds': st - bt if bt is not None and st is not None else None,
            'quality': {'status': 'not_scored', 'reason': 'independent task scorer required'},
            'controlled_comparison': control}
