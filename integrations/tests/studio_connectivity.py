"""Network-only official Studio/local Agent Server diagnostics. No GUI control."""
import argparse
import importlib.metadata
import json
from pathlib import Path
import re
import time
from urllib.error import HTTPError
from urllib.request import Request, build_opener, ProxyHandler

ROOT = Path(__file__).resolve().parents[2]
OPENER = build_opener(ProxyHandler({}))
BASE = 'http://127.0.0.1:12026'
ORIGIN = 'https://smith.langchain.com'


def fetch(url, method='GET', headers=None):
    started = time.monotonic()
    try:
        response = OPENER.open(Request(url, method=method, headers=headers or {}), timeout=25)
    except HTTPError as error:
        response = error
    with response:
        body = response.read()
        result = {'url': url, 'status': response.status, 'seconds': round(time.monotonic() - started, 4),
                  'bytes': len(body), 'headers': {k.lower(): v for k, v in response.headers.items() if k.lower().startswith('access-control') or k.lower() in {'content-type', 'vary'}}}
        return result, body


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--phase', choices=['before', 'after'], default='after')
    parser.add_argument('--remote-resources', action='store_true', help='Fetch official shell and its two entry scripts over normal HTTPS')
    args = parser.parse_args()
    versions = {name: importlib.metadata.version(name) for name in ('langgraph-api', 'langgraph-cli', 'langgraph-sdk', 'langgraph', 'starlette')}
    probes = []
    for private, origin in ((False, ORIGIN), (True, ORIGIN), (True, 'https://untrusted.invalid')):
        headers = {'Origin': origin, 'Access-Control-Request-Method': 'POST', 'Access-Control-Request-Headers': 'content-type,x-user-agent'}
        if private:
            headers['Access-Control-Request-Private-Network'] = 'true'
        result, body = fetch(BASE + '/assistants/search', 'OPTIONS', headers)
        result.update(private_network=private, origin=origin, body=body.decode())
        probes.append(result)
    info, body = fetch(BASE + '/info', headers={'Origin': ORIGIN})
    info['body'] = json.loads(body)
    remote = []
    if args.remote_resources:
        shell, html = fetch(ORIGIN + '/studio/')
        remote.append(shell)
        for path in re.findall(r'<script[^>]+src="([^"]+)', html.decode()):
            result, _ = fetch(ORIGIN + path)
            remote.append(result)
    positive = probes[1]
    fixed = positive['status'] == 200 and positive['headers'].get('access-control-allow-private-network') == 'true'
    restricted = probes[2]['status'] == 400 and probes[2]['headers'].get('access-control-allow-origin') not in {'*', 'https://untrusted.invalid'}
    report = {'phase': args.phase, 'versions': versions, 'local_info': info, 'preflights': probes, 'remote_resources': remote,
              'private_network_preflight_passed': fixed, 'untrusted_origin_blocked': restricted,
              'gui_loaded_verified': False, 'browser_permissions_inspected': False, 'model_calls': 0,
              'system_proxy_or_browser_security_modified': False}
    destination = ROOT / '.local/integration-test-results' / ('studio-connectivity-' + args.phase + '.json')
    destination.write_text(json.dumps(report, ensure_ascii=False, indent=2) + '\n', encoding='utf-8')
    print(json.dumps({'phase': args.phase, 'versions': versions, 'private_network_preflight_passed': fixed,
                      'untrusted_origin_blocked': restricted, 'remote_resources_status': [p['status'] for p in remote], 'gui_loaded_verified': False}))
    if args.phase == 'after':
        assert fixed and restricted, 'Official-origin PNA must pass and foreign origin must remain blocked'


if __name__ == '__main__':
    main()
