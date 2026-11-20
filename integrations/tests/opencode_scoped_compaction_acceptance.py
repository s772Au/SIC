"""Drive the chain-scoped plugin through real OpenCode process boundaries."""
import argparse
from contextlib import contextmanager
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
from queue import Queue
import re
import secrets
import shutil
import subprocess
import tempfile
from threading import Event, Thread
from urllib.request import HTTPError, ProxyHandler, Request, build_opener
from urllib.parse import urlsplit

from pi_compaction_acceptance import send_text_stream
from pi_tool_wait_acceptance import core_service


ROUTES = {
    "chat": ("openai-compatible", "/v1/chat/completions", "openai-completions"),
    "messages": ("anthropic-compatible", "/v1/messages", "anthropic-messages"),
    "responses": ("openai-compatible-responses", "/v1/responses", "openai-responses"),
}


def parts(frame):
    return frame.get("messages", frame.get("input"))


def head(frame):
    return frame.get("system", frame.get("instructions", next(
        (row for row in parts(frame) if row.get("role") in ("system", "developer")), None)))


def exercise(cli, binary, sdk, root, scenario, protocol="chat"):
    root.mkdir()
    package, wire_path, api = ROUTES[protocol]
    with core_service(binary, root) as admin:
        chain = admin("chain.create", {})["chain_id"]
        foreign = admin("chain.create", {})["chain_id"]
    credential, foreign_credential = secrets.token_urlsafe(24), secrets.token_urlsafe(24)
    with core_service(binary, root, [(chain, credential), (foreign, foreign_credential)]) as core:
        workers, editors, operations = [], [], []
        lost = Event()
        interrupted = scenario in ("lost_commit", "lost_publish", "unknown")
        servers = None
        path_witness = root / "repo" / "native-witness.txt"
        tool_witness = "OPEN_CODE_REAL_NATIVE_READ_RESULT"

        class Provider(BaseHTTPRequestHandler):
            def log_message(self, *_):
                pass

            def do_POST(self):
                raw = self.rfile.read(int(self.headers["content-length"]))
                payload = json.loads(raw)
                if self.path == "/api/v2/harness/operations":
                    if scenario == "unknown" and payload["operation"] == "native.commit" and not lost.is_set():
                        servers.kill()
                        lost.set()
                        return
                    request = Request(core.endpoint + self.path, data=raw, headers={
                        "content-type": "application/json", "authorization": self.headers["authorization"],
                    })
                    try:
                        with build_opener(ProxyHandler({})).open(request, timeout=10) as upstream:
                            status, body = upstream.status, upstream.read()
                    except HTTPError as error:
                        status, body = error.code, error.read()
                    operations.append((payload["operation"], status, json.loads(body).get("error")))
                    loss_operation = {"lost_commit":"native.commit", "lost_publish":"native.publish"}.get(scenario)
                    if payload["operation"] == loss_operation and status == 200 and not lost.is_set():
                        servers.kill()
                        lost.set()
                        return
                    self.send_response(status)
                    self.send_header("content-type", "application/json")
                    self.end_headers()
                    self.wfile.write(body)
                    return

                assert urlsplit(self.path).path == wire_path and payload["model"] == "fixture", self.path
                assert payload.get("stream") is True
                messages = parts(payload)
                use_tool = False
                if "OPENCODE_SIC_EDITOR" in str(messages[-1].get("content", "")):
                    editors.append(payload)
                    text = json.dumps({"summary": f"SIC preserved scoped OpenCode checkpoint {len(editors)}"})
                    tokens = 200
                elif payload.get("tools"):
                    workers.append(payload)
                    text = "NATIVE_SCOPED_WORKER_DONE"
                    tokens = 29000 if scenario == "automatic" and len(workers) == 1 else 200
                    use_tool = scenario == "native_tool" and len(workers) == 1
                else:
                    text, tokens = "Title", 100
                self.send_response(200)
                self.send_header("content-type", "text/event-stream")
                self.end_headers()
                if use_tool:
                    read = next(tool.get("function", tool) for tool in payload["tools"]
                                if tool.get("function", tool).get("name") == "read")
                    fields = read.get("parameters", read.get("input_schema"))["properties"]
                    path_field = next(field for field in ("filePath", "path", "file") if field in fields)
                    if protocol != "chat":
                        send_text_stream(self.wfile.write, api, "fixture", "Reading the native fixture.", tokens,
                                         [("call_oc_native_read", "read", {path_field:str(path_witness)})])
                        return
                    replies = (({"role":"assistant","tool_calls":[{"index":0,"id":"call_oc_native_read","type":"function",
                        "function":{"name":"read","arguments":json.dumps({path_field:str(path_witness)})}}]}, None),
                        ({}, "tool_calls"))
                else:
                    if protocol != "chat":
                        send_text_stream(self.wfile.write, api, "fixture", text, tokens)
                        return
                    replies = (({"role":"assistant","content":text},None), ({}, "stop"))
                for delta, finish in replies:
                    packet = {"id":"oc-scoped","created":1,"object":"chat.completion.chunk","model":"fixture",
                              "choices":[{"index":0,"delta":delta,"finish_reason":finish}]}
                    if finish:
                        packet["usage"] = {"prompt_tokens":tokens,"completion_tokens":7,"total_tokens":tokens+7}
                    self.wfile.write(("data: " + json.dumps(packet) + "\n\n").encode())
                self.wfile.write(b"data: [DONE]\n\n")

        with ThreadingHTTPServer(("127.0.0.1", 0), Provider) as provider:
            thread = Thread(target=provider.serve_forever, daemon=True)
            thread.start()
            try:
                repo = root / "repo"
                repo.mkdir()
                path_witness.write_text(tool_witness, encoding="utf-8")
                plugin = repo / "plugin"
                shutil.copytree(Path(__file__).parents[1] / "opencode", plugin)
                home = root / "home"
                home.mkdir()
                config = repo / "opencode.json"
                endpoint = f"http://127.0.0.1:{provider.server_port}"
                settings = {"providers":{"local":{"env":["SIC_TEST_OC_KEY"],"package":"@opencode/ai/providers/"+package,
                    "settings":{"baseURL":endpoint+"/v1"},"models":{"fixture":{"limit":{"context":32768,"output":4096}}}}}}
                private = secrets.token_urlsafe(24)
                env = {**{name:value for name,value in os.environ.items() if not name.lower().endswith("_proxy")},
                       "HOME":str(home),"XDG_CONFIG_HOME":str(home/"config"),"XDG_CACHE_HOME":str(home/"cache"),
                       "XDG_DATA_HOME":str(home/"data"),"XDG_STATE_HOME":str(home/"state"),"OPENCODE_CONFIG":str(config),
                       "OPENCODE_DISABLE_MODELS_FETCH":"true","SIC_TEST_OC_KEY":secrets.token_urlsafe(24),
                       "OPENCODE_SERVER_PASSWORD":private,"OPENCODE_PASSWORD":private,"NO_PROXY":"127.0.0.1,localhost"}
                if os.name == "nt":
                    env.update({"USERPROFILE":str(home), "APPDATA":str(home/"roaming"), "LOCALAPPDATA":str(home/"local")})

                @contextmanager
                def launch(token=credential):
                    nonlocal servers
                    queue = Queue()
                    events = []
                    with subprocess.Popen([cli,"serve","--hostname","127.0.0.1","--port","0","--print-logs"], cwd=repo,
                        env={**env,"SIC_OPENCODE_HARNESS_TOKEN":token}, text=True, stdin=subprocess.DEVNULL,
                        stdout=subprocess.PIPE, stderr=subprocess.STDOUT) as process:
                        servers = process

                        def consume():
                            for line in process.stdout:
                                events.append(line)
                                if match := re.search(r"http://127\.0\.0\.1:\d+", line):
                                    if "listening" in line:
                                        queue.put(match.group())
                            queue.put(None)

                        Thread(target=consume, daemon=True).start()
                        try:
                            base = queue.get(timeout=20)
                            assert base, events[-15:]
                            yield base, events
                        finally:
                            if process.poll() is None:
                                process.terminate()
                                process.wait(timeout=10)

                def client(base, code):
                    prelude = f"""import {{ OpenCode }} from {json.dumps(sdk.as_uri())};
const authorization = "Basic " + Buffer.from("opencode:" + process.env.OPENCODE_PASSWORD).toString("base64");
const api = OpenCode.make({{baseUrl:{json.dumps(base)},headers:{{authorization,"x-opencode-directory":{json.dumps(str(repo))}}}}});
"""
                    return subprocess.run(["node","--input-type=module","-e",prelude+code],cwd=repo,env=env,
                                          capture_output=True,text=True,stdin=subprocess.DEVNULL,timeout=30)

                config.write_text(json.dumps(settings), encoding="utf-8")
                with launch() as (base, _):
                    result = client(base, 'console.log(JSON.stringify(await api.session.create({title:"SIC target"})));')
                    assert result.returncode == 0, result.stderr
                    session = json.loads(result.stdout)["id"]
                settings["plugins"] = [{"package":str(plugin),"options":{"sessionID":session,"chainID":chain,
                                       "agent":"build","coreUrl":endpoint,"batch":2}}]
                config.write_text(json.dumps(settings), encoding="utf-8")

                def prompt(base, text):
                    return subprocess.run([cli,"--server",base,"run","--format","json","--model","local/fixture",
                        "--session",session,text],cwd=repo,env=env,capture_output=True,text=True,
                        stdin=subprocess.DEVNULL,timeout=25)

                def compact(base):
                    return client(base, f"""
const sessionID = {json.dumps(session)};
await api.session.compact({{sessionID}});
await api.experimental.session.wait({{sessionID}});
""")

                def require_worker(response, events):
                    assert "NATIVE_SCOPED_WORKER_DONE" in response.stdout, (
                        response.stdout[-2000:], response.stderr[-2000:], events[-12:], operations)

                def text(index):
                    return f"OPENCODE_NATIVE_WITNESS_{index} " + "verified source evidence remains important " * 48

                def cut():
                    return core("cut.get", {}, chain)

                with launch() as (base, events):
                    require_worker(prompt(base, text(0)), events)
                    if scenario == "native_tool":
                        assert any(tool_witness in json.dumps(parts(worker)) for worker in workers), (
                            [parts(worker)[-1] for worker in workers])
                    if interrupted:
                        compact(base)
                        assert lost.is_set() and cut()["sequence"] == int(scenario == "lost_publish"), (
                            events[-30:], operations)
                    elif scenario == "automatic":
                        require_worker(prompt(base, text(1)), events)
                        assert cut()["sequence"] >= 1 and len(editors) == 1, (events[-20:], operations)
                    else:
                        for index in range(1, 5):
                            result = compact(base)
                            assert result.returncode == 0 and cut()["sequence"] >= index, (
                                events[-25:], operations, result.stdout, result.stderr)
                            require_worker(prompt(base, text(index)), events)
                        assert max(cb["level"] for cb in cut()["view"]["cbs"]) >= 3

                with launch() as (base, events):
                    prior = len(editors)
                    if scenario == "unknown":
                        compact(base)
                        assert len(editors) == prior and cut()["sequence"] == 0 and cut()["pending"]
                        retired = client(base, f"""
const sessionID = {json.dumps(session)};
await api.session.command({{sessionID,name:"sic-abandon-compaction",text:""}});
await api.experimental.session.wait({{sessionID}});
""")
                        assert retired.returncode == 0 and not cut()["pending"], (events[-12:], operations, retired.stderr)
                        prior += 1
                    if interrupted:
                        recovered = compact(base)
                        assert recovered.returncode == 0, (events[-25:], operations, recovered.stderr)
                    require_worker(prompt(base, text(10)), events)
                    assert len(editors) == prior, (len(editors), prior)
                projected = json.dumps(parts(workers[-1]))
                assert "OPENCODE_NATIVE_WITNESS_10" in projected and "OPENCODE_NATIVE_WITNESS_0" not in projected
                assert all(cb["body"] in projected for cb in cut()["view"]["cbs"])
                assert all(head(editor) == head(workers[0]) and editor["tools"] == workers[0]["tools"]
                           for editor in editors), "Native Editor and Worker differ in prompt or tools"
                if scenario == "native_tool":
                    assert any(tool_witness in json.dumps(parts(editor)[-1]) for editor in editors), (
                        "OpenCode native output was visible to Worker but lost from the Editor scope", operations)
                for token in ("", foreign_credential):
                    with launch(token) as (base, _):
                        count = len(workers)
                        denied = prompt(base, "This bound session requires its original Core grant.")
                        assert len(workers) == count and "NATIVE_SCOPED_WORKER_DONE" not in denied.stdout
                assert core("cut.get", {}, foreign)["sequence"] == 0
                return {"scenario":scenario,"protocol":protocol,"wire_path":wire_path,
                        "level":max(cb["level"] for cb in cut()["view"]["cbs"]),
                        "editors":len(editors),"workers":len(workers),"paid_requests":0}
            finally:
                provider.shutdown()
                thread.join(timeout=5)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("cli", type=Path)
    parser.add_argument("core", type=Path)
    parser.add_argument("--sdk", type=Path, help="Path to the matching @opencode/client dist/promise/index.js")
    scenarios = ("manual", "automatic", "lost_publish", "lost_commit", "unknown", "native_tool")
    parser.add_argument("--protocol", action="append", choices=tuple(ROUTES))
    parser.add_argument("--scenario", action="append", choices=scenarios)
    options = parser.parse_args()
    sdk = (options.sdk or options.cli.parent.parent / "@opencode/client/dist/promise/index.js").absolute()
    with tempfile.TemporaryDirectory(prefix="sic-opencode-scoped-") as scratch:
        results = [exercise(options.cli.absolute(), options.core.absolute(), sdk, Path(scratch) / (protocol + "-" + scenario),
                            scenario, protocol)
                   for protocol in (options.protocol or ["chat"]) for scenario in (options.scenario or scenarios)]
    print(json.dumps(results, indent=2))
