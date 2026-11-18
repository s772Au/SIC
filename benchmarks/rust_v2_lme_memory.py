"""Official LME Memory adapter for a caller-owned Rust v2 Core.

Only text-only trajectories are currently supported. Screenshot-bearing
trajectories fail before ingestion: the Rust Core does not yet preserve images
through its Editor and Worker. This adapter never drops those images silently.
"""
import hashlib
import json
import os
from pathlib import Path
import threading

from memory_modules.memory import Memory, register_memory
from scripts.rust_v2_test_runtime import RustClient, maintain_hierarchy


def text_trajectory_messages(trajectory):
    if not isinstance(trajectory, dict) or not isinstance(trajectory.get("id"), str) or not isinstance(trajectory.get("states"), list):
        raise ValueError("official trajectory requires an id and ordered states")
    if any(state.get("screenshot") is not None for state in trajectory["states"]):
        raise ValueError("Rust v2 media_not_implemented: trajectory screenshots cannot be preserved through Editor/Worker")
    messages = [{"role": "user", "content": json.dumps({"trajectory_id": trajectory["id"],
                 "domain": trajectory.get("domain"), "goal": trajectory.get("goal"),
                 "outcome": trajectory.get("outcome")}, ensure_ascii=False)}]
    previous = -1
    for state in trajectory["states"]:
        index = state.get("state_index")
        if type(index) is not int or index <= previous:
            raise ValueError("trajectory states must retain strict original order")
        previous = index
        text = json.dumps({key: state.get(key) for key in
                           ("state_index", "url", "accessibility_tree", "action", "thought")}, ensure_ascii=False)
        messages.append({"role": "user", "content": [{"type": "text", "text": text}]})
    return messages


