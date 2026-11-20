"""Launch a dedicated Claude Code session through SIC and a Messages provider."""

import argparse
from contextlib import closing
import json
import os
from pathlib import Path
import secrets
import shlex
import shutil
import sqlite3
import subprocess
import sys
import tempfile
from uuid import UUID

from gateway import core_operator, listen, open_request
from sic_messages import ClaudeCompaction


def precompact():
    event = json.load(sys.stdin)
    with open_request(os.environ["SIC_CLAUDE_HOOK_URL"], event,
                      {"Authorization": "Bearer " + os.environ["SIC_CLAUDE_HOOK_KEY"],
                       "Content-Type": "application/json"}, 20) as response:
        print(response.read().decode())


def child_command(path):
    return " ".join(shlex.quote(part) for part in (sys.executable, str(path), "precompact"))


def session_environment(state, base, worker_key, hook_key):
    local = os.environ.copy()
    for name in ("SIC_CLAUDE_CORE_GRANT", "SIC_CLAUDE_PROVIDER_KEY", "ANTHROPIC_AUTH_TOKEN",
                 "CLAUDE_CODE_USE_BEDROCK", "CLAUDE_CODE_USE_VERTEX", "CLAUDE_CODE_USE_FOUNDRY"):
        local.pop(name, None)
    local.update({"ANTHROPIC_BASE_URL": base, "ANTHROPIC_API_KEY": worker_key,
                  "CLAUDE_CONFIG_DIR": str(state / "claude"),
                  "CLAUDE_CODE_GATEWAY_HINT_HEADERS": "1",
                  "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC": "1",
                  "SIC_CLAUDE_HOOK_URL": base + "/sic/arm", "SIC_CLAUDE_HOOK_KEY": hook_key})
    return local


def run(args, extra):
    extra = extra[1:] if extra[:1] == ["--"] else extra
    controlled = {"--session-id", "--resume", "--continue", "--fork-session", "--from-pr", "--no-session-persistence",
                  "--settings", "--setting-sources", "-c", "-r"}
    if any(value.split("=", 1)[0] in controlled for value in extra):
        raise ValueError("Session, persistence and hook settings are owned by the SIC launcher")
    session = str(UUID(args.session))
    state = args.state.resolve()
    state.mkdir(mode=0o700, parents=True, exist_ok=True)
    cli = shutil.which(args.claude)
    if not cli:
        raise ValueError("Claude Code native executable was not found")
    worker_key, hook_key = secrets.token_urlsafe(32), secrets.token_urlsafe(32)
    operate = core_operator(args.core, os.environ["SIC_CLAUDE_CORE_GRANT"])
    with closing(sqlite3.connect(state / "launcher.lock", timeout=0)) as lock:
        lock.execute("BEGIN EXCLUSIVE")
        projection = ClaudeCompaction(state / "sic.db", session, args.chain, operate, None, args.batch)
        projection.cut()
        with listen(projection, args.provider, os.environ["SIC_CLAUDE_PROVIDER_KEY"], worker_key, hook_key) as base:
            with tempfile.TemporaryDirectory(prefix="sic-claude-settings-") as directory:
                command = child_command(Path(__file__).resolve())
                hooks = {"PreCompact": [{"matcher": "manual|auto", "hooks": [{"type": "command", "command": command}]}]}
                settings = Path(directory) / "settings.json"
                settings.write_text(json.dumps({"hooks": hooks}), encoding="utf-8")
                return subprocess.call([cli, "--setting-sources", "user", "--settings", str(settings),
                    "--resume" if args.resume else "--session-id", session, *extra],
                    env=session_environment(state, base, worker_key, hook_key))


def main():
    if sys.argv[1:] == ["precompact"]:
        precompact()
        return 0
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--claude", default="claude", help="Claude Code native executable")
    parser.add_argument("--core", default="http://127.0.0.1:18890")
    parser.add_argument("--provider", required=True, help="Fixed Messages base URL ending before /v1")
    parser.add_argument("--chain", required=True)
    parser.add_argument("--session", required=True, help="Native session UUID")
    parser.add_argument("--state", type=Path, required=True, help="Private directory dedicated to this session")
    parser.add_argument("--batch", type=int, default=8)
    parser.add_argument("--resume", action="store_true")
    parser.add_argument("extra", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    try:
        return run(args, args.extra)
    except (OSError, ValueError, KeyError, RuntimeError, sqlite3.Error) as error:
        print(f"Claude SIC startup stopped ({type(error).__name__}). Check state, grants and provider configuration.",
              file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
