"""Install the Rust workbench in a dedicated local Langflow instance."""
import argparse
import json
from pathlib import Path
from urllib.error import HTTPError
from urllib.parse import urlencode
from urllib.request import ProxyHandler, Request, build_opener


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--url", required=True)
    parser.add_argument("--auth-file", required=True)
    parser.add_argument("--runtime-dir", required=True)
    args = parser.parse_args()
    base = args.url.rstrip("/")
    if not base.startswith("http://127.0.0.1:"):
        raise ValueError("The dedicated workbench is installed only on IPv4 loopback")
    opener = build_opener(ProxyHandler({}))
    private = json.loads(Path(args.auth_file).read_text(encoding="utf-8-sig"))
    body = urlencode({"username": private["username"], "password": private["password"]}).encode()
    with opener.open(Request(base + "/api/v1/login", body,
                             {"Content-Type": "application/x-www-form-urlencoded"}, method="POST"), timeout=30) as response:
        token = json.load(response)["access_token"]
    headers = {"Content-Type": "application/json", "Authorization": "Bearer " + token}

    def request(path, body=None):
        data = None if body is None else json.dumps(body, ensure_ascii=False).encode("utf-8")
        with opener.open(Request(base + path, data, headers, method="GET" if data is None else "POST"), timeout=60) as response:
            return json.load(response)

    marker = Path(args.runtime_dir) / "rust-flow-id.txt"
    if marker.is_file():
        flow_id = marker.read_text(encoding="utf-8").strip()
        try:
            request("/api/v1/flows/" + flow_id)
            print("Rust workbench already installed; existing edits preserved.")
            return
        except HTTPError as error:
            if error.code != 404:
                raise
    flow = json.loads((Path(__file__).parent / "flows" / "workbench-rust.json").read_text(encoding="utf-8"))
    created = request("/api/v1/flows/", flow)
    marker.write_text(created["id"] + "\n", encoding="utf-8")
    print("Rust workbench installed in Langflow.")


if __name__ == "__main__":
    main()
