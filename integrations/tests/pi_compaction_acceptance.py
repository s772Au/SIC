"""Drive Pi's real RPC compaction endpoints against a dedicated Rust Core chain."""
import argparse
import base64
from contextlib import contextmanager
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
from queue import Queue
import re
import secrets
import shlex
import struct
import subprocess
import sys
import tempfile
from time import monotonic
import traceback
import zlib
from threading import Event, Thread
from urllib.request import HTTPRedirectHandler, ProxyHandler, Request, build_opener
from urllib.parse import urlsplit
from uuid import uuid4

from pi_tool_wait_acceptance import core_service


PNG = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVQIHWP4z8DwHwAFgAI/ScLttAAAAABJRU5ErkJggg=="
MEDIA_API = {"media_chat": "openai-completions", "media_messages": "anthropic-messages",
             "media_responses": "openai-responses"}
VISUAL_MODES = ("visual_mosaic", "visual_crops", "visual_description", "visual_invalid")
REGIONS = ((64, 70, 60, 44), (330, 175, 68, 42))


class LiveVision:
    """Send only the bounded Pi visual acceptance calls to the configured Provider."""

    def __init__(self, path):
        text = re.split("Provider2", path.read_text(encoding="utf-8-sig"), flags=re.I)[0]
        key = re.search(r"^\s*Key\s*[:：=]\s*(\S+)", text, re.M | re.I)
        model = re.search(r"^\s*(DeepSeek-[\w.-]+)\s*$", text, re.M)
        endpoints = [urlsplit(url) for url in re.findall(r"https://[^\s]+", text)]
        endpoint = next((url for url in endpoints if url.netloc == "www.autodl.art" and
                         url.path.endswith("/anthropic") and not url.query and not url.fragment), None)
        if not key or not model or not endpoint:
            raise RuntimeError("The private provider file has no matching live vision configuration")
        self.key = key[1]
        self.model = model[1]
        self.url = endpoint.geturl().rstrip("/") + "/v1/messages"
        self.requests = []

    def forward(self, payload, role):
        raw = json.dumps(payload, ensure_ascii=False).encode()
        output = payload.get("max_tokens")
        if (payload.get("model") != self.model or not payload.get("stream") or payload.get("thinking") != {"type":"disabled"}
                or not isinstance(output, int) or not 1 <= output <= 4096):
            raise RuntimeError("Unexpected live vision model or output configuration")
        reserve = len(raw) * 2 / 1_000_000 + output * 8 / 1_000_000
        if len(self.requests) >= 10 or sum(row["reserved_cny"] for row in self.requests) + reserve > 2:
            raise RuntimeError("Pi visual acceptance budget exhausted")
        row = {"role":role, "reserved_cny":reserve}
        self.requests.append(row)
        start = monotonic()
        request = Request(self.url, data=raw, headers={"x-api-key":self.key, "anthropic-version":"2023-06-01",
                                                       "content-type":"application/json", "accept":"text/event-stream"})
        class NoRedirect(HTTPRedirectHandler):
            def redirect_request(self, *args, **kwargs):
                return None

        try:
            with build_opener(ProxyHandler({}), NoRedirect()).open(request, timeout=115) as response:
                stream = response.read(2_000_001)
        except Exception as error:
            raise RuntimeError(f"Live vision transport failed ({type(error).__name__}, HTTP {getattr(error, 'code', '?')})") from None
        if len(stream) > 2_000_000:
            raise RuntimeError("Live vision response exceeds the acceptance envelope")
        output_text = []
        tool_arguments = []
        usage = {}
        event_types = []
        for line in stream.splitlines():
            if not line.startswith(b"data:"):
                continue
            event = json.loads(line[5:])
            event_types.append(event.get("type"))
            usage.update(event.get("message", {}).get("usage", {}))
            usage.update(event.get("usage", {}))
            if event.get("content_block", {}).get("type") == "text":
                output_text.append(event["content_block"].get("text", ""))
            if event.get("content_block", {}).get("type") == "tool_use":
                row.setdefault("requested_tools", []).append(event["content_block"].get("name"))
            delta = event.get("delta", {})
            if delta.get("type") == "text_delta":
                output_text.append(delta["text"])
            if delta.get("type") == "input_json_delta":
                tool_arguments.append(delta["partial_json"])
            if delta.get("stop_reason"):
                row["stop_reason"] = delta["stop_reason"]
        row.update({"usage":{key:usage.get(key, 0) for key in ("input_tokens","output_tokens","cache_read_input_tokens",
                     "cache_creation_input_tokens")}, "duration_ms":round((monotonic() - start)*1000),
                     "event_types":sorted(set(str(kind) for kind in event_types))})
        return stream, "".join(tool_arguments or output_text)


def panel_image():
    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))

    width, height = 512, 320
    rows = [bytearray(b"\xe6\xe6\xe6" * width) for _ in range(height)]
    for (x, y, w, h), color in zip((*REGIONS, (30, 225, 20, 22)),
                                  (b"\xed\x19\x83", b"\x17\xa4\xef", b"\xfa\xdc\x16"), strict=True):
        for row in rows[y:y+h]:
            row[x*3:(x+w)*3] = color * w
    raw = b"".join(b"\0" + row for row in rows)
    return base64.b64encode(b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0))
                           + chunk(b"IDAT", zlib.compress(raw)) + chunk(b"IEND", b"")).decode()


