"""Dry-plan or run selected real text datasets with fixed providers and bounded scheduling.

Default action is a read-only dry plan. --execute and --allow-network are both
required for a real provider. No resumes, hidden truncation or automatic retries.
"""
import argparse
from datetime import datetime, timezone
from collections import Counter
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import subprocess
import sys
import threading
import time
import urllib.error
import urllib.request
import zipfile
from urllib.parse import urlsplit

from benchmark_scoring import aggregate, babilong_score, judge_requests, parse_judgment, JUDGE_INSTRUCTION
from dataset_adapters import ROOT, VERSION, cases, source_entries, source_units, case_metadata, file_sha256
from run_experiment import digest, free_port, write_json
from sic_demo import SICClient


class DatasetMock(BaseHTTPRequestHandler):
    def do_POST(self):
        request = json.loads(self.rfile.read(int(self.headers["content-length"])))
        if request.get("tools"):
            identifier = request["tools"][0]["function"]["parameters"]["properties"]["edits"]["items"]["properties"]["target_id"]["enum"][0]
            message = {"role": "assistant", "tool_calls": [{"function": {"name": "sic_edit_wb",
                "arguments": json.dumps({"edits": [{"target_id": identifier, "op": "COMPRESS", "risk": "high", "content": "MOCK summary: plumbing only; original facts are not preserved."}]})}}]}
            reason = "tool_calls"
        else:
            content = '{"score":0,"reason":"Deterministic mock judge; not a quality result"}' if request["messages"][0].get("content") == JUDGE_INSTRUCTION else "MOCK answer: no quality inference"
            message, reason = {"role": "assistant", "content": content}, "stop"
        payload = json.dumps({"choices": [{"finish_reason": reason, "message": message}]}).encode()
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)

    def log_message(self, *_):
        pass


def endpoint_check(url):
    parsed = urlsplit(url or "")
    if parsed.scheme not in ("http", "https") or not parsed.hostname or not url.endswith("/chat/completions"):
        raise ValueError("full HTTP(S) /chat/completions endpoint required")
    if parsed.port == 8341 or parsed.username or parsed.password or parsed.query or parsed.fragment:
        raise ValueError("8341 and URL credentials/query/fragment are forbidden")


class Provider:
    def __init__(self, endpoint, model, key, output_tokens, output, request_limit):
        self.endpoint, self.model, self.key = endpoint, model, key
        self.output_tokens, self.output, self.request_limit = output_tokens, output, request_limit
        self.opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirect())
        self.calls = 0

    def call(self, messages, label):
        started = time.monotonic()
        self.calls += 1
        request = {"model": self.model, "messages": messages, "stream": False, "temperature": 0, "max_tokens": self.output_tokens}
        raw = json.dumps(request, ensure_ascii=False, separators=(",", ":")).encode()
        evidence = {"label": label, "model": self.model, "request_bytes": len(raw), "request_sha256": digest(raw), "status": "prepared"}
        path = self.output / f"call-{self.calls:06d}-{label}.json"
        try:
            if len(raw) > self.request_limit:
                raise ValueError("request exceeds configured byte limit; not truncated or delivered")
            http_request = urllib.request.Request(self.endpoint, raw, {"content-type": "application/json", "authorization": "Bearer " + self.key})
            try:
                with self.opener.open(http_request, timeout=330) as response:
                    response_raw = response.read(16 * 1024 * 1024 + 1)
                    evidence["http_status"] = response.status
            except urllib.error.HTTPError as error:
                response_raw = error.read(16 * 1024 * 1024 + 1)
                evidence["http_status"] = error.code
            if len(response_raw) > 16 * 1024 * 1024:
                raise ValueError("provider response exceeds byte limit")
            evidence["response_sha256"] = digest(response_raw)
            evidence["response_text"] = response_raw.decode("utf-8", errors="replace")
            if not 200 <= evidence["http_status"] < 300:
                raise ValueError("provider rejected request (see call evidence)")
            reply = json.loads(response_raw)
            if reply["choices"][0].get("finish_reason") != "stop":
                raise ValueError("answer/judge incomplete or tool response")
            answer = reply["choices"][0]["message"]["content"]
            if not isinstance(answer, str):
                raise ValueError("non-text provider response")
            evidence["status"] = "answered"
            return answer
        except Exception as error:
            evidence["status"], evidence["error"] = "failed", type(error).__name__ + ": " + str(error)
            raise
        finally:
            evidence["elapsed_seconds"] = round(time.monotonic() - started, 6)
            write_json(path, evidence)


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, *_):
        return None


