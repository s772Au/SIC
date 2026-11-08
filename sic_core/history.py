"""Narrow replay equivalence for the latest bridge-generated assistant only."""
import copy,json
from decimal import Decimal,InvalidOperation

def canonical(value):
    return json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":"))

def argument_object(raw):
    """Compare valid JSON objects losslessly, without float rounding or duplicate keys."""
    if not isinstance(raw, str):
        raise ValueError("tool arguments must be JSON text")

    def pairs(items):
        result = {}
        for key, value in items:
            if key in result:
                raise ValueError("duplicate tool argument key")
            result[key] = value
        return result

    def reject_constant(value):
        raise ValueError("non-finite tool argument number: " + value)

    value = json.loads(raw, object_pairs_hook=pairs, parse_int=Decimal, parse_float=Decimal,
                       parse_constant=reject_constant)
    if not isinstance(value, dict):
        raise ValueError("tool arguments must be an object")

    def typed(item):
        if item is None:
            return ("null",)
        if isinstance(item, bool):
            return ("boolean", item)
        if isinstance(item, str):
            return ("string", item)
        if isinstance(item, Decimal):
            if not item.is_finite():
                raise ValueError("non-finite tool argument number")
            sign, digits, exponent = item.as_tuple()
            digits = list(digits)
            if not any(digits):
                return ("number", sign, [0], 0)
            while digits[-1] == 0:
                digits.pop()
                exponent += 1
            return ("number", sign, digits, exponent)
        if isinstance(item, list):
            return ("array", [typed(part) for part in item])
        if isinstance(item, dict):
            return ("object", [(key, typed(part)) for key, part in sorted(item.items())])
        raise ValueError("unsupported tool argument value")

    return typed(value)

def generated_assistant_equivalent(expected, received):
    """Narrow pi-ai replay equivalences, only for the last bridge-owned reply.

    Do not use this for earlier history, protected/user/tool messages or request
    forwarding. Nonempty metadata, IDs, tool names/order and text stay strict.
    """
    if not isinstance(expected, dict) or not isinstance(received, dict):
        return False
    if expected.get("role") != "assistant" or received.get("role") != "assistant":
        return False
    left, right = copy.deepcopy(expected), copy.deepcopy(received)
    try:
        for message in (left, right):
            if message.get("refusal", "missing") is None:
                message.pop("refusal")
            if message.get("annotations") == []:
                message.pop("annotations")
            # Installed pi-ai inserts this empty field for compatible reasoning models.
            if message.get("reasoning_content", "missing") in (None, ""):
                message.pop("reasoning_content", None)
            # pi-ai emits tool_calls only when the reconstructed call list is nonempty.
            if message.get("tool_calls", "missing") is None or message.get("tool_calls") == []:
                message.pop("tool_calls", None)
            calls = message.get("tool_calls")
            if calls:
                if not isinstance(calls, list):
                    return False
                if message.get("content") in (None, "", []):
                    message["content"] = None
                for position, call in enumerate(calls):
                    if not isinstance(call, dict) or call.get("type") != "function":
                        return False
                    # LiteLLM may retain the response stream's positional
                    # index. Native pi-ai reconstructs complete tool calls
                    # without this transport field; IDs and list order remain
                    # authoritative. Never discard unknown/wrong positions.
                    if "index" in call:
                        if type(call["index"]) is not int or call["index"] != position:
                            return False
                        call.pop("index")
                    function = call.get("function")
                    if not isinstance(function, dict):
                        return False
                    function["arguments"] = argument_object(function.get("arguments"))
        return canonical(left) == canonical(right)
    except (ValueError, TypeError, InvalidOperation, RecursionError):
        return False
