"""Exercise Hermes' real conversation loop and native manual compaction helper."""

import argparse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import secrets
import subprocess
import tempfile
from threading import Thread

from pi_tool_wait_acceptance import core_service


NATIVE = """
import json, os
from pathlib import Path
from run_agent import AIAgent
from agent.conversation_compression_manual import CompressRequest, compress_now
from agent.conversation_compression import finalize_context_engine_compression_notification
from hermes_state import SessionDB

mode = os.environ['SIC_HERMES_TEST_PHASE']
session_id = 'sic-hermes-test'
db = SessionDB(Path(os.environ['SIC_HERMES_TEST_HISTORY']))
if mode != 'restart':
    db.create_session(session_id, 'cli', model='fixture')
agent = AIAgent(provider='custom', model='fixture', base_url=os.environ['SIC_HERMES_TEST_PROVIDER'],
    api_key='loopback', session_id=session_id, quiet_mode=True, enabled_toolsets=['file'],
    skip_memory=True, skip_context_files=True, max_iterations=3, session_db=db)
history = db.get_messages_as_conversation(session_id)

def turn(prompt):
    result = agent.run_conversation(prompt, conversation_history=history)
    assert result.get('final_response') == 'HERMES_SIC_WORKER_DONE', result
    return result['messages']

if mode == 'restart':
    history = turn('Continue from the persisted native Hermes transcript.')
elif mode == 'manual':
    for i in range(6):
        history = turn('Original witness ' + str(i) + ': ' + 'retain finalized prior findings and important work ' * 70)
        if i:
            result = compress_now(agent, history, CompressRequest())
            assert result.status == 'compressed', result.status
            history = result.after_messages
            current = db.get_messages_as_conversation(session_id)
            assert any(str(row.get('content')).startswith('[SIC Hermes native checkpoint ') for row in current)
            finalize_context_engine_compression_notification(agent, committed=True)
elif mode == 'automatic':
    for i in range(3):
        history = turn('Trigger threshold with native history ' + str(i) + ': ' + 'retain verified work ' * 120)

db.close()
"""


def exercise(python, source, core_binary, root, mode):
    with core_service(core_binary, root) as admin:
        chain = admin("chain.create", {})["chain_id"]
    secret = secrets.token_urlsafe(24)
    with core_service(core_binary, root, [(chain, secret)]) as core:
        endpoint = ""
        workers, editors = [], []

        class Provider(BaseHTTPRequestHandler):
            def log_message(self, *_):
                pass

            def do_POST(self):
                request = json.loads(self.rfile.read(int(self.headers["content-length"])))
                editor = "HERMES_SIC_EDITOR" in str(request.get("messages", [])[-1].get("content"))
                if editor:
                    editors.append(request)
                    answer = json.dumps({"summary":f"Verified native Hermes evidence checkpoint {len(editors)}"})
                else:
                    workers.append(request)
                    answer = "HERMES_SIC_WORKER_DONE"
                input_tokens = 59000 if mode == "automatic" and not editor else 250
                usage = {"prompt_tokens":input_tokens,"completion_tokens":8,"total_tokens":input_tokens+8}
                self.send_response(200)
                if request.get("stream"):
                    self.send_header("content-type","text/event-stream")
                    self.end_headers()
                    for delta, finish in (({"role":"assistant","content":answer},None), ({}, "stop")):
                        result = {"id":"hermes-native","created":1,"object":"chat.completion.chunk","model":"fixture",
                                  "choices":[{"index":0,"delta":delta,"finish_reason":finish}],"usage":usage}
                        self.wfile.write(("data: "+json.dumps(result)+"\n\n").encode())
                    self.wfile.write(b"data: [DONE]\n\n")
                else:
                    self.send_header("content-type","application/json")
                    self.end_headers()
                    self.wfile.write(json.dumps({"id":"hermes-native","created":1,"object":"chat.completion","model":"fixture",
                        "choices":[{"index":0,"message":{"role":"assistant","content":answer},"finish_reason":"stop"}],
                        "usage":usage}).encode())

        with ThreadingHTTPServer(("127.0.0.1",0), Provider) as server:
            endpoint = f"http://127.0.0.1:{server.server_port}/v1"
            thread = Thread(target=server.serve_forever, daemon=True)
            thread.start()
            try:
                home = root/"home"
                home.mkdir()
                (home/"config.yaml").write_text(json.dumps({"context":{"engine":"sic_context","require_engine":True},
                    "model":{"default":"fixture","provider":"custom","base_url":endpoint,"context_length":65536},
                    "compression":{"enabled":True,"in_place":True},
                    "memory":{"memory_enabled":False,"user_profile_enabled":False}}))
                env = {**{key:value for key,value in os.environ.items() if not key.lower().endswith("_proxy")},
                       "HOME":str(home),"HERMES_HOME":str(home),"PYTHONPATH":str(source),"NO_PROXY":"127.0.0.1,localhost",
                       "SIC_HERMES_CORE_URL":core.endpoint,"SIC_HERMES_CHAIN":chain,"SIC_HERMES_HARNESS_TOKEN":secret,
                       "SIC_HERMES_JOURNAL":str(root/"sic-journal.db"),"SIC_HERMES_BATCH":"2",
                       "OPENAI_API_KEY":"loopback","SIC_HERMES_TEST_PROVIDER":endpoint,
                       "SIC_HERMES_TEST_HISTORY":str(root/"native-history.db")}
                for phase in (mode,"restart"):
                    runner = subprocess.run([python,"-c",NATIVE], cwd=root, env={**env,"SIC_HERMES_TEST_PHASE":phase},
                        stdin=subprocess.DEVNULL,capture_output=True,text=True,encoding="utf-8",timeout=100)
                    assert runner.returncode == 0, (runner.stderr[-7000:],runner.stdout[-3500:],len(editors),len(workers))
                cut = core("cut.get", {}, chain)
                assert cut["sequence"] and editors
                if mode == "manual":
                    assert max(cb["level"] for cb in cut["view"]["cbs"]) == 3
                last = json.dumps(workers[-1]["messages"])
                assert all(cb["body"] in last for cb in cut["view"]["cbs"])
                assert "Original witness 0" not in last and "Continue from the persisted native Hermes transcript." in last
                return {"result":"PASS","mode":mode,"cut_sequence":cut["sequence"],"editor_calls":len(editors),"paid_requests":0}
            finally:
                server.shutdown()
                thread.join(timeout=5)


if __name__ == "__main__":
    options = argparse.ArgumentParser(description=__doc__)
    options.add_argument("python", type=Path)
    options.add_argument("source", type=Path)
    options.add_argument("core", type=Path)
    options.add_argument("--mode", choices=("manual", "automatic"), default="manual")
    args = options.parse_args()
    with tempfile.TemporaryDirectory(prefix="sic-hermes-native-") as path:
        print(json.dumps(exercise(args.python,args.source,args.core,Path(path),args.mode),indent=2))