def make_plan(args):
    sources, metadata = [], []
    selected_source_ids = set()
    for dataset in args.dataset:
        for case in cases(dataset, args.limit_cases, args.question_limit):
            meta = case_metadata(case, args.chunk_bytes)
            metadata.append(meta)
            selected_source_ids.add(meta["locator"]["source_id"])
        for entry in source_entries(dataset):
            if entry["id"] not in selected_source_ids:
                continue
            path = Path(entry["resolved_path"])
            actual_size = path.stat().st_size
            if actual_size != entry["bytes"]:
                raise ValueError("source size mismatch: " + entry["id"])
            verified = args.verify_sources or args.execute
            if verified and file_sha256(path) != entry["sha256"]:
                raise ValueError("source SHA-256 mismatch: " + entry["id"])
            public_entry = {key: value for key, value in entry.items() if key != "resolved_path"}
            sources.append(dict(public_entry, actual_bytes=actual_size, hash_verified=verified))
    if not metadata:
        raise ValueError("no selected cases")
    identities = [(case["dataset"], case["id"]) for case in metadata]
    if len(set(identities)) != len(identities):
        raise ValueError("duplicate dataset/case identity")
    for case in metadata:
        questions = case["questions"]
        if not case["context_bytes"] or not questions or len({q["id"] for q in questions}) != len(questions):
            raise ValueError("empty context/questions or duplicate question identity")
        for question in questions:
            rubric = question["rubric"]
            if not isinstance(question["question"], str) or not question["question"] or not isinstance(question["tail"], str):
                raise ValueError("invalid question text")
            if rubric is not None and (not isinstance(rubric, list) or not rubric or not all(isinstance(item, str) and item for item in rubric)):
                raise ValueError("invalid or empty BEAM rubric")
    qcount = sum(len(c["questions"]) for c in metadata)
    ccalls = sum(c["wb_chunks"] for c in metadata) if "sic" in args.arms else 0
    jcalls = sum(sum(len(q["rubric"]) if q["rubric"] is not None else 1 for q in c["questions"] if not c["dataset"].startswith("babilong")) for c in metadata) * len(args.arms)
    return {"schema": "sic-real-text-plan-v1", "adapter_version": VERSION,
            "sources": sources, "cases": metadata, "arms": args.arms,
            "chunk_bytes": args.chunk_bytes, "scheduling": "sequential-fixed-order-no-retry",
            "call_estimate": {"compressor": ccalls, "answer": qcount * len(args.arms), "judge": jcalls,
                              "total": ccalls + qcount * len(args.arms) + jcalls},
            "max_provider_request_bytes": args.max_request_bytes, "token_count": "unknown; no truncation or model capacity admission",
            "scoring": "pinned BABILong label metric + strict exact; local BEAM rubric and CorpusQA fixed-judge v1 (not official scores)",
            "quality_claim": False, "physical_adoption": "unknown"}


