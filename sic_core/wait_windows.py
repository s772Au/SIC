"""Measured Worker hold intervals excluding actual tools and declared waits."""
def serial_wait_segments(events, start, end):
    tools, waits, natural = {}, {}, []
    for event in events:
        if event.get('duplicate_observation'):
            continue
        when = event.get('time', end)
        if when > end:
            continue
        kind = event.get('type')
        if kind == 'tool_start':
            tools[event['call_id']] = when
        elif kind == 'tool_end' and event['call_id'] in tools:
            natural.append((tools.pop(event['call_id']), when))
        elif kind == 'wait':
            waits[event['wait_id']] = (when, when + event['seconds'])
        elif kind in ('wait_cancel', 'wait_complete') and event['wait_id'] in waits:
            a, b = waits.pop(event['wait_id'])
            natural.append((a, min(b, when)))
    natural.extend((a, end) for a in tools.values())
    natural.extend(waits.values())
    cursor, segments = start, []
    for a, b in sorted(natural):
        a, b = max(start, a), min(end, b)
        if b <= cursor or b <= a:
            continue
        if a > cursor:
            segments.append((cursor, a))
        cursor = max(cursor, b)
    if cursor < end:
        segments.append((cursor, end))
    return segments
