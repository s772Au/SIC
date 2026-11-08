"""New-core HTTP + official LFX/LangGraph acceptance. Own loopback model only."""
import asyncio
import copy
from contextlib import ExitStack
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import sys
import tempfile
import threading
import uuid

PROJECT = Path(__file__).resolve().parents[2]
sys.path[:0] = [str(PROJECT), str(PROJECT / "integrations")]
from sic_core.service import Core
from sic_core.server import make_server
from sic_workflow.v2_client import CoreClient
from components.sic.action import SICAction


def main():
    requests = []

    class MockModel(BaseHTTPRequestHandler):
        def log_message(self, *args):
            pass

        def do_POST(self):
            request = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
            requests.append(request)
            content = json.dumps({"ops": [], "pvi": {"P": [], "V": [], "I": {}}})
            result = {"id": "fixture-completion", "object": "chat.completion", "created": 1,
                      "model": request["model"], "choices": [{"index": 0, "finish_reason": "stop",
                      "message": {"role": "assistant", "content": content}}],
                      "usage": {"prompt_tokens": 12, "completion_tokens": 8, "total_tokens": 20}}
            body = json.dumps(result).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

    mock = ThreadingHTTPServer(("127.0.0.1", 0), MockModel)
    threading.Thread(target=mock.serve_forever, daemon=True).start()
    saved = {key: os.environ.get(key) for key in ("SIC_CORE_TOKEN", "SIC_CORE_URL", "SIC_CORE_MOCK_KEY", "NO_PROXY", "LITELLM_LOCAL_MODEL_COST_MAP")}
    os.environ["SIC_CORE_TOKEN"] = uuid.uuid4().hex
    os.environ["SIC_CORE_MOCK_KEY"] = uuid.uuid4().hex
    os.environ["NO_PROXY"] = "localhost,127.0.0.1,::1"
    os.environ["LITELLM_LOCAL_MODEL_COST_MAP"] = "true"
    role = {"model": "openai/sic-own-mock", "api_base": f"http://127.0.0.1:{mock.server_port}/v1", "api_key_env": "SIC_CORE_MOCK_KEY",
            "tokenizer_model": "gpt-4o", "max_input_tokens": 100000, "max_output_tokens": 4096, "context_window": 128000,
            "tokenizer_status": "fixture-tokenizer-not-provider-verified"}
    config = {"tokens": {"SIC_CORE_TOKEN": "host"}, "roles": {"worker": role, "editor": role}, "profiles": {"fixture": role}, "revocation_actors": ["host"]}
    destination = PROJECT / ".local/integration-test-results"
    destination.mkdir(parents=True, exist_ok=True)
    server = None
    try:
        with ExitStack() as cleanup:
            temp = cleanup.enter_context(tempfile.TemporaryDirectory(prefix="sic-v2-ui-"))
            core = Core(str(Path(temp) / "store.db"), config)
            cleanup.callback(core.close)
            server = make_server(core, config, port=0)
            threading.Thread(target=server.serve_forever, daemon=True).start()
            cleanup.callback(server.server_close)
            cleanup.callback(server.shutdown)
            os.environ["SIC_CORE_URL"] = f"http://127.0.0.1:{server.server_port}"
            client = CoreClient()
            executed = set()
            context = {}
            trace = []

            def call(operation, payload=None, override=None, expected=True):
                nonlocal context
                component = SICAction()
                component.set(operation_name=operation, context_json=json.dumps(context | (override or {})),
                              payload_json=json.dumps(payload or {}), bindings_json="{}", allow_write=True,
                              allow_model=True, continue_after_error=False)
                envelope = component.run_action().data
                trace.append({"operation": operation, "ok": envelope.get("ok"), "error": envelope.get("error")})
                if expected:
                    assert envelope.get("ok"), envelope
                    executed.add(operation)
                    context = envelope["context"]
                else:
                    assert envelope.get("ok") is False, envelope
                return envelope.get("result", {}) if expected else envelope

            caps = client.capabilities()
            assert caps["implementation"] == "sic-python-core"
            call("chain.create", {"creators": []})
            original = context["chain_id"]
            first = call("source.ingest", {"format": "openai-chat", "request": {"messages": [
                {"role": "system", "content": "Protected constraints"},
                {"role": "user", "content": "first evidence"}, {"role": "user", "content": "first evidence"}]}})
            source = call("source.read", {"source_id": first["source_id"]})["source"]
            assert source["sidecar"]["messages"][0]["role"] == "system"
            call("edit.program", {"source_id": first["source_id"], "rules": ["deduplicate_exact"]})
            material = call("edit.validate", {"job_id": context["job_id"]})["material"]
            assert len([op for op in material["ops"] if op["op"] == "DELETE"]) == 1
            publication = call("cut.publish", {"job_id": context["job_id"], "expected_cut": 0, "idempotency_key": "fixture-cut-one"})
            first_view = call("cut.get")
            frozen_cb = copy.deepcopy(first_view["view"]["cbs"][0])
            if "tools.invoke" in {item["operation"] for item in caps["operations"]}:
                assert call("tools.invoke", {"name": "sic_operation_directory", "arguments": {}})["executed"] == "SIC-program"
            call("cut.ack", {"cut_id": publication["cut_id"], "digest": publication["digest"]})
            checkpoint = call("chain.checkpoint")["checkpoint_id"]
            sock = call("socket.create", {"checkpoint_id": checkpoint, "message_acl": ["host"], "signal_acl": ["host"], "spawn_acl": ["host"]})["socket_id"]
            call("socket.message", {"socket_id": sock, "text": "Correction via new persistent communication", "lifecycle": "P", "message_id": "fixture-P"})
            call("socket.message", {"socket_id": sock, "text": "Verify this next", "lifecycle": "V", "message_id": "fixture-V"})
            call("socket.signal", {"socket_id": sock, "signal": "wait"})
            call("socket.signal", {"socket_id": sock, "signal": "wake"})
            board = call("board.get")
            call("board.update", {"expected_revision": board["revision"], "value": {"task": "verify", "claimed_by": "host"}})
            tool = {"name": "fixture_lookup", "descriptor": "Read fixture evidence", "schema": {"type": "function", "function": {
                "name": "fixture_lookup", "description": "Lookup", "parameters": {"type": "object", "properties": {}}}},
                "discover_acl": ["host"], "expand_acl": ["host"], "execute_acl": ["host"], "expanded": True}
            call("tools.configure", {"tools": [tool]})
            assert call("tools.resolve")["tools"]
            assert call("tools.authorize", {"name": tool["name"]})["authorized"]
            call("index.maintain", {"lifecycle": "I"})
            call("config.get")
            call("config.update", {"worker_profile": "fixture", "editor_profiles": {"editor-1": "fixture", "editor-2": "fixture"},
                                   "program_rules": ["deduplicate_exact"], "schedule_policy": "wait", "interleave_budget": 1,
                                   "worker_prompt": "Use source and operation-directory tools to verify retained evidence when needed."})
            next_source = call("source.ingest", {"format": "openai-chat", "request": {"messages": [
                {"role": "user", "content": "new part one"}, {"role": "user", "content": "new part two"}]}})
            call("harness.event", {"type": "tool_start", "call_id": "fixture-running-tool"})
            call("edit.prepare", {"source_id": next_source["source_id"], "assignments": [
                {"editor": "editor-1", "unit_ids": ["u1"]}, {"editor": "editor-2", "unit_ids": ["u2"]}]})
            model_job = context["job_id"]
            assert call("harness.schedule", {"job_id": model_job, "policy": "wait", "interleave_budget": 1})["decision"] == "overlap"
            call("edit.run", {"job_id": model_job, "editor": "editor-1"})
            call("edit.run", {"job_id": model_job, "editor": "editor-2"})
            assert len(requests) == 2
            call("harness.event", {"type": "tool_end", "call_id": "fixture-running-tool"})
            call("edit.validate", {"job_id": model_job})
            call("cut.publish", {"job_id": model_job, "expected_cut": 1, "idempotency_key": "fixture-cut-two"})
            second_view = call("cut.get")
            assert second_view["view"]["cbs"][0] == frozen_cb
            assert "Correction via" in second_view["view"]["cbs"][1]["body"]
            assert second_view["view"]["pending"]
            assert second_view["view"]["instant"]["board"]["value"]["task"] == "verify"
            preview = call("provider.preview", {"format": "openai-chat"})
            assert "operations" in json.dumps(preview["request"])
            call("provider.continue", {"format": "openai-chat", "idempotency_key": "fixture-worker-one"})
            assert len(requests) == 3
            assert call("source.search", {"query": "first evidence"})["hits"]
            call("usage.record", {"call_id": "fixture-manual-price", "role": "fixture-only", "model": "fixture", "categories": [
                {"category": "fixture_category", "quantity": 2, "unit_price": 0.1}]})
            assert call("usage.report")["unpriced"]
            call("chain.get")
            if "socket.spawn" in {item["operation"] for item in caps["operations"]}:
                call("socket.spawn", {"socket_id": sock, "text": "Continue from historical endpoint", "lifecycle": "V", "message_id": "fixture-spawn"})
                context["chain_id"] = original
            call("chain.fork", {"checkpoint_id": checkpoint})
            peer = context["chain_id"]
            peer_view = call("cut.get")
            assert len(peer_view["view"]["cbs"]) == 1
            assert peer_view["view"]["cbs"][0] == frozen_cb
            peer_source = call("source.ingest", {"format": "openai-chat", "request": {"messages": [{"role": "user", "content": "peer continuation"}]}})
            call("edit.prepare", {"source_id": peer_source["source_id"]})
            call("edit.submit", {"job_id": context["job_id"], "editor": "editor-1", "ops": [
                {"op": "REPLACE", "target": "native.model", "content": "forbidden"}], "pvi": {}}, expected=False)
            call("edit.submit", {"job_id": context["job_id"], "editor": "editor-1", "ops": [],
                                 "pvi": {"P": ["New correction; old CB stays fixed"], "V": [], "I": {"operator_note": "peer"}}})
            call("edit.validate", {"job_id": context["job_id"]})
            call("cut.publish", {"job_id": context["job_id"], "expected_cut": 1, "idempotency_key": "fixture-peer-cut"})
            call("chain.create", {"creators": [original, peer]})
            call("source.ingest", {"format": "openai-chat", "request": {"messages": [{"role": "user", "content": "bad"}, {"role": "system", "content": "late protection"}]}}, expected=False)
            call("config.update", {"worker_profile": "unapproved-route"}, expected=False)
            call("cb.update", {"cb_id": frozen_cb["cb_id"], "body": "forbidden"}, expected=False)
            direct_denial = client._request("/api/v2/operations", {"operation": "cb.update", "context": {**context, "allow_write": True},
                                            "payload": {"cb_id": frozen_cb["cb_id"], "body": "forbidden"}})
            assert direct_denial.get("ok") is False and direct_denial["error"]["code"] == "unknown_operation"
            trace.append({"operation": "direct-http-cb-update", "ok": False, "error": direct_denial["error"]})
            call("source.search", {"query": "evidence"}, override={"actor": "unauthorized-actor"}, expected=False)
            # Final catalog additions use another owned route. Every action is
            # still invoked through the real official SICAction and Core HTTP.
            call('chain.list')
            extra_source = call('source.ingest', {'format': 'openai-chat', 'request': {'messages': [{'role': 'user', 'content': 'Final typed-workbench evidence'}]}})['source_id']
            call('wb.form', {'source_ids': [extra_source], 'reason': 'owned acceptance boundary'})
            extra_job = call('edit.prepare', {'source_id': extra_source})['job_id']
            call('edit.run_all', {'job_id': extra_job})
            candidate = call('edit.candidate.get', {'job_id': extra_job})
            call('edit.candidate.update', {'job_id': extra_job, 'expected_revision': candidate['revision'],
                 'ops': [{'op': 'REPLACE', 'target': 'u1', 'content': 'Versioned final candidate', 'reason': 'Owned review'}], 'pvi': {}})
            call('frontier.compare', {'job_id': extra_job, 'expected_cut': 0, 'position_budget': 0, 'length_budget': 4096})
            call('edit.validate', {'job_id': extra_job})
            extra_pub = call('cut.publish', {'job_id': extra_job, 'expected_cut': 0, 'expected_candidate_revision': 1, 'idempotency_key': 'final-candidate-publication'})
            extra_cb = extra_pub['cb_id']
            call('memory.timeline', {'offset': 0, 'limit': 20})
            call('memory.locate', {'cb_id': extra_cb})
            call('memory.page', {'kind': 'proposal', 'object_id': extra_cb, 'offset': 0, 'limit': 1024})
            call('memory.project', {'role': 'editor', 'max_chars': 4096, 'query': 'evidence'})
            revision = call('decision.get')['revision']
            call('decision.update', {'expected_revision': revision, 'events': [{'kind': 'failure', 'text': 'Owned failed alternative', 'evidence': [{'source_id': extra_source}]}]})
            call('decision.maintain')
            call('runtime.configure', {'enabled': False, 'editor_mode': 'program', 'worker_mode': 'host', 'max_steps': 4})
            call('runtime.get')
            call('runtime.run', {'max_steps': 4, 'continue_worker': False, 'editor_mode': 'program'})
            call('runtime.recover', {'action': 'resume_queue', 'confirm': True})
            extra_checkpoint = call('chain.checkpoint')['checkpoint_id']
            extra_socket = call('socket.create', {'checkpoint_id': extra_checkpoint, 'message_acl': ['host'], 'spawn_acl': ['host'], 'signal_acl': ['host']})['socket_id']
            call('collaboration.dispatch', {'socket_ids': [extra_socket], 'question': 'Inspect final evidence and return findings', 'lifecycle': 'V', 'request_id': 'owned-final-peer'})
            call('collaboration.status')
            call('collaboration.run', {'request_id': 'owned-final-peer', 'max_steps': 8, 'editor_mode': 'model'})
            board = call('board.get')
            call('board.claim', {'task_id': 'owned-final-task', 'expected_revision': board['revision'], 'description': 'Check final evidence'})
            call('resource.record', {'record_id': 'owned-zero-cpu-measurement', 'resource': 'program', 'quantity': '0', 'unit': 'cpu-second', 'measurement_method': 'Explicit zero fixture record; not a production measurement'})
            call('ledger.export')
            model_calls_before_observation = len(requests)
            if "security.revoke" in {item["operation"] for item in caps["operations"]}:
                revoked_source = call("source.ingest", {"format": "openai-chat", "request": {"messages": [{"role": "user", "content": "Own disposable withdrawal fixture"}]}})["source_id"]
                revoked = call("security.revoke", {"source_ids": [revoked_source], "confirm_revoke": True, "reason": "Own temporary acceptance fixture"})
                assert revoked["immutable_bytes_rewritten"] is False
                call("source.read", {"source_id": revoked_source}, expected=False)
            advertised = {spec["operation"] for spec in caps["operations"]}
            assert advertised <= executed, sorted(advertised - executed)
            exported_operations = set()
            for path in (PROJECT / "integrations/flows").glob("core-*.json"):
                fixture_flow = json.loads(path.read_text(encoding="utf-8"))
                for node in fixture_flow["data"]["nodes"]:
                    template = node["data"]["node"]["template"]
                    exported_operations.add(template["operation_name"]["value"])
                    assert template["allow_write"]["value"] is False
                    assert template["allow_model"]["value"] is False
            assert advertised <= exported_operations, sorted(advertised - exported_operations)

            from generate_core_flows import make_core_flow
            from lfx.load import load_flow_from_json
            actions = [
                {"operation": "chain.create", "payload": {"creators": []}},
                {"operation": "source.ingest", "payload": {"format": "openai-chat", "request": {"messages": [{"role": "user", "content": "official runtime fixture"}]}}},
                {"operation": "edit.program", "payload": {"source_id": "$context.source_id", "rules": ["deduplicate_exact"]}},
                {"operation": "edit.validate", "payload": {"job_id": "$context.job_id"}},
                {"operation": "cut.publish", "payload": {"job_id": "$context.job_id", "expected_cut": 0, "idempotency_key": "$context.job_id"}},
                {"operation": "cut.get"}, {"operation": "provider.preview", "payload": {"format": "openai-chat"}},
                {"operation": "usage.report"},
            ]
            flow = make_core_flow("SIC Core acceptance fixture", actions)
            for node in flow["data"]["nodes"]:
                node["data"]["node"]["template"]["allow_write"]["value"] = True
            lfx = load_flow_from_json(flow)
            output = asyncio.run(lfx.arun(inputs=[{}], outputs=["SICAction-8"]))
            node_result = output[0].outputs[0].outputs["result"]
            envelope = node_result.get("message", node_result.get("data", node_result)) if isinstance(node_result, dict) else node_result.data
            assert envelope["ok"], envelope
            complete_flow_stages = {}
            for filename in ("core-native-to-cut.json", "core-editor-to-cut.json"):
                complete = json.loads((PROJECT / "integrations/flows" / filename).read_text(encoding="utf-8"))
                for node in complete["data"]["nodes"]:
                    node["data"]["node"]["template"]["allow_write"]["value"] = True
                count = len(complete["data"]["nodes"])
                loaded = load_flow_from_json(complete)
                finished = asyncio.run(loaded.arun(inputs=[{}], outputs=[f"SICAction-{count}"]))
                final = finished[0].outputs[0].outputs["result"]
                final = final.get("message", final.get("data", final)) if isinstance(final, dict) else final.data
                assert final["ok"], final
                complete_flow_stages[filename] = count

            from sic_workflow.v2_graph import builder
            from langgraph.checkpoint.memory import InMemorySaver
            from langgraph.types import Command
            graph = builder.compile(checkpointer=InMemorySaver())
            config_graph = {"configurable": {"thread_id": "v2-fixture"}, "recursion_limit": 300}
            state = graph.invoke({"actions": [{**action, "allow_write": True} for action in actions]}, config_graph)
            pauses = 0
            while state.get("__interrupt__"):
                pauses += 1
                action = state["__interrupt__"][0].value["action"]
                state = graph.invoke(Command(resume=action), config_graph)
            assert state["status"] == "completed", state.get("result")
            assert pauses == 4
            assert len(state["trace"]) == len(actions)
            extra_actions = [
                {"operation": "harness.event", "context": {"chain_id": original}, "payload": {"type": "wait", "seconds": 1}, "allow_write": True},
                {"operation": "chain.get"}, {"operation": "tools.resolve"}, {"operation": "usage.report"},
            ]
            extra_config = {"configurable": {"thread_id": "v2-additional-stages"}, "recursion_limit": 100}
            extra = graph.invoke({"actions": extra_actions, "context": {"chain_id": original}}, extra_config)
            while extra.get("__interrupt__"):
                extra = graph.invoke(Command(resume=extra["__interrupt__"][0].value["action"]), extra_config)
            assert extra["status"] == "completed", extra.get("result")
            stages_exercised = {item["stage"] for item in state["trace"] + extra["trace"]}
            assert len(stages_exercised) == 8, stages_exercised
            assert len(requests) == model_calls_before_observation, "Observation and manual orchestration must not add model requests"
            secrets = [os.environ["SIC_CORE_TOKEN"], os.environ["SIC_CORE_MOCK_KEY"]]
            exported = json.dumps(flow) + json.dumps(state)
            assert all(secret not in exported for secret in secrets)
            report = {"status": "passed", "new_core_http_operations": sorted(executed), "operation_count": len(executed),
                      "official_component_operations": len(executed),
                      "exported_operations_cover_catalog": advertised <= exported_operations,
                      "http_negative_checks": sum(item["ok"] is False for item in trace), "official_lfx_stages": len(actions), "official_langgraph_stages": len(state["trace"]),
                      "complete_official_lfx_flows": complete_flow_stages, "langgraph_mechanism_routes_exercised": sorted(stages_exercised),
                      "langgraph_write_review_pauses": pauses, "own_loopback_model_calls": len(requests), "real_provider_calls": 0,
                      "published_cb_unchanged": True, "new_pvi_correction": True, "export_contains_credentials": False,
                      "quality_claim": False, "trace": trace}
            (destination / "v2-acceptance.json").write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
            print(json.dumps({key: value for key, value in report.items() if key != "trace"}))
    finally:
        if server:
            server.shutdown()
            server.server_close()
        mock.shutdown()
        mock.server_close()
        for key, value in saved.items():
            if value is None:
                os.environ.pop(key, None)
            else:
                os.environ[key] = value


if __name__ == "__main__":
    main()
