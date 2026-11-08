"""Local official-runtime authentication; credentials never enter flow data."""
import json
import os
from pathlib import Path
from urllib.parse import urlencode, urlsplit
from urllib.request import Request


def langflow_access_token(base, opener):
    parsed = urlsplit(base)
    if parsed.scheme != "http" or parsed.hostname not in {"localhost", "127.0.0.1", "::1"} or parsed.username or parsed.password:
        raise ValueError("Acceptance authentication is restricted to local official runtimes")
    if os.environ.get("LANGFLOW_ACCESS_TOKEN"):
        return os.environ["LANGFLOW_ACCESS_TOKEN"]
    username = os.environ.get("LANGFLOW_USERNAME")
    password = os.environ.get("LANGFLOW_PASSWORD")
    project = Path(__file__).resolve().parents[2]
    path = project / ".local" / f"langflow-{parsed.port}" / "admin-auth.json"
    if (not username or not password) and path.is_file():
        private = json.loads(path.read_text(encoding="utf-8-sig"))
        username, password = private["username"], private["password"]
    if username and password:
        body = urlencode({"username": username, "password": password}).encode()
        request = Request(base + "/api/v1/login", body, {"Content-Type": "application/x-www-form-urlencoded"}, method="POST")
    else:
        request = Request(base + "/api/v1/auto_login")
    with opener.open(request, timeout=30) as response:
        return json.load(response)["access_token"]
