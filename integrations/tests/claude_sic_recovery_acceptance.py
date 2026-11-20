"""Exercise Claude's compaction recovery and the live loopback authority boundary with a real Core."""

from pathlib import Path
import secrets
import sys
from tempfile import TemporaryDirectory
from urllib.error import HTTPError

from pi_tool_wait_acceptance import core_service

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "claude"))
from gateway import core_operator, listen, open_request
from sic_messages import ClaudeCompaction


def reject_request(url, frame, headers, expected):
    try:
        with open_request(url, frame, headers, 5):
            raise AssertionError("Foreign Claude request passed its gateway")
    except HTTPError as error:
        with error:
            assert error.code == expected


def exercise(root, tracked_op, chain, fault):
    journal = root / f"claude-{fault}.db"
    session = "fixture-native-session"
    seen = []
    reply_lost = False
    original = "Native authoritative evidence is archived before compaction. " * 15
    tool_text = "NATIVE_TOOL_OUTPUT_WITNESS"
    reasoning = "OPAQUE_NATIVE_REASONING_WITNESS"

    def operate(name, payload):
        nonlocal reply_lost
        response = tracked_op(name, payload)
        if name == fault and not reply_lost:
            reply_lost = True
            raise ConnectionError("Simulated lost Core receipt after a completed operation")
        return response

    def edit(frame):
        seen.append(frame)
        assert tool_text in frame["messages"][-1]["content"]
        assert reasoning not in str(frame)
        if fault == "editor":
            raise ConnectionError("Simulated unavailable Editor receipt")
        return '{"summary":"Archived user evidence and native tool result."}'

    first = ClaudeCompaction(journal, session, chain, operate, edit)
    forged = {"model": "fixture", "messages": [{"role": "user", "content": "unauthorized"}]}
    with listen(first, "http://127.0.0.1:1", "unused", "worker-test", "hook-test") as url:
        headers = {"X-Api-Key": "worker-test", "X-Claude-Code-Request-Class": "main"}
        reject_request(url + "/v1/messages", forged, headers, 403)
        headers["X-Claude-Code-Session-Id"] = session
        headers["X-Claude-Code-Request-Class"] = "compaction"
        reject_request(url + "/v1/messages", forged, headers, 409)
        reject_request(url + "/sic/arm", {"session_id": "foreign", "trigger": "auto"},
                       {"Authorization": "Bearer hook-test"}, 403)
    marker = first.arm(session, "manual")
    frame = {"model": "fixture", "system": "Keep the native Worker instructions.", "messages": [
        {"role": "user", "content": original},
        {"role": "assistant", "content": [
            {"type": "thinking", "thinking": reasoning, "signature": "fixture-signature"},
            {"type": "tool_use", "id": "toolu_1", "name": "Read", "input": {}},
        ]},
        {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "toolu_1",
                                       "content": [{"type": "text", "text": tool_text}]}]},
        {"role": "user", "content": marker},
    ]}
    try:
        first.route(frame, require_compaction=True)
    except ConnectionError:
        pass
    else:
        raise AssertionError("Fault injection did not interrupt compaction")

    restarted = ClaudeCompaction(journal, session, chain, tracked_op, edit)
    if fault == "editor":
        try:
            restarted.route(frame, require_compaction=True)
        except RuntimeError as error:
            assert "Editor outcome is unknown" in str(error)
        else:
            raise AssertionError("Unknown Editor dispatch was repeated")
        assert len(seen) == 1
        return
    mode, checkpoint = restarted.route(frame, require_compaction=True)
    assert mode == "compact" and len(seen) == 1 and restarted.cut()["sequence"] == 1
    source = restarted.op("native.get", {"source_id": restarted.state()["pending"]["source"]})["result"]
    assert reasoning in str(source["source_request"])
    continuation = {**frame, "messages": [{"role": "user", "content": [
        {"type": "text", "text": checkpoint}, {"type": "text", "text": "Preserve this native tail"},
    ]}]}
    mode, sent = restarted.route(continuation, require_compaction=False)
    assert mode == "worker" and sent["system"] == frame["system"]
    assert sent["messages"][0]["content"] == "Archived user evidence and native tool result."
    assert sent["messages"][-1]["content"][0]["text"] == "Preserve this native tail"
    assert "SIC_CLAUDE_CHECKPOINT_" not in str(sent)


def main(binary):
    with TemporaryDirectory(prefix="sic-claude-recovery-") as directory:
        root = Path(directory)
        keys = ("native.commit", "native.publish", "editor")
        with core_service(binary, root) as core:
            grants = [(core("chain.create", {})["chain_id"], secrets.token_urlsafe(24)) for _ in keys]
        with core_service(binary, root, grants) as core:
            for fault, (chain, grant) in zip(keys, grants, strict=True):
                exercise(root, core_operator(core.endpoint, grant), chain, fault)
        print("PASS Claude scope, tool evidence, opaque thinking, commit and publication replay, unknown Editor stop")


if __name__ == "__main__":
    main(Path(sys.argv[1]).resolve())
