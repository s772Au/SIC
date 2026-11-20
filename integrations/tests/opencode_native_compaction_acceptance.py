import json
import os
from pathlib import Path
import re
import secrets
import subprocess
import sys
import tempfile
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from queue import Queue
from threading import Thread
from pi_tool_wait_acceptance import core_service

cli, binary = map(Path, sys.argv[1:3])
automatic = len(sys.argv) == 4 and sys.argv[3] == "automatic"
sdk = cli.parent.parent / "@opencode/client/dist/promise/index.js"
old, new = "OPEN_CODE_NATIVE_COMPACTION_ORIGINAL", "OPEN_CODE_PUBLISHED_SIC_CHECKPOINT"
with tempfile.TemporaryDirectory(prefix="sic-opencode-compact-") as temporary:
    root = Path(temporary)
    with core_service(binary, root) as core:
        chain = core("chain.create", {})["chain_id"]
        reports = []
        registered = {}

        class Provider(BaseHTTPRequestHandler):
            def log_message(self, *_):
                pass
            def do_POST(self):
                frame = json.loads(self.rfile.read(int(self.headers["content-length"])))
                if self.path in ("/freeze", "/publish"):
                    if self.path == "/freeze":
                        prior = reports[0]
                        index = next(i for i, row in enumerate(prior["messages"]) if row["role"] == "user" and row["content"] == old)
                        source = core("native.begin", {"format":"openai-chat","request":prior,
                            "editable_anchors":[["messages",index,"content"]],"idempotency_key":"open-code-compaction"}, chain)
                        registered.update(source)
                        data = {"editable":next(unit["text"] for unit in source["units"]
                            if unit["unit_id"] == source["binding"]["editable_unit_ids"][0])}
                    else:
                        assert frame == {"summary":new}
                        unit, = registered["binding"]["editable_unit_ids"]
                        identity = {"source_id":registered["source_id"],"expected_source_digest":registered["source_digest"]}
                        core("native.commit", {**identity,"expected_version":0,"edits":[
                            {"unit_id":unit,"expected_text":old,"replacement":new}]}, chain)
                        core("native.publish", {**identity,"expected_version":1,"expected_cut":0,
                            "idempotency_key":"open-code-native-compaction",
                            "ops":[{"op":"COMPRESS","target":unit,"content":new}],"pvi":{}}, chain)
                        cut = core("cut.get", {}, chain)
                        data = {"cut":cut["cut_id"],"body":cut["view"]["cbs"][0]["body"]}
                    self.send_response(200)
                    self.send_header("content-type", "application/json")
                    self.end_headers()
                    self.wfile.write(json.dumps(data).encode())
                    return
                assert self.path == "/v1/chat/completions" and frame["model"] == "fixture"
                if "OPENCODE_SIC_EDITOR" in frame["messages"][-1]["content"]:
                    assert frame["messages"][0] == reports[0]["messages"][0]
                    assert frame["tools"] == reports[0]["tools"]
                    answer = json.dumps({"summary":new})
                else:
                    answer = "OPENCODE WORKER COMPLETE"
                reports.append(frame)
                prompt_tokens = 29000 if automatic and len(reports) == 1 else 350
                self.send_response(200)
                self.send_header("content-type", "text/event-stream")
                self.end_headers()
                for delta, finish in (({"role":"assistant","content":answer}, None), ({}, "stop")):
                    packet = {"id":"ocmock","created":1,"object":"chat.completion.chunk","model":"fixture",
                              "choices":[{"index":0,"delta":delta,"finish_reason":finish}]}
                    if finish:
                        packet["usage"] = {"prompt_tokens":prompt_tokens,"completion_tokens":5,
                                           "total_tokens":prompt_tokens+5}
                    self.wfile.write(("data: " + json.dumps(packet) + "\n\n").encode())
                self.wfile.write(b"data: [DONE]\n\n")

        with ThreadingHTTPServer(("127.0.0.1", 0), Provider) as provider:
            served = Thread(target=provider.serve_forever, daemon=True)
            served.start()
            try:
                repo = root / "project"
                repo.mkdir()
                plug = repo / "fixture"
                plug.mkdir()
                (plug / "package.json").write_text('{"name":"sic-native-compaction-fixture","version":"0.0.0","type":"module","exports":"./index.mjs"}')
                (plug / "index.mjs").write_text("""
import { appendFileSync } from "node:fs";
export default {
  id: "sic-native-compaction-fixture",
  async setup(ctx) {
    const assembled = new Map();
    const json = async (path, body) => {
      const result = await fetch(ctx.options.bridge + path, {method: "POST",
        headers: {"content-type": "application/json"}, body: JSON.stringify(body)});
      if (!result.ok) throw new Error("SIC test bridge operation failed");
      return result.json();
    };
    await ctx.session.hook("title", ev => { ev.result = "SIC native compaction fixture"; });
    await ctx.session.hook("context", ev => {
      assembled.set(ev.sessionID, structuredClone({system:ev.system, tools:ev.tools}));
    });
    await ctx.session.hook("generate", ev => {
      const common = assembled.get(ev.sessionID);
      if (!common) throw new Error("missing prior native assembly");
      ev.system = common.system;
      ev.tools = common.tools;
      ev.messages = [ev.messages.at(-1)];
    });
    await ctx.session.hook("compaction", async ev => {
      const original = ev.messages.some(m => m.role === "user" && m.content.some(
        part => part.type === "text" && part.text === ctx.options.original));
      if (!original) throw new Error("missing native compaction source");
      const material = await json("/freeze", {});
      const candidate = await ctx.session.generate({sessionID:ev.sessionID,
        prompt:JSON.stringify({role:"OPENCODE_SIC_EDITOR", material:material.editable, delivery:{summary:"text"}})});
      const published = await json("/publish", JSON.parse(candidate.text));
      ev.result = { summary: published.body, metadata: { sic_cut: published.cut } };
      appendFileSync(ctx.options.trace, JSON.stringify({ sessionID: ev.sessionID, cut: published.cut }) + "\\n");
    });
  }
};
""", encoding="utf-8")
                trace = root / "hook-trace.jsonl"
                settings = {"plugins":[{"package":str(plug),"options":{"original":old,
                    "bridge":f"http://127.0.0.1:{provider.server_port}","trace":str(trace)}}],
                    "providers":{"local":{"env":["SIC_TEST_OC_KEY"],"package":"@opencode/ai/providers/openai-compatible",
                    "settings":{"baseURL":f"http://127.0.0.1:{provider.server_port}/v1"},
                    "models":{"fixture":{"limit":{"context":32768,"output":4096}}}}}}
                cfg = repo / "opencode.json"
                cfg.write_text(json.dumps(settings), encoding="utf-8")
                private = root / "home"
                private.mkdir()
                password = secrets.token_urlsafe(24)
                env = {**{k:v for k,v in os.environ.items() if not k.lower().endswith("_proxy")},
                       "HOME":str(private),"XDG_CONFIG_HOME":str(private/"config"),
                       "XDG_CACHE_HOME":str(private/"cache"),"XDG_DATA_HOME":str(private/"data"),
                       "XDG_STATE_HOME":str(private/"state"),"OPENCODE_CONFIG":str(cfg),
                       "OPENCODE_DISABLE_MODELS_FETCH":"true","SIC_TEST_OC_KEY":secrets.token_hex(16),
                       "OPENCODE_SERVER_PASSWORD":password,"OPENCODE_PASSWORD":password}
                def start():
                    return subprocess.Popen([cli, "serve", "--hostname", "127.0.0.1", "--port", "0"],
                        cwd=repo, env=env, text=True, stdout=subprocess.PIPE,
                        stderr=subprocess.STDOUT, stdin=subprocess.DEVNULL)

                def address(service):
                    lines = Queue()
                    def receive():
                        for line in service.stdout:
                            lines.put(line)
                        lines.put(None)
                    Thread(target=receive, daemon=True).start()
                    while True:
                        banner = lines.get(timeout=20)
                        if banner is None:
                            raise RuntimeError("OpenCode serve exited")
                        match = re.search(r"http://127\.0\.0\.1:\d+", banner)
                        if match:
                            return match.group()

                with start() as service:
                    try:
                        base = address(service)
                        first = subprocess.run([cli,"--server",base,"run","--format","json","--model","local/fixture",old],
                                               cwd=repo,env=env,capture_output=True,text=True,stdin=subprocess.DEVNULL,timeout=25)
                        assert first.returncode == 0 and "OPENCODE WORKER COMPLETE" in first.stdout, (
                            first.returncode, first.stdout[-1600:], first.stderr[-2000:])
                        rows = [json.loads(line) for line in first.stdout.splitlines() if line.startswith("{")]
                        session = next(row["sessionID"] for row in rows if row.get("sessionID"))
                        code = f"""import {{ OpenCode }} from {json.dumps(sdk.as_uri())};
const authorization = "Basic " + Buffer.from("opencode:" + process.env.OPENCODE_PASSWORD).toString("base64");
const api = OpenCode.make({{baseUrl: {json.dumps(base)}, headers: {{ authorization,
  "x-opencode-directory": {json.dumps(str(repo))} }} }});
const sessionID = {json.dumps(session)};
const request = await api.session.compact({{sessionID}});
await api.experimental.session.wait({{sessionID}});
console.log(JSON.stringify(request));
"""
                        if not automatic:
                            client = subprocess.run(["node","--input-type=module","-e",code],cwd=repo,env=env,text=True,
                                                    capture_output=True,stdin=subprocess.DEVNULL,timeout=30)
                            assert client.returncode == 0 and trace.is_file(), (client.stdout,client.stderr,reports)
                        second = subprocess.run([cli,"--server",base,"run","--format","json","--model","local/fixture",
                            "--session",session,"Continue with the current native task."],cwd=repo,env=env,text=True,
                            capture_output=True,stdin=subprocess.DEVNULL,timeout=30)
                        assert second.returncode == 0 and "OPENCODE WORKER COMPLETE" in second.stdout, (
                            second.stdout, second.stderr)
                        assert trace.is_file() and len(reports) == 3 and new in json.dumps(reports[-1]["messages"]), (
                            trace.is_file(), [r["messages"][-1] for r in reports])
                        assert all("read" in [t["function"]["name"] for t in r["tools"]] for r in reports)
                        cut = core("cut.get", {}, chain)
                        assert cut["sequence"] == 1
                        record = json.loads(trace.read_text().splitlines()[0])
                        service.kill()
                        service.wait(timeout=10)
                    finally:
                        if service.poll() is None:
                            service.terminate()
                            service.wait(timeout=10)
                with start() as restarted:
                    try:
                        resumed = subprocess.run([cli,"--server",address(restarted),"run","--format","json",
                            "--model","local/fixture","--session",session,"Resume the native task."],cwd=repo,env=env,
                            text=True,capture_output=True,stdin=subprocess.DEVNULL,timeout=30)
                        assert resumed.returncode == 0 and "OPENCODE WORKER COMPLETE" in resumed.stdout, (
                            resumed.stdout, resumed.stderr)
                        last = json.dumps(reports[-1]["messages"])
                        assert len(reports) == 4 and new in last and "Continue with the current native task." in last
                        assert record["cut"] == core("cut.get", {}, chain)["cut_id"]
                        print(json.dumps({"result":"PASS","native_compaction_trigger":"automatic" if automatic else "manual",
                            "provided_checkpoint_used_by_request":True,"native_summary_model_requests":0,
                            "native_editor_calls":1, "same_core_cut":True,
                            "cut_and_tail_reused_after_native_server_hard_kill":True,"paid_requests":0}, indent=2))
                    finally:
                        if restarted.poll() is None:
                            restarted.terminate()
                            restarted.wait(timeout=10)
            finally:
                provider.shutdown()
                served.join(timeout=5)
