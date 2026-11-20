"""Exercise native Codex App Server compaction through a configured Responses route."""
import argparse
from contextlib import contextmanager
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
from queue import Queue
import subprocess
import tempfile
from threading import Thread

from pi_compaction_acceptance import send_text_stream


def exercise(binary, directory, automatic):
    seen = []
    old = "CODEX_NATIVE_ORIGINAL_WITNESS"
    summary = "CODEX_NATIVE_GATEWAY_COMPACTION_WITNESS"
    instruction = "CODEX_NATIVE_COMPACTION_ROUTE_FIXTURE"
    route = "sic-test-model"
    phase = "initial"

    class Provider(BaseHTTPRequestHandler):
        def log_message(self, *_):
            pass

        def do_POST(self):
            frame = json.loads(self.rfile.read(int(self.headers["content-length"])))
            seen.append((phase, self.path, frame))
            if not self.path.endswith("/responses"):
                self.send_error(404)
                return
            compaction = instruction in json.dumps(frame["input"])
            result = summary if compaction else "CODEX_NATIVE_WORKER_DONE"
            tokens = 12000 if automatic and phase == "initial" and not compaction else 150
            self.send_response(200)
            self.send_header("content-type", "text/event-stream")
            self.end_headers()
            send_text_stream(self.wfile.write, "openai-responses", route, result, tokens)

    with ThreadingHTTPServer(("127.0.0.1", 0), Provider) as server:
        thread = Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            codex_dir = directory / "codex"
            codex_dir.mkdir()
            endpoint = f"http://127.0.0.1:{server.server_port}/v1"
            auto = "model_auto_compact_token_limit = 8000" if automatic else ""
            (codex_dir / "config.toml").write_text(f"""
model = "{route}"
model_provider = "sic_local"
compact_prompt = "{instruction}"
approval_policy = "never"
sandbox_mode = "read-only"
{auto}
[model_providers.sic_local]
name = "SIC acceptance fixture"
base_url = "{endpoint}"
env_key = "SIC_CODEX_FIXTURE_KEY"
wire_api = "responses"
supports_websockets = false
""", encoding="utf-8")
            env = {key:value for key,value in os.environ.items()
                   if not key.startswith(("CODEX_", "OPENAI_")) and not key.lower().endswith("_proxy")}
            env.update({"CODEX_HOME":str(codex_dir),"SIC_CODEX_FIXTURE_KEY":"offline-fixture",
                        "HOME":str(directory),"NO_PROXY":"127.0.0.1,localhost"})

            @contextmanager
            def serve():
                queue = Queue()
                with (directory/"app-server.log").open("a", encoding="utf-8") as stderr:
                    with subprocess.Popen([binary,"app-server","--listen","stdio://"],env=env,cwd=directory,
                                          stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=stderr,text=True) as service:
                        def read():
                            for line in service.stdout:
                                queue.put(json.loads(line))
                            queue.put(None)
                        Thread(target=read, daemon=True).start()
                        sequence = 0
                        events = []

                        def until(predicate):
                            for _ in range(1200):
                                event = queue.get(timeout=30)
                                assert event, (events[-10:], seen, (directory/"app-server.log").read_text()[-4000:])
                                events.append(event)
                                if predicate(event):
                                    return event
                            raise AssertionError(events[-10:])

                        def send(method, params, notification=False):
                            nonlocal sequence
                            sequence += 1
                            frame = {"jsonrpc":"2.0","method":method,"params":params}
                            if not notification:
                                frame["id"] = sequence
                            service.stdin.write(json.dumps(frame)+"\n")
                            service.stdin.flush()
                            if not notification:
                                result = until(lambda item: item.get("id") == sequence)
                                assert "error" not in result, result
                                return result["result"]

                        send("initialize", {"clientInfo":{"name":"sic-acceptance","version":"0.1.4"}})
                        send("initialized", {}, notification=True)
                        try:
                            yield send, until
                        finally:
                            if service.poll() is None:
                                service.terminate()
                                service.wait(timeout=10)

            def turn(send, until, session, text):
                start = send("turn/start", {"threadId":session,"input":[{"type":"text","text":text,"text_elements":[]}]})
                turn_id = start["turn"]["id"]
                complete = until(lambda ev: ev.get("method") == "turn/completed"
                                 and ev["params"]["turn"]["id"] == turn_id)
                assert complete["params"]["turn"]["status"] == "completed", complete

            with serve() as (send, until):
                session = send("thread/start", {"cwd":str(directory),"model":route,"modelProvider":"sic_local"})["thread"]["id"]
                turn(send, until, session, old)
                phase = "compact"
                if automatic:
                    turn(send, until, session, "Advance after crossing the native compaction threshold.")
                else:
                    send("thread/compact/start", {"threadId":session})
                    compacted = until(lambda ev: ev.get("method") == "turn/completed")
                    assert compacted["params"]["turn"]["status"] == "completed", compacted
                assert any(p == "compact" and old in json.dumps(body) and instruction in json.dumps(body)
                           for p, _, body in seen), [(p, path) for p,path,_ in seen]
            phase = "resumed"
            with serve() as (send, until):
                send("thread/resume", {"threadId":session, "cwd":str(directory), "model":route, "modelProvider":"sic_local"})
                turn(send, until, session, "Continue using the persisted checkpoint.")
            assert any(p == "resumed" and summary in json.dumps(body) for p, _, body in seen)
            print(json.dumps({"result":"PASS","native_compaction":"automatic" if automatic else "manual",
                              "configured_responses_route":True,
                              "native_checkpoint_seen_after_cold_resume":True,"paid_requests":0}, indent=2))
        finally:
            server.shutdown()
            thread.join(timeout=5)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("codex", type=Path)
    parser.add_argument("--automatic", action="store_true")
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="sic-codex-compaction-") as directory:
        exercise(args.codex.absolute(), Path(directory), args.automatic)
