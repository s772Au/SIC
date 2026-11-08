"""Authenticated local HTTP transport; Core can also be embedded directly."""
import argparse,hmac,json,os
from http.server import BaseHTTPRequestHandler,ThreadingHTTPServer
from .service import Core,CoreError

def make_server(core,config,host='127.0.0.1',port=18888):
    identities={os.environ[name]:actor for name,actor in config.get('tokens',{}).items() if os.environ.get(name)}
    if not identities:raise ValueError('No authentication tokens configured via environment')
    class Handler(BaseHTTPRequestHandler):
        def log_message(self,*args):pass
        def reply(self,status,value):
            body=json.dumps(value,ensure_ascii=False,allow_nan=False).encode();self.send_response(status);self.send_header('Content-Type','application/json; charset=utf-8');self.send_header('Content-Length',str(len(body)));self.send_header('Cache-Control','no-store');self.end_headers();self.wfile.write(body)
        def principal(self):
            header=self.headers.get('Authorization','');supplied=header[7:] if header.startswith('Bearer ') else ''
            actor=next((v for k,v in identities.items() if hmac.compare_digest(k,supplied)),None)
            if not actor:raise CoreError('unauthorized','Valid Bearer authorization required',401)
            return actor
        def do_GET(self):
            try:
                self.principal()
                if self.path!='/api/v2/capabilities':raise CoreError('not_found','Route not found',404)
                self.reply(200,core.capabilities())
            except CoreError as error:self.reply(error.status,{'ok':False,'error':{'code':error.code,'message':str(error)}})
        def do_POST(self):
            try:
                actor=self.principal()
                if self.path!='/api/v2/operations':raise CoreError('not_found','Route not found',404)
                length=int(self.headers.get('Content-Length','0'))
                if length<=0 or length>16*1024*1024:raise CoreError('request_size','JSON request must be 1..16MiB',413)
                data=json.loads(self.rfile.read(length));context=data.get('context',{})
                if context.get('actor')!=actor:raise CoreError('actor_mismatch','Context actor must match authenticated principal',403)
                self.reply(200,core.operate(data['operation'],context,data.get('payload',{})))
            except CoreError as error:self.reply(error.status,{'ok':False,'error':{'code':error.code,'message':str(error)}})
            except (ValueError,KeyError,TypeError):self.reply(400,{'ok':False,'error':{'code':'invalid_json','message':'Malformed operation request'}})
            except Exception:self.reply(500,{'ok':False,'error':{'code':'internal','message':'Operation failed; no confidential details returned'}})
    return ThreadingHTTPServer((host,port),Handler)

def main():
    parser=argparse.ArgumentParser();parser.add_argument('--db',default='.local/core-v2.db');parser.add_argument('--config',required=True);parser.add_argument('--host',default='127.0.0.1');parser.add_argument('--port',type=int,default=18888);parser.add_argument('--bridge-port',type=int);args=parser.parse_args()
    from pathlib import Path
    config=json.loads(Path(args.config).read_text(encoding='utf-8-sig'));Path(args.db).parent.mkdir(parents=True,exist_ok=True)
    core=Core(args.db,config);server=make_server(core,config,args.host,args.port)
    bridge_server=None
    if args.bridge_port is not None:
        from .bridge import make_bridge_server
        import threading
        bridge_server=make_bridge_server(core,config,args.host,args.bridge_port);threading.Thread(target=bridge_server.serve_forever,daemon=True,name='sic-bridge-http').start()
    core.runtime.start()
    print(json.dumps({'bridge_port':args.bridge_port,'service':'sic-core-v2','host':args.host,'port':server.server_port,'authentication':'required','provider_calls':'explicit-dual-gate'}),flush=True)
    try:server.serve_forever()
    except KeyboardInterrupt:pass
    finally:
        server.server_close()
        if bridge_server is not None:bridge_server.shutdown();bridge_server.server_close()
        core.close()
if __name__=='__main__':main()