def run(args, plan):
    args.out.mkdir(parents=True, exist_ok=False)
    write_json(args.out / "plan.json", plan)
    snapshot = args.out / "code-snapshot.zip"
    snapshot_paths = ["scripts/dataset_runner.py", "scripts/dataset_adapters.py", "scripts/benchmark_scoring.py",
        "scripts/verify_dataset_run.py", "scripts/sic_demo.py", "scripts/run_experiment.py",
        "benchmarks/sources-lock.json", "benchmarks/source_paths.py", "benchmarks/requirements.txt", "benchmarks/vendor/sources.json",
        "benchmarks/vendor/babilong_metrics.py", "benchmarks/vendor/BABILONG-LICENSE", "benchmarks/test-catalog.json", "src/main.rs", "Cargo.toml", "Cargo.lock"]
    with zipfile.ZipFile(snapshot, "w", zipfile.ZIP_DEFLATED) as archive:
        for relative in snapshot_paths:
            path = ROOT / relative
            if path.is_file():
                archive.write(path, relative)
    mock = process = None
    records, consumed = [], 0
    try:
        if args.provider == "mock":
            mock = ThreadingHTTPServer(("127.0.0.1", 0), DatasetMock)
            threading.Thread(target=mock.serve_forever, daemon=True).start()
            endpoint = f"http://127.0.0.1:{mock.server_port}/v1/chat/completions"
            model, key, judge_model = "deterministic-plumbing-mock-v1", "local-mock", "deterministic-judge-mock-v1"
        else:
            endpoint, model, key, judge_model = args.endpoint, args.model, os.environ[args.api_key_env], args.judge_model
        port = free_port()
        token = os.urandom(24).hex()
        env = {k: v for k, v in os.environ.items() if not k.startswith("SIC_")}
        env.update(SIC_BIND=f"127.0.0.1:{port}", SIC_ADMIN_TOKEN=token,
            SIC_DB=f"sqlite:///{(args.out.resolve() / 'run.db').as_posix()}?mode=rwc",
            SIC_COMPRESSER_URL=endpoint, SIC_COMPRESSER_MODEL=model, SIC_COMPRESSER_API_KEY=key,
            SIC_COMPRESSER_MAX_TOKENS=str(args.max_output_tokens), SIC_COMPRESSER_MAX_REQUEST_BYTES=str(args.max_request_bytes),
            SIC_WORKER_UPSTREAM_BASE_URL=endpoint.removesuffix("/chat/completions"), SIC_WORKER_UPSTREAM_API_KEY=key)
        binary_name = "sic-proxy.exe" if os.name == "nt" else "sic-proxy"
        exe = next((p for p in [ROOT / "bin" / binary_name, ROOT / "target/release" / binary_name, ROOT / "target/debug" / binary_name] if p.exists()), None)
        if exe is None:
            raise ValueError("build SIC first")
        write_json(args.out / "manifest.json", {"schema": "sic-real-text-run-v1", "provider": args.provider,
            "created_at_utc": datetime.now(timezone.utc).isoformat(),
            "code_snapshot_sha256": file_sha256(snapshot),
            "endpoint": endpoint, "answer_compressor_model": model, "judge_model": judge_model,
            "max_output_tokens": args.max_output_tokens, "binary_sha256": file_sha256(exe),
            "plan_sha256": file_sha256(args.out / "plan.json"), "adapter_sha256": file_sha256(ROOT / "scripts/dataset_adapters.py"),
            "runner_sha256": file_sha256(Path(__file__)), "scorer_sha256": file_sha256(ROOT / "scripts/benchmark_scoring.py"),
            "client_sha256": file_sha256(ROOT / "scripts/sic_demo.py"), "sources_lock_sha256": file_sha256(ROOT / "benchmarks/sources-lock.json"),
            "metric_sources": json.loads((ROOT / "benchmarks/vendor/sources.json").read_text(encoding="utf-8")),
            "judge_prompt_sha256": digest(JUDGE_INSTRUCTION.encode()), "quality_claim": False, "official_benchmark_score": False})
        answerer = Provider(endpoint, model, key, args.max_output_tokens, args.out, args.max_request_bytes)
        judge = Provider(endpoint, judge_model, key, args.max_output_tokens, args.out, args.max_request_bytes)
        with (args.out / "service.log").open("wb") as log:
            process = subprocess.Popen([str(exe)], cwd=ROOT, env=env, stdout=log, stderr=log)
            client = SICClient(f"http://127.0.0.1:{port}", token)
            for _ in range(100):
                try:
                    client.call("GET", "/api/v1/ui/bootstrap")
                    break
                except OSError:
                    if process.poll() is not None:
                        raise RuntimeError("SIC exited during startup")
                    time.sleep(.1)
            else:
                raise RuntimeError("SIC startup timeout")
            case_index = 0
            for dataset in args.dataset:
                for case in cases(dataset, args.limit_cases, args.question_limit):
                    actual = case_metadata(case, args.chunk_bytes)
                    if actual != plan["cases"][case_index]:
                        raise ValueError("source selection changed since plan")
                    session, ingestion_error = None, None
                    if "sic" in args.arms:
                        session = client.call("POST", "/api/v1/sessions", {"name": f"dataset-{case_index}"})["id"]
                        ingestion = []
                        try:
                            for unit in source_units(case["context"], args.chunk_bytes):
                                consumed += 1
                                if consumed > args.max_model_calls:
                                    raise ValueError("model call budget exhausted")
                                result = client.ingest(session, [unit])
                                ingestion.append(result)
                                write_json(args.out / f"ingestion-{case_index:04d}.json", ingestion)
                        except Exception as error:
                            ingestion_error = type(error).__name__ + ": " + str(error)
                    for question in case["questions"]:
                        for arm in args.arms:
                            record = {"dataset": dataset, "case_id": case["id"], "question_id": question["id"],
                                "arm": arm, "reference": question["reference"], "scorer": question["scorer"], "status": "failed"}
                            try:
                                if arm == "sic" and ingestion_error:
                                    raise RuntimeError("ingestion failed: " + ingestion_error)
                                consumed += 1
                                if consumed > args.max_model_calls:
                                    raise ValueError("model call budget exhausted")
                                if arm == "baseline":
                                    messages = case["protected"] + [{"role": "user", "content": case["context"] + question["tail"]}]
                                    answer = answerer.call(messages, "answer")
                                else:
                                    request = {"model": model, "max_tokens": args.max_output_tokens, "temperature": 0, "stream": False,
                                               "messages": case["protected"] + [{"role": "user", "content": question["tail"]}]}
                                    preview = client.call("POST", f"/api/v1/sessions/{session}/worker/assemble-chat", {"request": request})
                                    record["local_projection"] = {k: v for k, v in preview.items() if k != "request"}
                                    reply, response_headers = client.call("POST", "/v1/chat/completions", request,
                                        {"x-sic-session-id": session, "x-sic-history-mode": "tail-only"}, with_headers=True)
                                    relay_id = response_headers.get("x-sic-relay-id")
                                    if not relay_id:
                                        raise ValueError("missing local relay evidence ID")
                                    record["relay_event"] = client.call("GET", f"/api/v1/relay-events/{relay_id}")
                                    record["worker_response"] = reply
                                    if reply["choices"][0].get("finish_reason") != "stop":
                                        raise ValueError("Worker answer incomplete")
                                    answer = reply["choices"][0]["message"]["content"]
                                    if not isinstance(answer, str):
                                        raise ValueError("non-text Worker answer")
                                record["answer"], record["status"] = answer, "answered-unscored"
                                if dataset.startswith("babilong"):
                                    record["score"] = babilong_score(question["reference"], answer, question["question"], question["category"])
                                else:
                                    judgments = []
                                    for messages in judge_requests(question["question"], answer, question["reference"], question["rubric"]):
                                        consumed += 1
                                        if consumed > args.max_model_calls:
                                            raise ValueError("judge call budget exhausted")
                                        judgments.append(parse_judgment(judge.call(messages, "judge")))
                                    record["score"] = {"metric": question["scorer"], "score": sum(j["score"] for j in judgments) / len(judgments),
                                                       "judgments": judgments, "official_benchmark_score": False}
                                record["status"] = "scored"
                            except Exception as error:
                                record["error"] = type(error).__name__ + ": " + str(error)
                            records.append(record)
                            write_json(args.out / "records.json", records)
                    case_index += 1
        write_json(args.out / "scores.json", aggregate(records))
        write_json(args.out / "completion.json", {"status": "complete" if all(r["status"] == "scored" for r in records) else "completed-with-failures",
            "finished_at_utc": datetime.now(timezone.utc).isoformat(),
            "attempted_model_steps": consumed, "quality_claim": False, "official_benchmark_score": False})
    finally:
        if process is not None:
            process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=5)
        if mock:
            mock.shutdown()
            mock.server_close()
        hashes = {p.name: file_sha256(p) for p in sorted(args.out.iterdir()) if p.is_file() and p.suffix in (".json", ".log", ".zip")}
        write_json(args.out / "checksums.json", hashes)
    return 0 if records and all(r["status"] == "scored" for r in records) else 1


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dataset", action="append", required=True)
    parser.add_argument("--data-root", type=Path, help="override source locations; mirror corpusqa/, beam10m/, babilong/ inside this folder")
    parser.add_argument("--limit-cases", type=int, default=1)
    parser.add_argument("--question-limit", type=int, default=2, help="BEAM questions per history; 0 means all")
    parser.add_argument("--arms", nargs="+", choices=["baseline", "sic"], default=["baseline", "sic"])
    parser.add_argument("--chunk-bytes", type=int, default=524288)
    parser.add_argument("--max-request-bytes", type=int, default=64 * 1024 * 1024)
    parser.add_argument("--max-output-tokens", type=int, default=4096)
    parser.add_argument("--max-model-calls", type=int, default=10)
    parser.add_argument("--verify-sources", action="store_true")
    parser.add_argument("--execute", action="store_true")
    parser.add_argument("--provider", choices=["mock", "chat-completions"], default="mock")
    parser.add_argument("--endpoint")
    parser.add_argument("--model")
    parser.add_argument("--judge-model")
    parser.add_argument("--api-key-env", default="SIC_EXPERIMENT_API_KEY")
    parser.add_argument("--allow-network", action="store_true")
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    if args.data_root:
        os.environ["SIC_DATA_ROOT"] = str(args.data_root.resolve())
    if args.limit_cases < 1 or args.question_limit < 0 or args.chunk_bytes < 4 or args.max_output_tokens < 1 or len(set(args.arms)) != len(args.arms) or len(set(args.dataset)) != len(args.dataset):
        parser.error("invalid limits or duplicate arms/datasets")
    if args.out.exists():
        parser.error("output already exists; no reuse or resume")
    if args.execute and args.provider == "chat-completions":
        try:
            endpoint_check(args.endpoint)
        except ValueError as error:
            parser.error(str(error))
        if not args.allow_network or not args.model or not os.environ.get(args.api_key_env):
            parser.error("real execution requires --allow-network, fixed --model and API key environment variable")
        if any(not d.startswith("babilong") for d in args.dataset) and not args.judge_model:
            parser.error("BEAM/CorpusQA require explicit fixed --judge-model")
    plan = make_plan(args)
    if not args.execute:
        args.out.mkdir(parents=True, exist_ok=False)
        write_json(args.out / "plan.json", plan)
        print(json.dumps({"output_directory": args.out.name, "cases": len(plan["cases"]), "calls": plan["call_estimate"], "executed": False}, indent=2))
        return 0
    if args.max_model_calls < plan["call_estimate"]["total"]:
        parser.error(f"planned {plan['call_estimate']['total']} calls exceed --max-model-calls; inspect a dry plan and raise the explicit budget")
    return run(args, plan)


if __name__ == "__main__":
    sys.exit(main())
