import copy
import json
from pathlib import Path
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "scripts"))
from dsh_chat_bridge import Bridge, history_parts, generated_assistant_equivalent


class Client:
    def __init__(self):
        self.ingested, self.requests = [], []

    def ingest(self, session, units):
        self.ingested.append((session, units))

    def call(self, method, path, body=None, headers=None):
        if path == "/api/v1/sessions":
            return {"id": "session"}
        self.requests.append(copy.deepcopy((body, headers)))
        return {"choices": [{"message": {"role": "assistant", "content": "ok"}, "finish_reason": "stop"}]}


class BridgeTests(unittest.TestCase):
    def test_actual_installed_pi_ai_roundtrip_fixtures(self):
        fixture = json.loads((Path(__file__).parent / "fixtures/pi-ai-1.1.0-replay.json").read_text(encoding="utf-8"))
        self.assertEqual((fixture["version"], fixture["network_requests"]), ("1.1.0", 0))
        for sample in fixture["samples"]:
            with self.subTest(sample=sample["name"]):
                expected, received = copy.deepcopy(sample["upstream_assistant"]), copy.deepcopy(sample["pi_replayed_assistant"])
                self.assertEqual(generated_assistant_equivalent(expected, received), sample["expected_compatible"])
                self.assertEqual(expected, sample["upstream_assistant"])
                self.assertEqual(received, sample["pi_replayed_assistant"])

    def test_only_last_generated_assistant_can_be_reformatted(self):
        fixture = json.loads((Path(__file__).parent / "fixtures/pi-ai-1.1.0-replay.json").read_text(encoding="utf-8"))
        sample = next(s for s in fixture["samples"] if s["name"] == "tool-json-reformat")

        class ReplayClient(Client):
            def call(self, method, path, body=None, headers=None):
                if path == "/api/v1/sessions":
                    return {"id": "session"}
                self.requests.append(copy.deepcopy((body, headers)))
                assistant = sample["upstream_assistant"] if len(self.requests) == 1 else {"role": "assistant", "content": "ok"}
                return {"choices": [{"message": copy.deepcopy(assistant), "finish_reason": "tool_calls" if assistant.get("tool_calls") else "stop"}]}

        client, first = ReplayClient(), {"model": "mock", "messages": [{"role": "system", "content": "protected"}, {"role": "user", "content": "one"}]}
        bridge = Bridge(client, 1000000)
        bridge.chat("a", first)
        second = copy.deepcopy(first)
        second["messages"] += [copy.deepcopy(sample["pi_replayed_assistant"]), {"role": "tool", "tool_call_id": "call_test", "content": "result"}, {"role": "user", "content": "two"}]
        unchanged = copy.deepcopy(second)
        bridge.chat("a", second)
        self.assertEqual(second, unchanged)
        self.assertEqual(client.requests[-1][0]["messages"], second["messages"])
        self.assertEqual(bridge.states["a"]["expected"][:-1], second["messages"])
        third = copy.deepcopy(second)
        third["messages"] += [{"role": "assistant", "content": "ok"}, {"role": "user", "content": "three"}]
        third["messages"][2]["tool_calls"][0]["function"]["arguments"] = '{ "b": 2, "a": 1 }'
        with self.assertRaisesRegex(ValueError, "prefix mismatch"):
            bridge.chat("a", third)

    def test_argument_reformatting_never_hides_value_or_precision_changes(self):
        def assistant(arguments):
            return {"role": "assistant", "content": None, "tool_calls": [{"id": "id", "type": "function", "function": {"name": "f", "arguments": arguments}}]}
        rejected = [(' {"x":9007199254740993}', '{"x":9007199254740992}'),
            ('{"x":1234567890123456789012345678901}', '{"x":1234567890123456789012345678902}'),
            ('{"x":true}', '{"x":1}'), ('{"x":null}', '{"x":"null"}'),
            ('{"x":1,"x":2}', '{"x":2}'), ('{"x":NaN}', '{"x":NaN}'),
            ('{"x":{"a":1,"a":2}}', '{"x":{"a":2}}'),
            ('{"x":Infinity}', '{"x":Infinity}'), ('[1,2]', '[1,2]'),
            ('{"x":-0}', '{"x":0}'), ('{"x":[1,2]}', '{"x":[2,1]}')]
        for before, after in rejected:
            with self.subTest(before=before, after=after):
                self.assertFalse(generated_assistant_equivalent(assistant(before), assistant(after)))
        self.assertTrue(generated_assistant_equivalent(assistant('{"n":1.00,"text":"中","arr":[true,null]}'),
            assistant('{ "arr": [true,null], "text":"\\u4e2d", "n": 1e0 }')))

    def test_nonempty_metadata_text_and_tool_identity_stay_strict(self):
        base = {"role": "assistant", "content": "text", "tool_calls": [{"id": "a", "type": "function", "function": {"name": "f", "arguments": "{}"}}]}
        for field, value in [("reasoning_content", "reasoning"), ("refusal", "refusal"), ("annotations", [{"url": "evidence"}]), ("unknown", None)]:
            changed = dict(copy.deepcopy(base), **{field: value})
            self.assertFalse(generated_assistant_equivalent(changed, base), field)
        for mutate in (lambda m: m.update(content="text "),
                       lambda m: m["tool_calls"][0].update(id="b"),
                       lambda m: m["tool_calls"][0]["function"].update(name="g")):
            changed = copy.deepcopy(base)
            mutate(changed)
            self.assertFalse(generated_assistant_equivalent(base, changed))
        self.assertFalse(generated_assistant_equivalent({"role": "assistant", "content": None}, {"role": "assistant", "content": ""}))
        ordered = copy.deepcopy(base)
        ordered["tool_calls"].append({"id": "b", "type": "function", "function": {"name": "g", "arguments": "{}"}})
        swapped = copy.deepcopy(ordered)
        swapped["tool_calls"].reverse()
        self.assertFalse(generated_assistant_equivalent(ordered, swapped))

    def test_accepted_user_tool_and_protected_messages_remain_strict(self):
        first = {"model": "mock", "messages": [{"role": "system", "content": "protect"}, {"role": "user", "content": "one"},
            {"role": "assistant", "content": None, "tool_calls": [{"id": "a", "type": "function", "function": {"name": "f", "arguments": "{}"}}]},
            {"role": "tool", "tool_call_id": "a", "content": "result"}, {"role": "user", "content": "two"}]}
        bridge = Bridge(Client(), 1000000)
        reply, _ = bridge.chat("a", first)
        for index in (0, 1, 3):
            changed = copy.deepcopy(first)
            changed["messages"] += [reply["choices"][0]["message"], {"role": "user", "content": "next"}]
            changed["messages"][index]["content"] += " changed"
            with self.assertRaisesRegex(ValueError, "prefix mismatch"):
                bridge.chat("a", changed)

    def test_full_history_auto_ingest_and_exact_retry(self):
        client = Client()
        bridge = Bridge(client, 1)
        request = {"model": "mock", "messages": [{"role": "system", "content": "protect"}, {"role": "user", "content": "one"}]}
        reply, _ = bridge.chat("a", request)
        self.assertEqual(bridge.chat("a", request)[0], reply)
        self.assertEqual(len(client.requests), 1)
        follow = copy.deepcopy(request)
        follow["messages"] += [reply["choices"][0]["message"], {"role": "user", "content": "two"}]
        bridge.chat("a", follow)
        self.assertEqual(len(client.ingested), 1)
        self.assertNotIn("protect", "".join(client.ingested[0][1]))
        self.assertEqual(client.requests[-1][0]["messages"], [request["messages"][0], follow["messages"][-1]])
        changed = copy.deepcopy(follow)
        changed["messages"][1]["content"] = "changed"
        with self.assertRaisesRegex(ValueError, "prefix mismatch"):
            bridge.chat("a", changed)

    def test_tool_groups_are_never_split(self):
        call = {"role": "assistant", "tool_calls": [{"id": "a"}, {"id": "b"}]}
        result_a, result_b = {"role": "tool", "tool_call_id": "a", "content": "x"}, {"role": "tool", "tool_call_id": "b", "content": "y"}
        _, groups = history_parts([call, result_b, result_a, {"role": "user", "content": "continue"}])
        self.assertEqual(groups[0], [call, result_b, result_a])
        for invalid in ([call], [call, result_a], [call, result_a, result_a]):
            with self.assertRaises(ValueError):
                history_parts(invalid)

    def test_late_protected_instruction_rejected(self):
        with self.assertRaisesRegex(ValueError, "late protected"):
            history_parts([{"role": "user", "content": "x"}, {"role": "system", "content": "late"}])


if __name__ == "__main__":
    unittest.main()
