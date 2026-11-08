"""Explicit fixed-profile isolated Core. Never touches any running service."""
import copy,json,os,threading,uuid
from pathlib import Path
from urllib.parse import urlsplit
from v2_experiment_fixture import Transport

class ConfiguredCore:
    def __init__(self,path,config,*,allow_test_gateway=False):self.path=Path(path);self.config=copy.deepcopy(config);self.allow_test_gateway=allow_test_gateway
    def __enter__(self):
        from sic_core.service import Core
        from sic_core.server import make_server
        for role in ('worker','editor'):
            profile=self.config['roles'][role]
            for k in ('model','model_revision','api_base','tokenizer_model','max_input_tokens','max_output_tokens','context_window','route_id'):
                if not profile.get(k):raise ValueError('Fixed profile missing '+role+'.'+k)
            if any(k in profile for k in ('api_key','access_token','authorization')):raise ValueError('Use api_key_env; inline secrets forbidden')
            if profile.get('api_base'):
                url=urlsplit(profile['api_base'])
                if url.scheme not in ('http','https') or not url.hostname or url.username or url.password or url.query or url.fragment:raise ValueError('Malformed or credential-bearing provider URLs forbidden')
                if url.port==8341 and not self.allow_test_gateway:raise ValueError('Testing route 8341 requires explicit --allow-test-gateway')
            sampling=profile.get('sampling')
            if not isinstance(sampling,dict) or not {'temperature','top_p'}<=set(sampling) or set(sampling)-{'temperature','top_p','seed'}:raise ValueError('Freeze explicit temperature/top_p and optional seed; no ignored profile sampling')
            env=profile.get('api_key_env')
            if not env or not os.environ.get(env):raise ValueError('Explicit private provider credential environment missing')
        self.key='SIC_RESEARCH_RUN_TOKEN_'+uuid.uuid4().hex;os.environ[self.key]=uuid.uuid4().hex
        self.config['tokens']={self.key:'host'};self.path.parent.mkdir(parents=True,exist_ok=True)
        from sic_core.adapters import LiteLLMProvider
        fixed=copy.deepcopy(self.config['roles']);delegate=LiteLLMProvider(fixed)
        class FixedSamplingProvider:
            def capabilities(self):return delegate.capabilities()
            def call(self,role,request):
                value=copy.deepcopy(request);value.update(fixed[role]['sampling']);return delegate.call(role,value)
        # Use Core's supported provider injection seam. Empty internal bindings
        # keep every Editor/Worker on this same immutable sampled delegate.
        internal=copy.deepcopy(self.config);internal['roles']={};internal['profiles']={}
        self.core=Core(str(self.path),internal,provider=FixedSamplingProvider());self.server=make_server(self.core,internal,port=0)
        threading.Thread(target=self.server.serve_forever,daemon=True).start();self.client=Transport('http://127.0.0.1:'+str(self.server.server_port),os.environ[self.key]);return self
    def __exit__(self,*args):
        if args and args[0] is not None:
            text=json.dumps(self.client.trace,ensure_ascii=False,indent=2)
            for profile in self.config['roles'].values():
                value=os.environ.get(profile.get('api_key_env',''),'')
                if value:text=text.replace(value,'[redacted]')
            (self.path.parent/'partial-trace.json').write_text(text+'\n',encoding='utf-8')
        self.server.shutdown();self.server.server_close();self.core.close();os.environ.pop(self.key,None)

def route_disclosure(config,allow_test_gateway=False):
    gateway=False
    for profile in config['roles'].values():
        url=urlsplit(profile.get('api_base',''))
        if url.scheme not in ('http','https') or not url.hostname or url.username or url.password or url.query or url.fragment:raise ValueError('Malformed or credential-bearing provider URLs forbidden')
        gateway=gateway or url.port==8341
    if gateway and not allow_test_gateway:raise ValueError('Testing route 8341 requires explicit --allow-test-gateway')
    return {'route_status':'nonfixed-exploratory','quality_generalization':False,'model_identity_attested':False} if gateway else {'route_status':'declared-profile-not-provider-attested'}

def public_config(config,allow_test_gateway=False):
    disclosure=route_disclosure(config,allow_test_gateway)
    return {'roles':{role:{k:v for k,v in p.items() if k in ('model','model_revision','api_base','tokenizer_model','max_input_tokens','max_output_tokens','context_window','route_id','tokenizer_status','sampling')} for role,p in config['roles'].items()},'provider_revision_status':'nonfixed-exploratory' if disclosure['route_status']=='nonfixed-exploratory' else 'declared-not-provider-attested',**disclosure}
