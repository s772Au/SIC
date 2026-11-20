"""Select closed native Responses turns adjacent to admitted Codex material."""


def editable_parts(parts, field, text_type):
    if isinstance(parts, str):
        return [(field,)] if parts.strip() else []
    if not isinstance(parts, list):
        raise ValueError("Codex native context contains an unsupported content shape")
    if any(not isinstance(part, dict) or part.get("type") != text_type for part in parts):
        raise ValueError("Codex native compaction requires an explicit projection for non-text material")
    return [(field, index, "text") for index, part in enumerate(parts)
            if isinstance(part.get("text"), str) and part["text"].strip()]


def closed_native_span(items, admitted_indices, prefix_size):
    """Keep each admitted user occurrence and its completed native assistant/tool evidence.

    `items` excludes the current compaction instruction. Unadmitted user turns mark boundaries.
    Tool calls and signed reasoning are archived but never become editable text anchors.
    """
    spans = []
    anchors = []
    admitted = set(admitted_indices)
    open_calls = set()
    active = False
    for index, item in enumerate(items):
        kind, role = item.get("type", "message"), item.get("role")
        if kind == "message" and role == "user":
            if open_calls:
                raise ValueError("Codex compaction cannot split an unfinished native tool batch")
            active = index in admitted
        if not active:
            continue
        fields = []
        if kind == "message" and role in ("user", "assistant"):
            fields = editable_parts(item.get("content"), "content", "input_text" if role == "user" else "output_text")
        elif kind == "function_call":
            identity = item.get("call_id")
            if not isinstance(identity, str) or not identity or identity in open_calls:
                raise ValueError("Codex source carries an invalid or duplicate native tool call")
            open_calls.add(identity)
        elif kind == "function_call_output":
            identity = item.get("call_id")
            if identity not in open_calls:
                raise ValueError("Codex native tool result is outside its admitted call batch")
            open_calls.remove(identity)
            fields = editable_parts(item.get("output"), "output", "input_text")
        elif kind == "reasoning" or (kind == "message" and role in ("system", "developer")):
            pass
        else:
            raise ValueError(f"Codex native item {kind!r} requires a separate archival contract")
        anchor = ["input", prefix_size + len(spans)]
        spans.append(item)
        anchors.extend([*anchor, *path] for path in fields)
    if open_calls:
        raise ValueError("Codex compaction cannot split an unfinished native tool batch")
    return spans, anchors
