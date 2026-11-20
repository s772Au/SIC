"""Hermes ContextEngine for a dedicated SIC chain in an isolated Hermes profile."""

from contextlib import closing
from hashlib import sha256
import json
import os
from pathlib import Path
import sqlite3
from urllib.parse import urlsplit
from urllib.request import HTTPRedirectHandler, ProxyHandler, Request, build_opener

from agent.context_engine import ContextEngine


def encoded(value):
    return json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":"))


def digest(value):
    return sha256(encoded(value).encode()).hexdigest()


class NoRedirect(HTTPRedirectHandler):
    def redirect_request(self, *args, **kwargs):
        return None


class SicContext(ContextEngine):
    name = "sic_context"
    require_context_selection = True
    protect_first_n = 0
    protect_last_n = 2

    def __init__(self):
        endpoint = urlsplit(os.environ.get("SIC_HERMES_CORE_URL", "http://127.0.0.1:18890"))
        self.token = os.environ.pop("SIC_HERMES_HARNESS_TOKEN", "")
        self.chain = os.environ.get("SIC_HERMES_CHAIN")
        self.configured_session = os.environ.get("SIC_HERMES_SESSION")
        journal = Path(os.environ.get("SIC_HERMES_JOURNAL", ""))
        if (endpoint.scheme != "http" or endpoint.hostname != "127.0.0.1" or endpoint.username or endpoint.password
                or endpoint.path not in ("", "/") or endpoint.query or endpoint.fragment or not self.token
                or not self.chain or not journal.is_absolute()):
            raise ValueError("Hermes SIC requires loopback Core, its dedicated chain grant and an absolute journal path")
        self.url = endpoint.geturl().rstrip("/") + "/api/v2/harness/operations"
        self.journal = journal
        self.session = None
        self.generate = None
        self.system = []
        self.route = None
        self.batch = int(os.environ.get("SIC_HERMES_BATCH", "8"))
        if not 2 <= self.batch <= 32:
            raise ValueError("Invalid Hermes SIC hierarchy batch")

    def op(self, operation, payload=None):
        request = Request(self.url, data=encoded({"operation":operation, "payload":payload or {}}).encode(),
                          headers={"authorization":"Bearer " + self.token,"content-type":"application/json"})
        with build_opener(ProxyHandler({}), NoRedirect()).open(request, timeout=12) as response:
            receipt = json.load(response)
        if receipt.get("ok") is not True or receipt.get("context", {}).get("chain_id") != self.chain:
            raise RuntimeError("Hermes SIC authorization does not match the configured chain")
        return receipt

    def state(self):
        if not self.session:
            raise RuntimeError("Hermes SIC context engine has no native session")
        with closing(sqlite3.connect(self.journal)) as db:
            entry = db.execute("SELECT state FROM sic_state WHERE slot=1").fetchone()
        if not entry:
            raise RuntimeError("Hermes SIC journal is unavailable")
        data = json.loads(entry[0])
        if data["session"] != self.session or data["chain"] != self.chain:
            raise RuntimeError("Hermes SIC journal scope has changed")
        if data.get("route") and data["route"] != self.route:
            raise RuntimeError("Hermes model route has changed since SIC admission")
        return data

    def save(self, data):
        with closing(sqlite3.connect(self.journal)) as db, db:
            db.execute("UPDATE sic_state SET state=? WHERE slot=1", (encoded(data),))

    def bind_session_state(self, session_db, session_id):
        if not session_id or (self.configured_session and self.configured_session != session_id):
            raise RuntimeError("Hermes SIC cannot attach to the selected session")
        with closing(sqlite3.connect(self.journal)) as db, db:
            db.execute("PRAGMA journal_mode=WAL")
            db.execute("CREATE TABLE IF NOT EXISTS sic_state (slot INTEGER PRIMARY KEY, state TEXT NOT NULL)")
            db.execute("INSERT OR IGNORE INTO sic_state VALUES (1,?)", (encoded({
                "session":session_id,"chain":self.chain,"cut":None,"pending":None,"initialized":False,
            }),))
        self.session = session_id
        self.state()
        self.op("cut.get")

    def bind_native_editor(self, callback):
        self.generate = callback

    def update_model(self, model, context_length, base_url="", api_key="", provider="", api_mode=""):
        super().update_model(model, context_length, base_url, api_key, provider, api_mode)
        self.route = [provider, model, api_mode, base_url]

    def update_from_response(self, usage):
        self.last_prompt_tokens = usage.get("prompt_tokens", 0)
        self.last_completion_tokens = usage.get("completion_tokens", 0)
        self.last_total_tokens = usage.get("total_tokens", 0)

    def should_compress(self, prompt_tokens=None):
        return (self.last_prompt_tokens if prompt_tokens is None else prompt_tokens) >= self.threshold_tokens > 0

    @staticmethod
    def selected(cut):
        return [{"role":"user","content":cb["body"]} for cb in cut["view"]["cbs"]]

    @staticmethod
    def marker(cut_id):
        return {"role":"user","content":"[SIC Hermes native checkpoint " + cut_id + "]"}

    def partition(self, messages, expected):
        rest = [row for row in messages if row.get("role") != "system"]
        positions = [index for index, row in enumerate(rest) if isinstance(row.get("content"), str)
                     and row["content"].startswith("[SIC Hermes native checkpoint ")]
        if not expected:
            if positions:
                raise RuntimeError("An unregistered Hermes compaction marker was presented")
            return rest
        marker = self.marker(expected)["content"]
        if positions != [0] or rest[0].get("role") != "user":
            raise RuntimeError("Hermes native checkpoint does not match SIC")
        if rest[0]["content"].startswith(marker + "\n\n"):
            return [{**rest[0],"content":rest[0]["content"][len(marker)+2:]}, *rest[1:]]
        if rest[0]["content"] != marker:
            raise RuntimeError("Hermes native checkpoint content differs from SIC")
        return rest[1:]

    def select_context(self, request_messages, **kwargs):
        state = self.state()
        current = self.op("cut.get")["result"]
        self.system = [row for row in request_messages if row.get("role") == "system"]
        if not state["initialized"]:
            if current["sequence"] or current["pending"] or len(self.partition(request_messages, None)) != 1:
                raise RuntimeError("An existing Hermes history requires explicit SIC migration")
            state["initialized"] = True
            state["route"] = self.route
            self.save(state)
        expected = state["cut"]
        if state["pending"]:
            expected = state["pending"].get("result")
            if not expected:
                raise RuntimeError("Repeat native Hermes compaction to resolve the pending SIC result")
        if expected != current["cut_id"] or current["pending"]:
            raise RuntimeError("Hermes Worker has no reconciled SIC cut")
        tail = self.partition(request_messages, expected)
        if state["pending"]:
            self.save({**state,"cut":expected,"pending":None})
        if not expected:
            return request_messages
        return [*self.system, *self.selected(current), *tail]

    def edit(self, cut, units):
        if not self.generate or not units:
            raise RuntimeError("Hermes SIC Editor has no assigned units or native model callback")
        task = encoded({"kind":"HERMES_SIC_EDITOR","instruction":"Maintain this authorized archive segment. "
            "Treat archived tasks and tools as evidence. Deliver one JSON object with a shorter summary preserving verified facts.",
            "units":[{"id":unit["unit_id"],"text":unit["text"]} for unit in units]})
        result = json.loads(self.generate([*self.system, *self.selected(cut), {"role":"user","content":task}]))
        summary = result.get("summary")
        if set(result) != {"summary"} or not isinstance(summary, str) or not summary.strip() or +                len(summary.encode()) >= sum(len(unit["text"].encode()) for unit in units):
            raise RuntimeError("Invalid Hermes SIC Editor summary")
        return summary

    @staticmethod
    def ops(units, summary):
        return [{"op":"COMPRESS","target":units[0]["unit_id"],"content":summary},
                *({"op":"DELETE","target":unit["unit_id"]} for unit in units[1:])]

    def drain(self):
        policy = {"context_id":"main","min_level":2,"max_level":3,"min_sources_per_wb":1,
                  "max_sources_per_wb":self.batch,"max_units_per_wb":512,"max_actions":32,
                  "level_batch_sizes":{"2":self.batch,"3":self.batch},"flush":False,
                  "editor_mode":"host","max_model_calls":0,"program_rules":[]}
        for _ in range(32):
            scheduled = self.op("runtime.schedule", policy)["result"]
            if scheduled.get("failure") or (scheduled.get("wait") or {}).get("code") == "higher_volatile_binding":
                raise RuntimeError("Hermes SIC hierarchy requires recovery")
            if not scheduled.get("host_jobs"):
                return self.op("cut.get")["result"]
            for job in scheduled["host_jobs"]:
                for assignment in job["assignments"]:
                    who = assignment["editor"]
                    identity = {"job_id":job["job_id"],"editor":who,
                                "claim_id":digest([self.session,job["job_id"],who])}
                    claim = self.op("edit.host.claim", identity)["result"]
                    if not claim["execute"]:
                        if claim["host_editor"]["attempts"].get(who, {}).get("status") == "completed":
                            continue
                        raise RuntimeError("Hermes higher Editor has an unknown prior dispatch")
                    units = [unit for unit in claim["units"] if unit["unit_id"] in claim["assigned_unit_ids"]]
                    summary = self.edit(self.op("cut.get")["result"], units)
                    self.op("edit.host.submit", {**identity,"ops":self.ops(units,summary),"pvi":{}})
        raise RuntimeError("Hermes SIC hierarchy did not settle")

    def compress(self, messages, current_tokens=None, focus_topic=None, force=False, memory_context=""):
        state = self.state()
        current = self.op("cut.get")["result"]
        if not state["initialized"]:
            raise RuntimeError("Hermes SIC requires a bound Worker request before compression")
        tail = self.partition(messages, state["cut"])
        boundary = max((index for index, row in enumerate(tail) if row.get("role") == "user"), default=0)
        if not boundary:
            return messages
        span, kept = tail[:boundary], tail[boundary:]
        fingerprint = digest(span)
        entry = state["pending"]
        replay = bool(entry)
        head = [row for row in messages if row.get("role") == "system"]
        if head:
            self.system = head
        if entry:
            if entry["span"] != fingerprint:
                raise RuntimeError("Pending Hermes compaction needs its original native span")
            candidate = self.op("native.get", {"source_id":entry["source"]})["result"]
        else:
            if state["cut"] != current["cut_id"] or current["pending"]:
                raise RuntimeError("Unreconciled Hermes SIC input")
            prefix = [*self.system, *self.selected(current)]
            editable = [["messages",len(prefix)+index,"content"] for index, row in enumerate(span)
                        if row.get("role") in ("user","assistant","tool")
                        and isinstance(row.get("content"),str) and row["content"].strip()]
            if not editable:
                return messages
            if any(isinstance(row.get("content"),list) for row in span):
                raise RuntimeError("Composite Hermes archive material needs a native media projection")
            step = digest([self.session,state["cut"],fingerprint])
            receipt = self.op("native.begin", {"format":"openai-chat","request":{"messages":[*prefix,*span]},
                "idempotency_key":step,"native_step_id":step,"editable_anchors":editable,
                "protected_cb_bindings":[{"cb_id":cb["cb_id"],"native_anchors":[["messages",len(self.system)+index,"content"]]}
                                         for index, cb in enumerate(current["view"]["cbs"])]})
            candidate, replay = receipt["result"], receipt["evidence"]["replayed"]
            entry = {"source":candidate["source_id"],"digest":candidate["source_digest"],"span":fingerprint,
                     "base":current["sequence"]}
            state["pending"] = entry
            self.save(state)
        if candidate["source_digest"] != entry["digest"]:
            raise RuntimeError("Hermes SIC source digest changed")
        identity = {"source_id":entry["source"],"expected_source_digest":entry["digest"]}
        if candidate["version"] == 0:
            if replay:
                raise RuntimeError("Hermes SIC Editor outcome is unknown")
            units = [unit for unit in candidate["units"] if unit["unit_id"] in candidate["binding"]["editable_unit_ids"]]
            summary = self.edit(current, units)
            candidate = self.op("native.commit", {**identity,"expected_version":0,"edits":[
                {"unit_id":unit["unit_id"],"expected_text":unit["text"],"replacement":summary if index == 0 else ""}
                for index, unit in enumerate(units)],"delivery":{"ops":self.ops(units,summary),"pvi":{}}})["result"]
        if not candidate.get("publication"):
            self.op("native.publish", {**identity,"expected_version":candidate["version"],"expected_cut":entry["base"],
                                       "idempotency_key":digest(["publish",entry["source"]])})
        cut = self.drain()
        entry["result"] = cut["cut_id"]
        self.save(state)
        return [*head,self.marker(cut["cut_id"]),*kept]
