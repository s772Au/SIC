"""Installed-distribution mechanical check; no checkout imports or model calls."""
import hashlib
import importlib.metadata
import importlib.resources
import importlib.util
import json
from pathlib import Path
import sys
import sic_core
from sic_core import Core


def main():
    installed=Path(sic_core.__file__).resolve();prefix=Path(sys.prefix).resolve()
    assert installed.is_relative_to(prefix), 'Core must load from the isolated installation'
    assert 'site-packages' in installed.parts, 'Core must not load from a source checkout'
    assert importlib.metadata.version('sic-core')=='0.1.0'
    assert importlib.util.find_spec('litellm') is None, 'Base verification must not install the optional provider'
    assert importlib.util.find_spec('PIL') is None, 'Base verification must not install optional image support'
    assert importlib.util.find_spec('torch') is None
    catalog_bytes=importlib.resources.files('sic_core').joinpath('operations.json').read_bytes()
    catalog=json.loads(catalog_bytes);names={row['operation'] for row in catalog['operations']}
    assert {'source.ingest','edit.program','cut.publish','hierarchy.form','hierarchy.expand','ledger.export'}<=names
    for resource in ('task_ledger.schema.json','config.example.json','requirements.txt'):
        assert importlib.resources.files('sic_core').joinpath(resource).is_file()
    core=Core();context={'actor':'installed-package-check','allow_write':True,'allow_model':False}
    def op(name,payload=None):return core.operate(name,context,payload or {})['result']
    def publish(source,pvi):
        job=op('edit.program',{'source_id':source,'pvi':pvi})['job_id']
        return op('cut.publish',{'job_id':job,'expected_cut':op('chain.get')['cut_seq'],'idempotency_key':job})
    def body(cb):return op('memory.page',{'kind':'cb','object_id':cb,'offset':0,'limit':8192})['value']
    try:
        context['chain_id']=op('chain.create')['chain_id']
        source=op('source.ingest',{'request':{'messages':[{'role':'user','content':'Installed immutable original needle'}]}})['source_id']
        first=publish(source,{'P':['MAIN-P-FIRST'],'V':['MAIN-V-NEXT'],'I':{'stage':'MAIN-I-FIRST'}})
        first_body=body(first['cb_id']);assert 'MAIN-P-FIRST' in first_body and 'MAIN-I-FIRST' not in first_body
        pending=op('chain.get')['pending'];assert pending==[first['volatile_source_id']]
        assert 'MAIN-V-NEXT' in json.dumps(op('source.read',{'source_id':pending[0]}))
        second=publish(pending[0],{'P':['MAIN-P-SECOND'],'I':{'stage':'MAIN-I-SECOND'}})
        assert op('chain.get')['instant']['stage']=='MAIN-I-SECOND'
        assert body(first['cb_id'])==first_body
        high_source=op('hierarchy.form',{'level':2,'cb_ids':[first['cb_id'],second['cb_id']]})['source_id']
        high_units=op('source.read',{'source_id':high_source})['source']['units']
        assert all('MAIN-I-' not in unit['text'] for unit in high_units)
        upper=publish(high_source,{'P':['UPPER-P-SECOND'],'V':['UPPER-V-NEXT'],'I':{'stage':'UPPER-I-SECOND'}})
        assert 'UPPER-P-SECOND' in body(upper['cb_id']) and 'UPPER-I-SECOND' not in body(upper['cb_id'])
        third_source=op('hierarchy.form',{'level':3,'cb_ids':[upper['cb_id']]})['source_id']
        third=publish(third_source,{'P':['UPPER-P-THIRD'],'I':{'stage':'UPPER-I-THIRD'}})
        hierarchy=op('hierarchy.get')
        for level,publication in ((1,first),(1,second),(2,upper),(3,third)):
            row=hierarchy['levels'][str(level)]
            assert publication['wb_id'] in row['wbs'] and publication['cb_id'] in row['cbs']
            assert row['pairs'][publication['wb_id']]==publication['cb_id']
            assert op('hierarchy.expand',{'cb_id':publication['cb_id']})['wb_id']==publication['wb_id']
        assert hierarchy['levels']['2']['pending']==[upper['volatile_source_id']]
        assert hierarchy['levels']['2']['instant']['stage']=='UPPER-I-SECOND'
        assert op('chain.get')['instant']['stage']=='MAIN-I-SECOND'
        assert 'UPPER-V-NEXT' in json.dumps(op('source.read',{'source_id':upper['volatile_source_id']}))
        assert body(first['cb_id'])==first_body
        op('hierarchy.select',{'level':3,'roles':['worker']})
        preview=op('provider.preview');assert third['cb_id'] in json.dumps(preview['request'])
        assert len(op('chain.list')['routes'])==1
        assert op('ledger.export')['calls']==[]
        assert core.capabilities()['adapters']['sdk_installed'] is False
        result={'status':'passed','distribution':'sic-core','version':'0.1.0','installed_core':'<fresh-venv>/'+installed.relative_to(prefix).as_posix(),'isolated_prefix':'<fresh-venv>','installed_location_asserted_inside_fresh_site_packages':True,'catalog_operations':len(names),'catalog_sha256':hashlib.sha256(catalog_bytes).hexdigest(),'actual_levels':[1,2,3],'same_level_pairs':4,'same_agent_routes':1,'PVI_same_chain':True,'immutable_first_CB':True,'optional_provider_installed':False,'optional_media_installed':False,'model_calls':0,'service_started':False,'physical_cache_hit':'unknown','semantic_quality':'not_evaluated'}
        print(json.dumps(result,ensure_ascii=False))
    finally:core.close()

if __name__=='__main__':main()
