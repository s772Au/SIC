"""Optional local transport for explicit host-owned fixed profiles; no startup calls."""
import argparse
import json
from pathlib import Path
import sys
import threading

PROJECT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(PROJECT))
from sic_core.service import Core
from sic_core.server import make_server


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--config", required=True)
    parser.add_argument("--db", default=".local/core-fixed-profile.db")
    parser.add_argument("--port", type=int, default=18888)
    parser.add_argument("--bridge-port", type=int, default=18890)
    args = parser.parse_args()
    config = json.loads(Path(args.config).read_text(encoding="utf-8-sig"))
    path = PROJECT / args.db
    path.parent.mkdir(parents=True, exist_ok=True)
    core = Core(str(path), config)
    server = make_server(core, config, port=args.port)
    bridge = None
    if args.bridge_port:
        from sic_core.bridge import make_bridge_server
        bridge = make_bridge_server(core, config, port=args.bridge_port)
        threading.Thread(target=bridge.serve_forever, daemon=True).start()
    print(json.dumps({"mode": "host-fixed-profile", "core_port": args.port, "bridge_port": args.bridge_port,
                      "authentication": "required", "startup_model_calls": 0}), flush=True)
    try:
        server.serve_forever()
    finally:
        server.server_close()
        if bridge:
            bridge.shutdown()
            bridge.server_close()
        core.close()


if __name__ == "__main__":
    main()