@register_memory
class RustV2Memory(Memory):
    memory_type = "sic_rust_v2"

    def __init__(self, memory_params):
        super().__init__(memory_params)
        allowed = {"data_root", "editor_mode", "wb_state_count", "max_editor_calls", "hierarchy_fanout"}
        if set(memory_params) - allowed:
            raise ValueError("unknown Rust v2 LME memory parameter")
        self.editor_mode = memory_params.get("editor_mode", "program")
        if self.editor_mode not in ("program", "model"):
            raise ValueError("editor_mode must be program or model")
        self.wb_state_count = memory_params.get("wb_state_count", 1)
        self.max_editor_calls = memory_params.get("max_editor_calls", 0)
        self.hierarchy_fanout = memory_params.get("hierarchy_fanout", 0)
        if type(self.wb_state_count) is not int or self.wb_state_count < 1:
            raise ValueError("wb_state_count must be a positive integer")
        if type(self.max_editor_calls) is not int or self.max_editor_calls < 0:
            raise ValueError("max_editor_calls must be a nonnegative integer")
        if self.editor_mode == "model" and self.max_editor_calls == 0:
            raise ValueError("model editing requires an explicit positive max_editor_calls")
        if type(self.hierarchy_fanout) is not int or (self.hierarchy_fanout != 0 and not 2 <= self.hierarchy_fanout <= 64):
            raise ValueError("hierarchy_fanout must be 0 or an integer from 2 to 64")
        origin = os.environ.get("SIC_RUST_CORE_URL")
        token = os.environ.get("SIC_RUST_CORE_TOKEN")
        actor = os.environ.get("SIC_RUST_CORE_ACTOR", "benchmark")
        if not origin or not token:
            raise ValueError("set SIC_RUST_CORE_URL and SIC_RUST_CORE_TOKEN for an isolated Rust v2 service")
        self.client = RustClient(origin.rstrip("/"), token, actor)
        self.chain_id = self.client.call("chain.create")["chain_id"]
        self.client.call("config.update", {"editor_feedback": {"max_corrections": 0, "feedback_chars": 8192}}, self.chain_id)
        self.inserted, self.failed = {}, set()
        self.editor_calls = 0
        self._lock = threading.RLock()

    def insert(self, trajectory):
        # Validate the whole trajectory before publishing any of it.
        messages = text_trajectory_messages(trajectory)
        identity = trajectory["id"]
        fingerprint = hashlib.sha256(json.dumps(trajectory, ensure_ascii=False, sort_keys=True).encode()).hexdigest()
        with self._lock:
            if identity in self.inserted:
                if self.inserted[identity]["sha256"] != fingerprint:
                    raise ValueError("trajectory identity reused with changed content")
                return
            if identity in self.failed:
                raise ValueError("partially failed trajectory requires a fresh Rust chain")
            states = messages[1:]
            chunks = [([messages[0]] if start == 0 else []) + states[start:start + self.wb_state_count]
                      for start in range(0, len(states), self.wb_state_count)] or [messages]
            if self.editor_mode == "model" and self.editor_calls + len(chunks) > self.max_editor_calls:
                raise ValueError("trajectory exceeds the remaining explicit LME Editor-call limit")
            published = []
            try:
                for index, chunk in enumerate(chunks):
                    key = "lme-" + fingerprint + "-" + str(index)
                    source = self.client.call("source.ingest", {"request": {"messages": chunk}, "idempotency_key": key}, self.chain_id)["source_id"]
                    self.client.call("wb.form", {"source_ids": [source]}, self.chain_id)
                    if self.editor_mode == "model":
                        if self.editor_calls >= self.max_editor_calls:
                            raise ValueError("explicit LME Editor-call limit exhausted")
                        job = self.client.call("edit.prepare", {"source_id": source}, self.chain_id)
                        self.editor_calls += 1
                        result = self.client.call("edit.run", {"job_id": job["job_id"]}, self.chain_id, True)
                        if result.get("status") != "ready" or result.get("attempts") != 1:
                            raise ValueError("Editor did not complete in one attempt")
                    else:
                        job = self.client.call("edit.program", {"source_id": source, "rules": []}, self.chain_id)
                    seq = self.client.call("chain.get", chain_id=self.chain_id)["cut_seq"]
                    cut = self.client.call("cut.publish", {"job_id": job["job_id"], "expected_cut": seq,
                                           "idempotency_key": key}, self.chain_id)
                    published.append(cut["cb_id"])
                    maintain_hierarchy(self.client, self.chain_id, self.hierarchy_fanout)
            except Exception:
                self.failed.add(identity)
                raise
            self.inserted[identity] = {"sha256": fingerprint, "cb_ids": published}

    def query(self, query, query_image=None):
        if not isinstance(query, str):
            raise ValueError("query must be text")
        if query_image is not None:
            raise ValueError("Rust v2 media_not_implemented: question images cannot reach the Rust Worker")
        with self._lock:
            view = self.client.call("cut.get", chain_id=self.chain_id)["view"]
            return [{"type": "text", "value": cb["body"]} for cb in view["cbs"]]

    def post_query_hook(self, **_kwargs):
        return {"backend": "rust-v2", "memory_scope": "text-only; no query ranking",
                "question_image": "unsupported by the Rust v2 workflow",
                "research_evaluation": "not_evaluated"}

    def _save_backend(self, output_dir):
        with self._lock:
            if self.failed:
                raise ValueError("cannot save a memory with a partially failed trajectory")
            pack = self.client.call("experiment.pack", {}, self.chain_id)
            state = {"schema": "sic-rust-v2-lme-link-v1", "chain_id": self.chain_id,
                     "inserted": self.inserted, "editor_calls": self.editor_calls, "pack": pack,
                     "storage": "external Rust database must remain available"}
            (Path(output_dir) / "rust-v2-link.json").write_text(json.dumps(state, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")

    def _load_backend(self, input_dir):
        state = json.loads((Path(input_dir) / "rust-v2-link.json").read_text(encoding="utf-8"))
        if state.get("schema") != "sic-rust-v2-lme-link-v1":
            raise ValueError("invalid Rust v2 LME memory link")
        verification = self.client.call("experiment.verify", {"pack": state["pack"]}, state["chain_id"])
        if not verification.get("ok") or not verification.get("cut_seq", {}).get("matches"):
            raise ValueError("saved Rust memory no longer matches its database")
        self.chain_id, self.inserted = state["chain_id"], state["inserted"]
        self.editor_calls = state["editor_calls"]
