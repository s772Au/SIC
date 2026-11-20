"""Check the native Claude Code compact result and the configured gateway boundary."""
import argparse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import re
import secrets
import shlex
import subprocess
import sys
import tempfile
from threading import Thread
from urllib.error import HTTPError
from urllib.request import ProxyHandler, Request, build_opener
from uuid import uuid4

from pi_compaction_acceptance import send_text_stream
from pi_tool_wait_acceptance import core_service


def run(cli, root, automatic, make_projection=None, launcher_args=None):
    requests = []
    original = "NATIVE_CLAUDE_ORIGINAL_WITNESS"
    checkpoint = "NATIVE_CLAUDE_GATEWAY_COMPACTION_WITNESS"
    compaction_input = "NATIVE_CLAUDE_PRECOMPACT_WITNESS"
    phase = "first"
    journal = root / "hooks.jsonl"
    projection = None
    hook_token = secrets.token_urlsafe(24)
    editor_calls = []
    errors = []

    def hook_events():
        return [json.loads(line) for line in journal.read_text(encoding="utf-8").splitlines()] if journal.exists() else []

    class Provider(BaseHTTPRequestHandler):
        def log_message(self, *_):
            pass

        def do_POST(self):
            frame = json.loads(self.rfile.read(int(self.headers["content-length"])))
            if self.path == "/sic/arm":
                if not projection or self.headers.get("authorization") != "Bearer " + hook_token:
                    self.send_error(403)
                    return
                instruction = projection.arm(frame["session_id"],frame["trigger"])
                self.send_response(200)
                self.end_headers()
                self.wfile.write(instruction.encode())
                return
            if "/v1/messages" not in self.path:
                self.send_error(404)
                return
            if make_projection and self.headers.get("x-api-key") != "loopback-fixture":
                self.send_error(403)
                return
            if launcher_args and frame.get("messages"):
                last = frame["messages"][-1].get("content")
                if isinstance(last, str) and '"kind":"CLAUDE_SIC_EDITOR"' in last:
                    assert frame["system"].startswith("You are the SIC Editor.")
                    assert "tools" not in frame and not frame.get("stream")
                    answer = editor(frame)
                else:
                    answer = "NATIVE_CLAUDE_WORKER_DONE"
            elif projection:
                try:
                    kind, selected = projection.route(frame)
                    if kind == "compact":
                        answer = selected
                    else:
                        frame = selected
                        answer = "NATIVE_CLAUDE_WORKER_DONE"
                except Exception as error:
                    detail = str(error)
                    if isinstance(error, HTTPError):
                        detail = error.read(1800).decode("utf-8","replace")
                    errors.append((type(error).__name__,detail))
                    self.send_error(400,"SIC operation rejected")
                    return
            else:
                answer = checkpoint if compaction_input in json.dumps(frame) else "NATIVE_CLAUDE_WORKER_DONE"
            requests.append((phase, self.path, frame))
            if automatic and phase == "seed":
                answer += "\n" + "Finalized work was verified using the recorded native evidence. " * 1100
            tokens = 35000 if automatic and phase == "first" else 150
            self.send_response(200)
            message_id = "msg_" + uuid4().hex
            if frame.get("stream"):
                self.send_header("content-type", "text/event-stream")
                self.end_headers()
                send_text_stream(self.wfile.write, "anthropic-messages", frame["model"], answer, tokens,
                                 message_id=message_id)
            else:
                self.send_header("content-type", "application/json")
                self.end_headers()
                self.wfile.write(json.dumps({"id":message_id,"type":"message","role":"assistant",
                    "model":frame["model"],"content":[{"type":"text","text":answer}],"stop_reason":"end_turn",
                    "stop_sequence":None,"usage":{"input_tokens":tokens,"output_tokens":7}}).encode())

    with ThreadingHTTPServer(("127.0.0.1", 0), Provider) as server:
        thread = Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            pid = str(uuid4())
            if make_projection:
                def editor(frame):
                    assert "CLAUDE_SIC_EDITOR" in frame["messages"][-1]["content"]
                    editor_calls.append(frame)
                    return json.dumps({"summary":f"Claude verified evidence {len(editor_calls)}"})
                projection = make_projection(pid,editor)
            private = root / "home"
            private.mkdir()
            project = root / "project"
            project.mkdir()
            observer = root / "observe.py"
            hook = "" if launcher_args else (
                "import os, urllib.request\n"
                "if event['hook_event_name'] == 'PreCompact':\n"
                "    req = urllib.request.Request(os.environ['SIC_CLAUDE_TEST_ARM'], data=json.dumps(event).encode(), "
                "headers={'authorization':'Bearer '+os.environ['SIC_CLAUDE_TEST_HOOK_TOKEN']})\n"
                "    print(urllib.request.urlopen(req, timeout=10).read().decode())\n"
                if make_projection else f"if event['hook_event_name'] == 'PreCompact': print({compaction_input!r})\n")
            observer.write_text(
                "import json, pathlib, sys\nevent = json.load(sys.stdin)\n"
                "with pathlib.Path(sys.argv[1]).open('a', encoding='utf-8') as journal:\n"
                "    journal.write(json.dumps(event) + '\\n')\n"
                + hook, encoding="utf-8")
            command = " ".join(shlex.quote(str(path)) for path in (Path(sys.executable), observer, journal))
            trigger = "auto" if automatic else "manual"
            hooks = {event:[{"matcher":trigger,"hooks":[{"type":"command","command":command}]}]
                     for event in ("PreCompact", "PostCompact")}
            settings = root / "claude-settings.json"
            debug = root / "claude-native-debug.log"
            if launcher_args:
                profile = root / "launcher" / "claude"
                profile.mkdir(parents=True)
                settings = profile / "settings.json"
            settings.write_text(json.dumps({"hooks":hooks}), encoding="utf-8")
            endpoint = f"http://127.0.0.1:{server.server_port}"
            env = {key:value for key,value in os.environ.items()
                   if not key.startswith(("ANTHROPIC_", "CLAUDE")) and not key.lower().endswith("_proxy")}
            env.update({"HOME":str(private),"CLAUDE_CONFIG_DIR":str(private/"claude"),"ANTHROPIC_BASE_URL":endpoint,
                        "ANTHROPIC_API_KEY":"loopback-fixture","DISABLE_TELEMETRY":"1",
                        "DISABLE_ERROR_REPORTING":"1","CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC":"1",
                        "NO_PROXY":"127.0.0.1,localhost"})
            env.update({"SIC_CLAUDE_TEST_ARM":endpoint+"/sic/arm","SIC_CLAUDE_TEST_HOOK_TOKEN":hook_token})
            def native(message, *session):
                active_env = {**env, "CLAUDE_AUTOCOMPACT_PCT_OVERRIDE":"2"} if automatic and phase == "compact" else env
                native_args = ["-p","--output-format","json","--model","sonnet","--debug-file",debug]
                command = [cli,"--setting-sources","user","--settings",settings,*session,*native_args]
                if launcher_args:
                    entry, core_endpoint, chain, grant = launcher_args
                    command = [sys.executable,entry,"--claude",cli,"--core",core_endpoint,"--provider",endpoint,
                               "--chain",chain,"--session",pid,"--state",root/"launcher","--batch","2"]
                    if session[0] == "--resume":
                        command.append("--resume")
                    command.extend(["--",*native_args])
                    active_env = {**active_env,"SIC_CLAUDE_CORE_GRANT":grant,"SIC_CLAUDE_PROVIDER_KEY":"loopback-fixture"}
                try:
                    reply = subprocess.run(command,
                        cwd=project,env=active_env,capture_output=True,text=True,input=message,timeout=45,encoding="utf-8")
                except subprocess.TimeoutExpired as error:
                    diagnostic = debug.read_text(encoding="utf-8",errors="replace")[-4800:] if debug.is_file() else ""
                    raise RuntimeError((phase,errors,diagnostic)) from error
                assert reply.returncode == 0, (reply.stdout[-2200:],reply.stderr[-2000:],errors)
                return json.loads(reply.stdout)

            if automatic:
                phase = "seed"
                native(original, "--session-id", pid)
                for index in range(7):
                    native(f"Older finalized item {index}.", "--resume", pid)
                phase = "first"
                witness = "Retain evidence and completed work before next turn. " * 3300
                native(witness, "--resume", pid)
            else:
                assert "NATIVE_CLAUDE_WORKER_DONE" in native(original, "--session-id", pid)["result"]
            phase = "compact"
            response = native("Continue after the first turn." if automatic else "/compact", "--resume", pid)
            events = hook_events()
            kinds = [event["hook_event_name"] for event in events]
            if not kinds or kinds[0] != "PreCompact" or kinds.count("PostCompact") != 1:
                relevant = [line[-460:] for line in debug.read_text(encoding="utf-8", errors="replace").splitlines()
                            if re.search(r"compact|summariz", line, re.I)] if debug.is_file() else []
                messages = Path(events[-1]["transcript_path"]) if events else None
                assistants = sum(json.loads(line).get("type") == "assistant"
                                 for line in messages.read_text(encoding="utf-8").splitlines()) if messages and messages.is_file() else 0
                print(json.dumps({"result":"INCOMPLETE", "trigger":trigger,
                    "hooks":kinds, "model_reported_worker_result":response.get("result"),
                    "requests":[{"stage":stage,"path":path,"precompact_instruction":compaction_input in json.dumps(frame),
                        "on_demand_compaction":frame.get("compaction"),
                        "context_management_edits":[edit.get("type") for edit in frame.get("context_management", {}).get("edits", [])]}
                        for stage,path,frame in requests], "persisted_assistant_records":assistants,
                        "gateway_rejections":errors, "debug":relevant[-24:], "paid_requests":0}, indent=2))
                return False
            assert all(event["session_id"] == pid and event["trigger"] == trigger for event in events)
            completed = next(event for event in events if event["hook_event_name"] == "PostCompact")
            if make_projection:
                assert "SIC_CLAUDE_CHECKPOINT_" in completed["compact_summary"] and editor_calls
            else:
                assert checkpoint in completed["compact_summary"]
            assert any(stage == "compact" for stage, _, _ in requests) or (launcher_args and editor_calls)
            phase = "resumed"
            if make_projection:
                projection = make_projection(pid,editor)
            native("Observe the latest native checkpoint.", "--resume", pid)
            continuation = next(frame for stage, _, frame in requests if stage == "resumed")
            if make_projection:
                selected = projection.cut()
                assert all(cb["body"] in json.dumps(continuation["messages"]) for cb in selected["view"]["cbs"])
                assert original not in json.dumps(continuation["messages"])
                if not automatic:
                    for index in range(1,5):
                        phase = "growth"
                        native(f"Scope {index}. " + "Preserve verified and finalized evidence. " * 35,"--resume",pid)
                        phase = "compact"
                        native("/compact","--resume",pid)
                        prior_cut = selected["cut_id"]
                        selected = projection.cut()
                        assert selected["cut_id"] != prior_cut,(selected,errors)
                        phase = "resumed"
                        projection = make_projection(pid,editor)
                        tail = f"Continue the new native tail {index}."
                        native(tail,"--resume",pid)
                        worker_request = json.dumps(requests[-1][2]["messages"])
                        assert tail in worker_request and all(cb["body"] in worker_request for cb in selected["view"]["cbs"])
                    assert max(cb["level"] for cb in selected["view"]["cbs"]) == 3
                print(json.dumps({"result":"PASS","native_compaction":trigger,"core_cut":selected["cut_id"],
                    "editor_calls":len(editor_calls),"selected_levels":[cb["level"] for cb in selected["view"]["cbs"]],
                    "runnable_gateway":bool(launcher_args),
                    "native_checkpoint_seen_after_resume":True,"paid_requests":0}, indent=2))
                return True
            assert checkpoint in json.dumps(continuation["messages"])
            print(json.dumps({"result":"PASS","native_pre_and_post_hooks":trigger,"gateway_receives_compaction":True,
                              "native_checkpoint_seen_after_resume":True,"paid_requests":0}, indent=2))
            return True
        finally:
            server.shutdown()
            thread.join(timeout=5)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("cli", type=Path)
    parser.add_argument("--automatic", action="store_true")
    parser.add_argument("--core", type=Path)
    parser.add_argument("--launcher", action="store_true")
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="sic-claude-compaction-") as directory:
        root = Path(directory)
        if not args.core:
            if args.launcher:
                parser.error("--launcher requires --core")
            sys.exit(0 if run(args.cli.absolute(),root,args.automatic) else 1)
        sys.path.insert(0,str(Path(__file__).resolve().parents[1]))
        from claude.sic_messages import ClaudeCompaction
        with core_service(args.core.absolute(),root) as core:
            chain = core("chain.create",{})["chain_id"]
        grant = secrets.token_urlsafe(24)
        with core_service(args.core.absolute(),root,[(chain,grant)]) as core:
            def operate(operation,payload):
                request = Request(core.endpoint+"/api/v2/harness/operations",
                    data=json.dumps({"operation":operation,"payload":payload}).encode(),
                    headers={"authorization":"Bearer "+grant,"content-type":"application/json"})
                with build_opener(ProxyHandler({})).open(request,timeout=10) as response:
                    return json.load(response)

            def bind(session,editor):
                journal = root/"launcher"/"sic.db" if args.launcher else root/"claude-sic.db"
                journal.parent.mkdir(parents=True,exist_ok=True)
                return ClaudeCompaction(journal,session,chain,operate,editor,batch=2)

            launcher = None
            if args.launcher:
                launcher = (Path(__file__).resolve().parents[1]/"claude"/"launch.py",core.endpoint,chain,grant)
            sys.exit(0 if run(args.cli.absolute(),root,args.automatic,bind,launcher) else 1)