def text_part(value):
    if isinstance(value, str):
        return value
    if isinstance(value, list):
        return "".join(part.get("text", "") for part in value if part.get("type") in
                       ("text", "input_text", "output_text"))
    return ""


def image_payloads(value):
    if isinstance(value, list):
        return [image for item in value for image in image_payloads(item)]
    if isinstance(value, dict):
        return [image for item in value.values() for image in image_payloads(item)]
    if isinstance(value, str):
        if value.startswith("data:image/"):
            return [base64.b64decode(value.split(",", 1)[1], validate=True)]
        if value.startswith("iVBORw0KGgo"):
            return [base64.b64decode(value, validate=True)]
    return []


def send_text_stream(write, api, model, text, tokens, calls=(), message_id="msg_local"):
    def send(event, payload):
        prefix = f"event: {event}\n" if event else ""
        write((prefix + "data: " + json.dumps(payload) + "\n\n").encode())

    if api == "anthropic-messages":
        send("message_start", {"type":"message_start", "message":{"id":message_id, "type":"message", "role":"assistant",
             "model":model, "content":[], "stop_reason":None, "usage":{"input_tokens":tokens, "output_tokens":0}}})
        send("content_block_start", {"type":"content_block_start", "index":0, "content_block":{"type":"text", "text":""}})
        send("content_block_delta", {"type":"content_block_delta", "index":0, "delta":{"type":"text_delta", "text":text}})
        send("content_block_stop", {"type":"content_block_stop", "index":0})
        for index, (identity, name, args) in enumerate(calls, 1):
            send("content_block_start", {"type":"content_block_start","index":index,
                "content_block":{"type":"tool_use","id":identity,"name":name,"input":{}}})
            send("content_block_delta", {"type":"content_block_delta","index":index,
                "delta":{"type":"input_json_delta","partial_json":json.dumps(args)}})
            send("content_block_stop", {"type":"content_block_stop","index":index})
        send("message_delta", {"type":"message_delta",
             "delta":{"stop_reason":"tool_use" if calls else "end_turn"}, "usage":{"output_tokens":7}})
        send("message_stop", {"type":"message_stop"})
    else:
        response = {"id":"resp_local", "object":"response", "model":model, "status":"in_progress", "output":[]}
        initial = {"type":"message", "id":"msg_local", "role":"assistant", "status":"in_progress", "content":[]}
        complete = {**initial, "status":"completed", "content":[{"type":"output_text","text":text,"annotations":[]}]}
        send("response.created", {"type":"response.created","response":response})
        send("response.output_item.added", {"type":"response.output_item.added","output_index":0,"item":initial})
        send("response.output_text.delta", {"type":"response.output_text.delta","output_index":0,"content_index":0,
                                           "item_id":initial["id"],"delta":text})
        send("response.output_item.done", {"type":"response.output_item.done","output_index":0,"item":complete})
        output = [complete]
        for index, (identity, name, args) in enumerate(calls, 1):
            item = {"type":"function_call","id":"fc_"+identity,"call_id":identity,
                    "name":name,"arguments":"","status":"in_progress"}
            send("response.output_item.added", {"type":"response.output_item.added","output_index":index,"item":item})
            arguments = json.dumps(args)
            send("response.function_call_arguments.delta", {"type":"response.function_call_arguments.delta",
                "output_index":index,"item_id":item["id"],"delta":arguments})
            send("response.function_call_arguments.done", {"type":"response.function_call_arguments.done",
                "output_index":index,"item_id":item["id"],"arguments":arguments})
            complete_call = {**item,"arguments":arguments,"status":"completed"}
            send("response.output_item.done", {"type":"response.output_item.done","output_index":index,"item":complete_call})
            output.append(complete_call)
        send("response.completed", {"type":"response.completed", "response":{**response,"status":"completed",
             "output":output,"usage":{"input_tokens":tokens,"output_tokens":7,"total_tokens":tokens+7,
             "input_tokens_details":{"cached_tokens":0},"output_tokens_details":{"reasoning_tokens":0}}}})


class PiRpc:
    def __init__(self, process, tool_finished, wait=25):
        self.process = process
        self.events = Queue()
        self.sequence = 0
        self.compaction_reasons = []
        self.notifications = []
        self.tool_finished = tool_finished
        self.wait = wait
        Thread(target=self._receive, daemon=True).start()

    def _receive(self):
        for line in self.process.stdout:
            try:
                event = json.loads(line)
                if event.get("type") == "turn_end" and event.get("toolResults"):
                    self.tool_finished.set()
                self.events.put(event)
            except json.JSONDecodeError:
                self.events.put({"unexpected_stdout": line.strip()})
        self.events.put(None)

    def next(self):
        event = self.events.get(timeout=self.wait)
        if event is None:
            raise EOFError("Pi stopped during RPC")
        if event.get("type") == "compaction_start":
            self.compaction_reasons.append(event["reason"])
        if event.get("type") == "extension_ui_request" and event.get("method") == "notify":
            self.notifications.append(event["message"])
        return event

    def send(self, kind, **fields):
        self.sequence += 1
        identity = f"fixture-{self.sequence}"
        command = {"type": kind, "id": identity, **fields}
        self.process.stdin.write(json.dumps(command) + "\n")
        self.process.stdin.flush()
        return identity

    def until(self, target, *, allow_extension_error=False):
        observed = []
        for _ in range(1500):
            event = self.next()
            if target(event):
                return event
            observed.append(event)
            if (event.get("type") == "extension_error" and not allow_extension_error) or "unexpected_stdout" in event:
                raise AssertionError(observed[-4:])
        raise AssertionError(observed[-4:])

    def command(self, name, *, success=True, **fields):
        identity = self.send(name, **fields)
        response = self.until(lambda event: event.get("type") == "response" and event.get("id") == identity)
        assert response["success"] is success, (response, self.notifications)
        return response.get("data")

    def prompt(self, text, *, success=True, images=None):
        # RPC acknowledges admission before the context hook can stop dispatch.
        self.command("prompt", message=text, **({"images": images} if images else {}))
        self.until(lambda event: event.get("type") == "agent_settled", allow_extension_error=not success)
        if success:
            assert not self.notifications, self.notifications


