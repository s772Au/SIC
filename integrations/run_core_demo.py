"""Persistent local teaching fixture: real SIC core, own HTTP mock, zero real models."""
import argparse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import sys
import threading
import time

PROJECT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(PROJECT))
from sic_core.service import Core
from sic_core.server import make_server


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--port", type=int, default=18888)
    parser.add_argument("--mock-port", type=int, default=18889)
    parser.add_argument("--bridge-port", type=int, default=18890)
    parser.add_argument("--db", default=".local/core-ui-demo.db")
    args = parser.parse_args()
    if not os.environ.get("SIC_CORE_TOKEN"):
        raise RuntimeError("Set a private SIC_CORE_TOKEN environment variable before starting the demo")
    os.environ["NO_PROXY"] = "localhost,127.0.0.1,::1"
    os.environ["LITELLM_LOCAL_MODEL_COST_MAP"] = "true"
    os.environ["SIC_OFFLINE_FIXTURE_KEY"] = "offline-own-loopback-only"

    class FixtureModel(BaseHTTPRequestHandler):
        def log_message(self, *args):
            pass

        def do_POST(self):
            request = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
            try:
                material = json.loads(request["messages"][-1]["content"])
            except (ValueError, KeyError, TypeError):
                material = {}
            serialized = json.dumps(request, ensure_ascii=False)
            dsh_test = "SIC_DSH_TOOL_TEST" in serialized
            peer_test = "SIC_DSH_PEER_TEST" in serialized
            peer_work = "SIC_PEER_WORK_TOOL_TEST" in serialized
            declared_wait = any((tool.get("function", {}).get("name") or tool.get("name")) == "sic_fixture_wait" for tool in request.get("tools", []))
            tool_calls = None
            if "editable_text_units" in material:
                if dsh_test or peer_work:
                    time.sleep(0.9)
                ops = [{"op": "COMPRESS", "target": unit["unit_id"], "content": "[offline fixture] " + unit["text"][:160],
                        "reason": "Deterministic transport fixture; no model quality claim"} for unit in material["editable_text_units"]]
                content = json.dumps({"ops": ops, "pvi": {"P": [], "V": [], "I": {}}}, ensure_ascii=False)
            elif peer_test and "SIC_PEER_TOOL_DONE" in serialized:
                content = "SIC_DSH_PEER_ROUNDTRIP_OK"
            elif peer_work and "SIC_SYNTHETIC_TOOL_RESULT" in serialized:
                content = "SIC_PEER_RESPONSE_OK: the historical peer consumed the actual Harness wait-tool result."
            elif peer_work and declared_wait:
                content = "SIC_PEER_WORK_TOOL_TEST: peer progress before its real external tool."
                tool_calls = [{"id":"sic_peer_wait_1","type":"function","function":{"name":"sic_fixture_wait","arguments":"{}"}}]
            elif peer_test:
                content = "Synthetic parent task requesting a historical peer."
                tool_calls = [{"id":"sic_parent_peer_1","type":"function","function":{"name":"sic_fixture_peer_roundtrip","arguments":"{}"}}]
            elif dsh_test and "SIC_SYNTHETIC_TOOL_RESULT" in serialized:
                content = "SIC_DSH_ROUNDTRIP_OK"
            elif dsh_test and declared_wait:
                # Real assistant material accompanies the native tool proposal.
                # It becomes a WB while the Harness actually runs the tool;
                # a call containing only protocol fields has no text to edit.
                content = "SIC_DSH_TOOL_TEST: synthetic progress before the actual wait tool. " * 8
                tool_calls = [{"id": "sic_dsh_wait_1", "type": "function", "function": {"name": "sic_fixture_wait", "arguments": "{}"}}]
            else:
                content = "[offline fixture Worker] Continued from the encoded SIC workspace. No real model inference."
            message = {"role": "assistant", "content": content}
            if tool_calls:
                message["tool_calls"] = tool_calls
            body = json.dumps({"id": "offline-fixture", "object": "chat.completion", "created": 1,
                               "model": request["model"], "choices": [{"index": 0, "finish_reason": "tool_calls" if tool_calls else "stop", "message": message}],
                               "usage": {"prompt_tokens": 12, "completion_tokens": 8, "total_tokens": 20}}, ensure_ascii=False).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

    mock = ThreadingHTTPServer(("127.0.0.1", args.mock_port), FixtureModel)
    threading.Thread(target=mock.serve_forever, daemon=True).start()
    role = {"model": "openai/sic-offline-fixture", "api_base": f"http://127.0.0.1:{mock.server_port}/v1",
            "api_key_env": "SIC_OFFLINE_FIXTURE_KEY", "route_id": "own-loopback-fixture",
            "tokenizer_model": "gpt-4o", "max_input_tokens": 100000, "max_output_tokens": 4096,
            "context_window": 128000, "tokenizer_status": "fixture-tokenizer-not-provider-verified"}
    # This profile exercises a known image-counting policy on our own fixture
    # endpoint. It never routes to OpenAI or verifies a real model's behavior.
    image_role = {**role, "model": "openai/gpt-4o-2024-08-06", "tokenizer_model": "gpt-4o-2024-08-06",
                  "route_id": "own-loopback-fixture-image-accounting"}
    config = {"tokens": {"SIC_CORE_TOKEN": "host"}, "roles": {"worker": role, "editor": role},
              "profiles": {"offline-fixture": role, "offline-fixture-image-accounting": image_role},
              "bridge": {"allow_write": True, "allow_model": True, "editor_mode": "model", "seal_bytes": 1, "public_model": "sic-worker",
                         "host_tools_enabled":True,"host_tool_allowlist":["sic_fixture_wait"]}}
    path = PROJECT / args.db
    path.parent.mkdir(parents=True, exist_ok=True)
    core = Core(str(path), config)
    server = make_server(core, config, port=args.port)
    bridge = None
    if args.bridge_port:
        from sic_core.bridge import make_bridge_server
        bridge = make_bridge_server(core, config, port=args.bridge_port)
        threading.Thread(target=bridge.serve_forever, daemon=True).start()
    print(json.dumps({"service": "sic-core-demo", "core_port": args.port, "mock_port": mock.server_port,
                      "bridge_port": args.bridge_port, "authentication": "required", "real_provider_calls": 0, "quality_verified": False}), flush=True)
    try:
        server.serve_forever()
    finally:
        server.server_close()
        if bridge:
            bridge.shutdown()
            bridge.server_close()
        core.close()
        mock.shutdown()
        mock.server_close()


if __name__ == "__main__":
    main()