def exercise(pi, core_binary, config, scenario, shell=None, sharp=None, live=None):
    scenario, _, transport = scenario.partition("__")
    automatic = scenario in ("threshold", "overflow")
    config.mkdir()
    with core_service(core_binary, config) as admin:
        chain = admin("chain.create", {})["chain_id"]
        foreign_chain = admin("chain.create", {})["chain_id"]
    secret = secrets.token_urlsafe(32)
    foreign_secret = secrets.token_urlsafe(32)
    with core_service(core_binary, config, [(chain, secret), (foreign_chain, foreign_secret)]) as core:
        capture = {"editors": [], "workers": [], "errors": [], "transactions": []}
        crash = Event()
        published_without_receipt = Event()
        committed_without_receipt = Event()
        interrupted_before_commit = Event()
        overflowed = Event()
        waiting_tool = scenario in ("tool_first", "editor_first", "wait_restart", "multi_tool")
        tool_ids = ["call_sic_durable", "call_sic_parallel"] if scenario == "multi_tool" else ["call_sic_durable"]
        visual = scenario in VISUAL_MODES
        media = scenario in MEDIA_API or visual
        source_image = panel_image() if visual else PNG
        original_pixels = base64.b64decode(source_image)
        api = MEDIA_API.get(scenario, MEDIA_API.get("media_" + transport, "openai-completions"))
        call_identity = lambda value: value.split("|", 1)[0]
        provider_path = {"openai-completions":"/v1/chat/completions", "openai-responses":"/v1/responses",
                         "anthropic-messages":"/v1/messages"}[api]
        tool_started = Event()
        tool_finished = Event()
        cut_published = Event()
        long_progress = "WORKER_FROZEN_PLAN: derive the result from authenticated native file evidence. " * 4
        tool_result = "SIC_NATIVE_LATE_TOOL_WITNESS"
        timeline = []
        lookups = []
        model = live.model if live else "sic-pi-compaction-fixture"
        text = "retain verified state, decisions, source evidence and recent work " * 100
        witness = lambda index: f"NATIVE_SESSION_WITNESS_{index}\n{text}"
        if live:
            assert scenario == "visual_mosaic" and api == "anthropic-messages"
        project = config / "project"
        project.mkdir()
        session = uuid4().hex
        running = None

        class Endpoint(BaseHTTPRequestHandler):
            def log_message(self, *_):
                pass

            def do_POST(self):
                try:
                    self.handle_request()
                except (BrokenPipeError, ConnectionResetError):
                    if not published_without_receipt.is_set():
                        capture["errors"].append("unexpected connection closed")
                except Exception as error:
                    capture["errors"].append(traceback.format_exc(limit=2))
                    self.send_error(400, "compaction fixture rejected request")

            def do_GET(self):
                assert self.path == "/tool-gate"
                timeline.append("tool-started")
                tool_started.set()
                if scenario == "editor_first":
                    assert cut_published.wait(12)
                timeline.append("tool-released")
                self.send_response(200)
                self.end_headers()
                self.wfile.write(tool_result.encode())

            def handle_request(self):
                raw = self.rfile.read(int(self.headers["content-length"]))
                payload = json.loads(raw)
                if self.path == "/api/v2/harness/operations":
                    request = Request(core.endpoint + self.path, data=raw, headers={
                        "content-type": "application/json", "authorization": self.headers["authorization"],
                    })
                    if scenario == "unknown" and payload["operation"] == "native.commit" and crash.is_set():
                        crash.clear()
                        interrupted_before_commit.set()
                        running.process.kill()
                        return
                    # Use Core itself for every authorization and state transition.
                    from urllib.error import HTTPError
                    try:
                        with build_opener(ProxyHandler({})).open(request, timeout=10) as upstream:
                            status, body = upstream.status, upstream.read()
                    except HTTPError as error:
                        status, body = error.code, error.read()
                    capture["transactions"].append([payload["operation"], status, json.loads(body).get("error")])
                    if media and payload["operation"] == "native.begin":
                        archived = payload["payload"]["request"]["messages"]
                        assert image_payloads(archived) == [original_pixels], [
                            (m["role"], text_part(m.get("content"))[:35]) for m in archived]
                    if waiting_tool and payload["operation"] == "native.publish" and status == 200 \
                            and not cut_published.is_set():
                        response = json.loads(body)["result"]
                        resolved_calls = response["binding"]["tool_wait"]["boundary"]["call_ids"]
                        assert [call_identity(value) for value in resolved_calls] == tool_ids, resolved_calls
                        assert tool_result not in json.dumps(response["source_request"])
                        timeline.append("tool-wait-published")
                        cut_published.set()
                        if scenario == "wait_restart":
                            published_without_receipt.set()
                            running.process.kill()
                            return
                    if scenario == "commit_ack" and payload["operation"] == "native.commit" and status == 200 and crash.is_set():
                        crash.clear()
                        committed_without_receipt.set()
                        running.process.kill()
                        return
                    if payload["operation"] == "native.publish" and status == 200 and crash.is_set():
                        crash.clear()
                        published_without_receipt.set()
                        running.process.kill()
                        return
                    self.send_response(status)
                    self.send_header("content-type", "application/json")
                    self.end_headers()
                    self.wfile.write(body)
                    return
                assert urlsplit(self.path).path == provider_path, self.path
                assert payload["model"] == model and payload["stream"] is True
                messages = payload.get("messages", payload.get("input"))
                requested_call = None
                head = payload.get("system", payload.get("instructions"))
                if head is None:
                    head = next((message for message in messages if message.get("role") in ("system", "developer")), None)
                content = text_part(messages[-1].get("content"))
                tools = payload.get("tools", [])
                if visual and "SIC_EDITOR_COMPACTION" in content:
                    assert [tool.get("function", tool)["name"] for tool in tools] == ["sic_submit_visual"], (
                        "Visual Editor received tools outside its delivery scope")
                else:
                    assert "read" in [tool.get("function", tool)["name"] for tool in tools]
                if live and ("SIC_EDITOR_COMPACTION" in content or len(capture["workers"]) >= 2):
                    is_editor = "SIC_EDITOR_COMPACTION" in content
                    if is_editor:
                        assert image_payloads(messages) == [original_pixels]
                        capture["editors"].append(payload)
                    else:
                        outbound_images = image_payloads(messages)
                        assert len(outbound_images) == 1 and original_pixels not in outbound_images, (
                            "The Worker must receive one derived image", len(outbound_images), capture.get("visual_delivery"))
                        worker_text = " ".join(text_part(m.get("content")) for m in messages)
                        assert not re.search(r"\b(magenta|pink|fuchsia|cyan|blue)\b", worker_text, re.I), (
                            "The selected text also supplies the visual answer", capture.get("visual_delivery"))
                        assert "description" not in next(part["text"] for m in messages if isinstance(m.get("content"), list)
                            for part in m["content"] if part.get("text", "").startswith('{"sic_visual_evidence":'))
                        capture["workers"].append(payload)
                    stream, observed = live.forward(payload, "editor" if is_editor else "worker")
                    capture["visual_delivery" if is_editor else "visual_answer"] = observed
                    self.send_response(200)
                    self.send_header("content-type", "text/event-stream")
                    self.end_headers()
                    self.wfile.write(stream)
                    return
                if "SIC_EDITOR_COMPACTION" in content:
                    invocation = json.loads(content)
                    assert invocation["kind"] == "SIC_EDITOR_COMPACTION"
                    current = core("cut.get", {}, chain)
                    assert [text_part(m.get("content")) for m in messages[:-1]
                            if m.get("role") == "user" and text_part(m.get("content"))] == [
                        cb["body"] for cb in current["view"]["cbs"]]
                    editor_head = text_part(head.get("content") if isinstance(head, dict) else head)
                    worker_head = text_part(capture["worker_head"].get("content") if isinstance(capture["worker_head"], dict)
                                            else capture["worker_head"])
                    assert editor_head and worker_head.startswith(editor_head) and "separate Worker and Editor" in editor_head
                    if not visual:
                        assert tools == capture["workers"][-1]["tools"]
                    assert invocation["units"]
                    if media:
                        actual_images = image_payloads(messages)
                        assert actual_images == [original_pixels], (len(actual_images), [
                            (m.get("role"), [item.get("type") for item in m.get("content", [])
                                             if isinstance(item, dict)]) for m in messages])
                    capture["editors"].append({"cut": current["sequence"], "units": len(invocation["units"])})
                    if waiting_tool and len(capture["editors"]) == 1:
                        assert long_progress in json.dumps(invocation)
                        assert tool_result not in json.dumps(invocation)
                        assert tool_started.wait(12)
                        if scenario != "editor_first":
                            assert tool_finished.wait(12), "Pi did not persist native results before continuation"
                        timeline.append("editor-responded")
                    elif waiting_tool:
                        assert long_progress not in json.dumps(invocation), "Published Worker progress was captured twice"
                    delivery = {"summary": f"SIC retained authenticated facts {len(capture['editors'])}"}
                    if visual:
                        image, = invocation["image_sources"]
                        assert (image["width"], image["height"]) == (512, 320)
                        mode = scenario.removeprefix("visual_")
                        delivery["visual"] = {"display":"mosaic" if mode == "invalid" else mode,
                            "crops":[{"message_id":image["message_id"] if mode != "invalid" else "foreign-message",
                                      "block_index":image["block_index"],"x":x,"y":y,"width":w,"height":h}
                                     for x,y,w,h in REGIONS],
                            "description":[{"text":"Two colored components were visible.","tiles":[1,2]}]}
                        requested_call = ("visual_delivery", "sic_submit_visual", delivery)
                    answer = json.dumps(delivery)
                    usage = 230
                else:
                    assert not any("summary of an earlier part" in str(m.get("content", "")).lower() for m in messages)
                    capture["workers"].append(payload)
                    capture["worker_head"] = head
                    if waiting_tool and len(capture["workers"]) == 2:
                        assert tool_result in json.dumps(messages)
                        assert long_progress not in json.dumps(messages)
                        assert "SIC retained authenticated facts 1" in json.dumps(messages)
                    if scenario == "overflow" and len(capture["workers"]) == 3:
                        overflowed.set()
                        self.send_response(400)
                        self.send_header("content-type", "application/json")
                        self.end_headers()
                        self.wfile.write(json.dumps({"error":{"message":"This model's maximum context length was exceeded",
                            "type":"invalid_request_error","code":"context_length_exceeded"}}).encode())
                        return
                    answer = "PI_WORKER_DONE"
                    usage = 31000 if scenario == "threshold" and len(capture["workers"]) == 2 else 230
                    if visual and len(capture["workers"]) >= 3:
                        marker = next((part["text"] for m in messages if isinstance(m.get("content"), list)
                                       for part in m["content"] if part.get("type") in ("text","input_text")
                                       and part.get("text", "").startswith('{"sic_visual_evidence":')), None)
                        assert marker, "Pi Worker did not receive a selected visual representation"
                        manifest = json.loads(marker)["sic_visual_evidence"]
                        assert "sic_image_source" in [tool.get("function", tool)["name"] for tool in tools]
                        visual_inputs = image_payloads(messages)
                        if scenario == "visual_description":
                            steps = [("crop",1),("description",None),("mosaic",1),("crop",1)]
                            index = len(lookups)
                            if index == 0:
                                assert not visual_inputs
                            if index == 1:
                                assert not visual_inputs and "before stepping back" in json.dumps(messages)
                            if index == 2:
                                assert len(visual_inputs) == 1 and original_pixels not in visual_inputs
                            if index == 3:
                                assert len(visual_inputs) == 2 and original_pixels not in visual_inputs
                            if index == 4:
                                assert visual_inputs[-1] == original_pixels
                                answer = "PI_VISUAL_RECALL_COMPLETE"
                            if index < len(steps):
                                requested, tile = steps[index]
                                args = {"manifest_id":manifest["manifest_id"],"from":requested}
                                if tile is not None:
                                    args["tile"] = tile
                                requested_call = (f"visual_step_{index}", "sic_image_source", args)
                                lookups.append(requested)
                        else:
                            expected = (1 if scenario == "visual_mosaic" else 2)
                            assert len(visual_inputs) == expected and original_pixels not in visual_inputs
                            assert manifest["representation"] == scenario.removeprefix("visual_")
                            assert "magenta" not in json.dumps(messages).lower()
                            answer = "PI_VISUAL_MOSAIC_TASK_COMPLETE"
                stats = {"prompt_tokens": usage, "completion_tokens": 7, "total_tokens": usage + 7}
                self.send_response(200)
                self.send_header("content-type", "text/event-stream")
                self.end_headers()
                if requested_call:
                    if api != "openai-completions":
                        send_text_stream(self.wfile.write, api, model, "Inspecting the previous visual representation.",
                                         usage, [requested_call])
                        return
                    identity, name, args = requested_call
                    replies = (({"role":"assistant","tool_calls":[{"index":0,"id":identity,"type":"function",
                        "function":{"name":name,"arguments":json.dumps(args)}}]}, None), ({}, "tool_calls"))
                elif waiting_tool and len(capture["workers"]) == 1 and not capture["editors"]:
                    url = f"http://127.0.0.1:{server.server_port}/tool-gate"
                    program = f"import urllib.request; print(urllib.request.urlopen({url!r}, timeout=15).read().decode())"
                    command = f"{shlex.quote(Path(sys.executable).as_posix())} -c {shlex.quote(program)}"
                    args = {"command":command,"timeout":20}
                    if api != "openai-completions":
                        send_text_stream(self.wfile.write, api, model, long_progress, usage,
                                         [(identity,"bash",args) for identity in tool_ids])
                        return
                    calls = [{"index":index, "id":identity, "type":"function","function":{"name":"bash",
                              "arguments":json.dumps(args)}}
                             for index, identity in enumerate(tool_ids)]
                    replies = (({"role":"assistant","content":long_progress,"tool_calls":calls}, None), ({}, "tool_calls"))
                elif scenario == "editor_tool" and "SIC_EDITOR_COMPACTION" in content:
                    replies = (({"role":"assistant","tool_calls":[{"index":0,"id":"editor-write",
                        "type":"function","function":{"name":"write","arguments":json.dumps({
                            "path":str(project / "editor-must-not-write.txt"),"content":"untrusted edit"})}}]}, None),
                        ({}, "tool_calls"))
                else:
                    if api != "openai-completions":
                        send_text_stream(self.wfile.write, api, model, answer, usage)
                        return
                    replies = (({"role":"assistant","content":answer}, None), ({}, "stop"))
                for delta, finish in replies:
                    packet = {"id":"compaction-fixture","created":1,"model":model,"object":"chat.completion.chunk",
                              "choices":[{"index":0,"delta":delta,"finish_reason":finish}],"usage":stats}
                    self.wfile.write(("data: " + json.dumps(packet) + "\n\n").encode())
                self.wfile.write(b"data: [DONE]\n\n")

        with ThreadingHTTPServer(("127.0.0.1", 0), Endpoint) as server:
            thread = Thread(target=server.serve_forever, daemon=True)
            thread.start()
            try:
                endpoint = f"http://127.0.0.1:{server.server_port}"
                home = config / "pi"
                home.mkdir()
                (home / "models.json").write_text(json.dumps({"providers": {"sic-fixture": {
                    "baseUrl": endpoint if api == "anthropic-messages" else endpoint + "/v1",
                    "api": api, "apiKey": "fixture",
                    "models": [{"id": model, "contextWindow": 32768, "maxTokens": 4096,
                                **({"reasoning":True} if live else {}),
                                **({"input":["text", "image"]} if media else {})}]}}}), encoding="utf-8")
                settings = {"compaction": {"enabled": automatic, "reserveTokens": 4096, "keepRecentTokens": 1250}}
                if media:
                    settings["images"] = {"autoResize": False}
                if shell:
                    settings["shellPath"] = str(shell.resolve())
                (home / "settings.json").write_text(json.dumps(settings), encoding="utf-8")
                env = {**{key: value for key, value in os.environ.items() if not key.lower().endswith("_proxy")},
                       "PI_CODING_AGENT_DIR": str(home), "PI_SKIP_VERSION_CHECK": "1", "PI_TELEMETRY": "0",
                       "SIC_PI_COMPACTION_SESSION": session, "SIC_PI_CORE_URL": endpoint,
                       "SIC_PI_HARNESS_TOKEN": secret, "SIC_PI_COMPACTION_BATCH": "2",
                       "NO_PROXY":"127.0.0.1,localhost"}
                if visual:
                    assert sharp, "Visual scenarios require --sharp"
                    env["SIC_PI_VISUAL_SHARP"] = str(sharp.absolute())
                else:
                    env.pop("SIC_PI_VISUAL_SHARP", None)
                extension = Path(__file__).parents[1] / "pi/sic-compaction.ts"
                command = [*pi, "--mode", "rpc", "--provider", "sic-fixture", "--model", model,
                           "--session-dir", config / "sessions", "--session-id", session, "--no-extensions", "-e", extension]
                if live:
                    command += ["--thinking", "off"]

                @contextmanager
                def client(credential=secret):
                    nonlocal running
                    with (config / "pi-stderr.log").open("a", encoding="utf-8") as stderr:
                        with subprocess.Popen(command, env={**env, "SIC_PI_HARNESS_TOKEN": credential}, cwd=project,
                                              stdin=subprocess.PIPE,
                                              stdout=subprocess.PIPE, stderr=stderr, text=True, encoding="utf-8") as process:
                            running = PiRpc(process, tool_finished, wait=130 if live else 25)
                            try:
                                yield running
                            finally:
                                if process.poll() is None:
                                    process.terminate()
                                process.wait(timeout=10)

                def read_cut():
                    return core("cut.get", {}, chain)

                if media:
                    with client() as rpc:
                        task = witness(0)
                        if live:
                            task = ("Archive the two large colored rectangles in the attached panel for a later identification task. "
                                    "The small lower-left square is unrelated. Preserve both large rectangles as two tight crops, "
                                    "then use the mosaic. Keep the text summary generic, with no colors or visual answers, since the "
                                    "later task needs to read the mosaic. Use direct pixel coordinates.\n" + task)
                        rpc.prompt(task, images=[{"type":"image","data":source_image,"mimeType":"image/png"}])
                        assert capture["workers"], (api, capture["errors"], str(list(rpc.events.queue)[-6:])[-1300:])
                        assert image_payloads(capture["workers"][0]) == [original_pixels], (
                            "Pi's initial native request did not contain the fixture image",
                            [(m.get("role"), [p.get("type") for p in m.get("content", []) if isinstance(p, dict)])
                             for m in capture["workers"][0].get("messages", capture["workers"][0].get("input", []))])
                        rpc.prompt(witness(1))
                        if scenario == "visual_invalid":
                            rpc.command("compact", success=False)
                            assert read_cut()["sequence"] == 0 and not capture["errors"]
                            assert any("outside this native assignment" in error for error in rpc.notifications), rpc.notifications
                            return {"out_of_scope_editor_crop_rejected":True}
                        if live:
                            try:
                                rpc.command("compact")
                            except AssertionError as error:
                                raise RuntimeError((rpc.notifications, capture["errors"], live.requests)) from error
                            delivery = json.loads(capture["visual_delivery"])
                            recipe = delivery.get("visual")
                            assert recipe and recipe.get("display", "mosaic") == "mosaic", (delivery, live.requests)
                        else:
                            crash.set()
                            rpc.send("compact")
                            assert published_without_receipt.wait(12), (capture["errors"], capture["transactions"])
                    original_cut = read_cut()
                    with client() as rpc:
                        if not live:
                            rpc.command("compact")
                        rpc.prompt("Read the mosaic. Report the colors of the left large rectangle and the right large rectangle, "
                                   "in that order, as a JSON object with keys left and right." if live
                                   else "Observe the retained Pi image and continue.")
                    assert len(capture["editors"]) == 1 and not capture["errors"], capture["errors"]
                    final = capture["workers"][-1]
                    if not visual:
                        assert image_payloads(final) == [original_pixels]
                    assert "NATIVE_SESSION_WITNESS_0" not in json.dumps(final)
                    assert "NATIVE_SESSION_WITNESS_1" in json.dumps(final)
                    assert original_cut["cut_id"] == read_cut()["cut_id"]
                    archive, = (config / "sessions").glob("*.jsonl")
                    rows = [json.loads(line) for line in archive.read_text(encoding="utf-8").splitlines()]
                    source = next(row["data"]["source"] for row in rows if row.get("customType") ==
                                  "sic-compaction-intent-v1")
                    candidate = core("native.get", {"source_id":source}, chain)
                    assert any(block["kind"] == "image" and block.get("host_origin")
                               for block in candidate["opaque_blocks"]), candidate["opaque_blocks"]
                    if live:
                        recipe = json.loads(capture["visual_delivery"])["visual"]
                        assert recipe.get("display", "mosaic") == "mosaic" and len(recipe["crops"]) == len(REGIONS)
                        for x, y, width, height in REGIONS:
                            assert any(max(0, min(x+width,crop["x"]+crop["width"])-max(x,crop["x"])) *
                                       max(0, min(y+height,crop["y"]+crop["height"])-max(y,crop["y"])) >= .75*width*height
                                       and crop["width"] * crop["height"] <= 4 * width * height
                                       for crop in recipe["crops"]), recipe
                        decoded = json.loads(re.search(r"\{[^{}]+\}", capture["visual_answer"])[0])
                        assert re.search("magenta|pink|fuchsia", decoded["left"], re.I), decoded
                        assert re.search("blue|cyan", decoded["right"], re.I), decoded
                        return {"actual_model":model,"regions_selected":len(REGIONS),"worker_answer":decoded,
                                "worker_received_only_mosaic":True,"restart_after_publish":True,"requests":live.requests}
                    archive.write_text(archive.read_text(encoding="utf-8").replace(source_image, "aW52YWxpZA=="),
                                       encoding="utf-8")
                    admitted = len(capture["workers"])
                    with client() as rpc:
                        rpc.prompt("An altered native image must prevent continuation.", success=False)
                    assert len(capture["workers"]) == admitted
                    return {"native_provider_path":provider_path, "editor_received_original":True,
                            **({"selected_representation":scenario.removeprefix("visual_"),"lookups":lookups}
                               if visual else {"worker_received_one_exact_image":True}),
                            "image_origin_archived":True,"publish_ack_recovered_after_restart":True,
                            "tampered_local_image_rejected":True}

                if waiting_tool:
                    with client() as rpc:
                        if scenario == "wait_restart":
                            rpc.command("prompt", message=witness(0))
                            assert published_without_receipt.wait(15), (capture["errors"], timeline)
                        else:
                            rpc.prompt(witness(0))
                    first = read_cut()
                    assert first["sequence"] == 1 and len(capture["editors"]) == 1
                    if scenario == "editor_first":
                        assert timeline.index("tool-wait-published") < timeline.index("tool-released")
                    else:
                        assert timeline.index("tool-released") < timeline.index("editor-responded")
                    with client() as rpc:
                        rpc.prompt(witness(1))
                        rpc.command("compact")
                        rpc.prompt(witness(2))
                    final = read_cut()
                    assert max(cb["level"] for cb in final["view"]["cbs"]) >= 2
                    body = capture["workers"][-1]
                    assert all(cb["body"] in json.dumps(body.get("messages", body.get("input"))) for cb in final["view"]["cbs"])
                    archive, = (config / "sessions").glob("*.jsonl")
                    original = archive.read_text(encoding="utf-8")
                    assert long_progress in original and tool_result in original
                    assert timeline.count("tool-started") == len(tool_ids) and not capture["errors"], capture["errors"]
                    return {"tool_wait_published":True,"highest_adopted_level":2,
                            "tool_result_kept_by_host":True,"tool_count":len(tool_ids),
                            "publication_survived_hard_kill":scenario == "wait_restart"}

                with client() as rpc:
                    rpc.prompt(witness(0))
                    rpc.prompt(witness(1))
                    if automatic:
                        rpc.prompt(witness(2))
                        assert capture["editors"] and read_cut()["sequence"] >= 1, (
                            capture["transactions"], rpc.compaction_reasons, capture["errors"])
                        assert rpc.compaction_reasons == [scenario], rpc.compaction_reasons
                    elif scenario == "editor_tool":
                        rpc.command("compact", success=False)
                        assert any("completed text candidate" in line for line in rpc.notifications)
                        assert not (project / "editor-must-not-write.txt").exists()
                        assert read_cut()["sequence"] == 0 and len(capture["editors"]) == 1
                        return {"editor_native_tool_not_executed": True, "invalid_candidate_not_published": True}
                    else:
                        crash.set()
                        rpc.send("compact")
                        completion = {"unknown":interrupted_before_commit,"commit_ack":committed_without_receipt,
                                      "manual":published_without_receipt}[scenario]
                        assert completion.wait(10), (
                            capture["errors"], capture["transactions"],
                            str(list(rpc.events.queue)[-8:])[-3600:],
                            (config / "pi-stderr.log").read_text(encoding="utf-8")[-3200:],
                        )
                if scenario == "unknown":
                    with client() as rpc:
                        rpc.command("compact", success=False)
                        assert any("outcome is unknown" in line for line in rpc.notifications)
                        rpc.prompt(witness(2), success=False)
                        assert len(capture["editors"]) == 1 and len(capture["workers"]) == 2
                        assert read_cut()["sequence"] == 0 and len(read_cut()["pending"]) == 1
                        advance = rpc.command("prompt", message="/sic-abandon-compaction")
                        assert advance["disposition"] == "handled"
                        assert not read_cut()["pending"] and not read_cut()["sequence"]
                        rpc.command("compact")
                    assert read_cut()["sequence"] == 1 and len(capture["editors"]) == 2
                    return {"unknown_editor_not_reissued":True, "uncertain_input_not_dispatched":True,
                            "operator_retired_and_recovered":True}
                if scenario == "commit_ack":
                    assert read_cut()["sequence"] == 0
                    with client() as rpc:
                        rpc.command("compact")
                        rpc.prompt(witness(2))
                    assert read_cut()["sequence"] == 1 and len(capture["editors"]) == 1
                    return {"durable_editor_delivery_published_after_restart": True, "duplicate_editor": False}
                if automatic:
                    selected = read_cut()
                    assert any(cb["body"] in json.dumps(capture["workers"][-1]["messages"])
                               for cb in selected["view"]["cbs"])
                else:
                    first = read_cut()
                    assert first["sequence"] == 1 and len(capture["editors"]) == 1
                    with client() as rpc:
                        rpc.command("compact")
                        assert len(capture["editors"]) == 1 and read_cut()["cut_id"] == first["cut_id"]
                        for index in range(2, 6):
                            rpc.prompt(witness(index))
                            rpc.command("compact")
                        rpc.prompt(witness(6))
                    selected = read_cut()
                    assert max(cb["level"] for cb in selected["view"]["cbs"]) >= 3, selected["view"]["cbs"]
                    assert all(cb["body"] in json.dumps(capture["workers"][-1]["messages"])
                               for cb in selected["view"]["cbs"])
                    assert first["view"]["cbs"][0]["body"] not in json.dumps(capture["workers"][-1]["messages"])
                    requests_before = len(capture["workers"])
                    for token in ("", foreign_secret):
                        with client(token) as rpc:
                            rpc.prompt("This bound session requires its original Core grant.", success=False)
                    assert len(capture["workers"]) == requests_before
                    assert not core("cut.get", {}, foreign_chain)["sequence"]
                session_file, = (config / "sessions").glob("*.jsonl")
                archive = session_file.read_text(encoding="utf-8")
                assert "NATIVE_SESSION_WITNESS_0" in archive and secret not in archive
                assert not capture["errors"], capture["errors"]
                return {"cut_sequence": selected["sequence"], "editor_calls": len(capture["editors"]),
                        "worker_calls": len(capture["workers"]),
                        "highest_adopted_level": max(cb["level"] for cb in selected["view"]["cbs"]),
                        "publication_receipt_loss_recovered": published_without_receipt.is_set(),
                        "overflow_retried": overflowed.is_set(),
                        "original_transcript_kept": True}
            finally:
                server.shutdown()
                thread.join(timeout=5)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--pi", type=Path, required=True)
    parser.add_argument("--core", type=Path, required=True)
    parser.add_argument("--node", type=Path)
    parser.add_argument("--shell", type=Path)
    parser.add_argument("--sharp", type=Path)
    parser.add_argument("--live-provider-file", type=Path)
    parser.add_argument("--scenario", action="append")
    args = parser.parse_args()
    live = LiveVision(args.live_provider_file) if args.live_provider_file else None
    pi = [args.pi.absolute()]
    if args.node:
        pi.insert(0, args.node.absolute())
    with tempfile.TemporaryDirectory(prefix="sic-pi-compaction-") as temporary:
        root = Path(temporary)
        names = ("manual", "threshold", "overflow", "unknown", "commit_ack", "editor_tool",
                 "tool_first", "editor_first", "wait_restart", "multi_tool", *MEDIA_API,
                 *(f"{race}__{transport}" for transport in ("messages", "responses")
                   for race in ("tool_first", "editor_first", "wait_restart")))
        if args.sharp:
            names = (*names, *VISUAL_MODES, *(f"{mode}__{transport}" for transport in ("messages", "responses")
                                               for mode in VISUAL_MODES))
        selected = ("visual_mosaic__messages",) if live else (args.scenario or names)
        results = {name: exercise(pi, args.core.resolve(), root / name, name, args.shell, args.sharp, live)
                   for name in selected}
    print(json.dumps({"result":"PASS","paid_requests":len(live.requests) if live else 0,**results}, indent=2))


if __name__ == "__main__":
    main()
