"""Freeze source inventories and control package without copying private data."""
import hashlib,json,sys,zipfile
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1];sys.path.insert(0,str(ROOT))
from benchmarks.core_adapters import readiness


EDITOR_OPERATION_CONTRACT = """Return one JSON object with ops (an ordered array) and pvi containing P and V arrays of strings and I as a named view object. Unmentioned units KEEP. Allowed operation fields only: op, target, content, reason, order, anchor, risk, lifecycle, view_key. Never emit role, tool IDs, permissions, provenance, origin or editor fields.
Seven operations and REQUIRED fields:
- KEEP: op=KEEP and target (an exact assigned unit_id); leaves that unit unchanged.
- DELETE: op=DELETE and target; removes only the active representation, never the archive.
- FLAG: op=FLAG, target AND content (a string containing the annotation). reason is optional explanatory metadata and NEVER substitutes for required content. Valid example: {"op":"FLAG","target":"u1","content":"Unverified proposal, not an executed action.","reason":"Preserve action status"}.
- REORDER: op=REORDER, target and order (an integer, never a boolean).
- COMPRESS: op=COMPRESS, target and content (replacement text string).
- REPLACE: op=REPLACE, target and content (replacement text string).
- ADD: op=ADD and content (new text string); OMIT target. Optional anchor must be exactly {"target":"assigned_unit_id","position":"before"} or position="after". anchor is permitted ONLY on ADD, must reference an assigned unit that remains active, and has no extra keys. Without anchor ADD appends content.
Every non-ADD operation requires target within YOUR assigned units. Ordered operations can combine on one target; never FLAG or anchor to a unit already deleted from the active representation. reason, when supplied, is a string; optional risk is one of none, low, medium, high, unknown.
lifecycle defaults to P. Only ADD, COMPRESS, REPLACE and FLAG may set lifecycle to V or I, and ALL FOUR still require content. KEEP, DELETE and REORDER must use P (or omit lifecycle). Any lifecycle=I operation MUST also provide view_key as a string naming its instantaneous view. Generated V content goes to the next WB; generated I content goes to that named current view; neither becomes durable CB body. COMPRESS/REPLACE routed to V/I remove their target from the durable representation; FLAG routed to V/I emits its annotation there without replacing target text. Operation records remain persistent regardless of generated-content lifecycle. P/V/I are not permissions.
Return pvi as {"P":[],"V":[],"I":{}} when no extra communication is needed; do not put operational metadata or protocol controls into editable content fields.
Edit only the CURRENT assigned WB. Read-only backgrounds are reference, not new editable units: do not copy their facts, decision tree or P/I into this response just to restate them. Add P/V/I only when the current task actually needs new communication or a current-view update; all seven operations and P/V/I remain available. If the assigned WB is merely a short acknowledgement without a new fact, correction or actionable constraint, no-op is valid and default KEEP retains it.
Return exactly one COMPLETE JSON object, no Markdown fences, commentary or text outside it. Include the full pvi object and close the outermost object with }. Examples below are complete envelopes, not fragments. The FLAG example is valid only when u1 is assigned; use the actual assigned unit_id, never copy an unassigned example ID.
NO-OP EXAMPLE: {"ops":[],"pvi":{"P":[],"V":[],"I":{}}}
FLAG EXAMPLE: {"ops":[{"op":"FLAG","target":"u1","content":"Unverified proposal, not an executed action."}],"pvi":{"P":[],"V":[],"I":{}}}
Reserved pvi.I.decision_tree MUST be an object with exactly four named view arrays: "Working on", "Failed Before", "Not Tried Yet", "Overlooked". Each array entry is a string or an object containing text (string) and evidence (array); optional id/status strings may preserve an existing entry. Never put a prose string in decision_tree. Other I keys may hold their original JSON value types and are not subject to this reserved four-view shape. Emit decision_tree only for an actual current-state change; omit it when no update is needed. For an update retain still-current entries. This empty example demonstrates valid shape only, not an instruction to reset existing views:
DECISION-TREE EXAMPLE: {"ops":[],"pvi":{"P":[],"V":[],"I":{"decision_tree":{"Working on":[],"Failed Before":[],"Not Tried Yet":[],"Overlooked":[]}}}}"""


# The operation still carries a string; only the primary reserved view decodes it.
EDITOR_OPERATION_CONTRACT += "\nPrefer the direct pvi.I.decision_tree object for decision updates. On the primary main level-1 context, an alternative lifecycle=I operation with view_key=decision_tree MUST keep content as a string encoding the complete four-view JSON object; the host parses it once and rejects malformed JSON or wrong shape, never repairs it. Generic I content remains a string, and private/higher-context generic I text is not silently decoded. This example is the primary reserved-view alternative, not a request to generate a new decision tree:\nDECISION-TREE I-OP EXAMPLE: " + json.dumps({'ops':[{'op':'ADD','lifecycle':'I','view_key':'decision_tree','content':json.dumps({name:[] for name in ['Working on','Failed Before','Not Tried Yet','Overlooked']},separators=(',',':'))}],'pvi':{'P':[],'V':[],'I':{}}},separators=(',',':'))


def editor_response_schema():
    """Canonical output envelope and conditional structural requirements.

    Assigned target membership and ordered active-target validity require Core's
    scope/materialization checks; static JSON Schema cannot attest those facts.
    """
    views=['Working on','Failed Before','Not Tried Yet','Overlooked']
    entry={'anyOf':[{'type':'string'},{'type':'object','required':['text','evidence'],'additionalProperties':False,'properties':{'text':{'type':'string'},'evidence':{'type':'array'},'id':{'type':'string'},'status':{'type':'string'}}}]}
    decision_tree={'type':'object','required':views,'additionalProperties':False,'properties':{view:{'type':'array','items':entry} for view in views}}
    generated=['ADD','COMPRESS','REPLACE','FLAG']
    operation={'type':'object','required':['op'],'additionalProperties':False,'properties':{
        'op':{'enum':['KEEP','DELETE','FLAG','REORDER','COMPRESS','REPLACE','ADD']},'target':{'type':'string'},'content':{'type':'string','description':'Generated text. For the primary main level-1 reserved I/view_key=decision_tree, encode the complete four-view object as JSON text; host context-aware validation parses once. Private/higher generic I text is unchanged.'},'reason':{'type':'string'},'order':{'type':'integer'},
        'anchor':{'type':'object','required':['target','position'],'additionalProperties':False,'properties':{'target':{'type':'string'},'position':{'enum':['before','after']}}},
        'risk':{'enum':['none','low','medium','high','unknown']},'lifecycle':{'enum':['P','V','I']},'view_key':{'type':'string'}},'allOf':[
        {'if':{'properties':{'op':{'const':'ADD'}}},'then':{'not':{'required':['target']}},'else':{'required':['target']}},
        {'if':{'properties':{'op':{'enum':generated}}},'then':{'required':['content']}},
        {'if':{'properties':{'op':{'const':'REORDER'}}},'then':{'required':['order']}},
        {'if':{'required':['anchor']},'then':{'properties':{'op':{'const':'ADD'}}}},
        {'if':{'required':['lifecycle'],'properties':{'lifecycle':{'enum':['V','I']}}},'then':{'properties':{'op':{'enum':generated}}}},
        {'if':{'required':['lifecycle'],'properties':{'lifecycle':{'const':'I'}}},'then':{'required':['view_key']}}]}
    return {'$schema':'https://json-schema.org/draft/2020-12/schema','$comment':'Assigned scope, ordered active targets and primary reserved I-op decoded decision shape require Core context-aware validation; content remains a string and no global rule rejects private/higher generic I text.','type':'object','required':['ops','pvi'],'additionalProperties':False,'properties':{'ops':{'type':'array','items':operation},'pvi':{'type':'object','additionalProperties':False,'properties':{'P':{'type':'array','items':{'type':'string'}},'V':{'type':'array','items':{'type':'string'}},'I':{'type':'object','properties':{'decision_tree':decision_tree},'additionalProperties':True}}}}}


def synchronize_editor_contract(pack):
    path=pack/'control-pack.json';control=json.loads(path.read_text(encoding='utf-8'));prompt=control['prompts']['sic_editor'];start=prompt.index('Return one JSON object with ops');end=prompt.index('\nP belongs to this immutable CB;',start);control['prompts']['sic_editor']=prompt[:start]+EDITOR_OPERATION_CONTRACT+prompt[end:]
    path.write_text(json.dumps(control,ensure_ascii=False,indent=2)+'\n',encoding='utf-8');(pack/'editor-response.schema.json').write_text(json.dumps(editor_response_schema(),ensure_ascii=False,indent=2)+'\n',encoding='utf-8')

def main():
    pack=ROOT/'research/compression/pack';synchronize_editor_contract(pack);lock=json.loads((pack.parent/'sources.lock.json').read_text(encoding='utf-8'));checks=[]
    for f in lock['files']:
        local=ROOT/'.local/compression-survey'/(f['repo'].replace('/','_')+'__'+f['path'].replace('/','_'))
        checks.append({'url':f['url'],'commit':f['commit'],'sha256':f['sha256'],'cache_present':local.is_file(),'cache_matches':local.is_file() and hashlib.sha256(local.read_bytes()).hexdigest()==f['sha256']})
    packages=[]
    for p in (ROOT/'benchmarks/packages').glob('sic-*-resource.zip'):
        with zipfile.ZipFile(p) as z:ok=z.testzip() is None
        packages.append({'path':p.relative_to(ROOT).as_posix(),'bytes':p.stat().st_size,'sha256':hashlib.sha256(p.read_bytes()).hexdigest(),'zip_crc_ok':ok})
    report=readiness();report.update(control_source_cache=checks,existing_control_packages=packages)
    (pack/'data-readiness.json').write_text(json.dumps(report,ensure_ascii=False,indent=2)+'\n',encoding='utf-8')
    paths=[p for p in pack.rglob('*') if p.is_file() and p.name!='manifest.json']+[pack.parent/'sources.lock.json']
    manifest={'schema':'sic-research-pack-v2','preparation':{'text_control_pack':'prepared','independent_reference_and_noise_rubric':'prepared','public_data_files':'existing-verification-reused-current-size-checked','llmlingua':'prepared-official-pinned-isolated-cpu-model; actual-weight-and-Core-consumption-acceptance-recorded-separately','official_lme':'prepared-native-image-carrier-and-pinned-official-Memory-wrapper; see independent acceptance; real quality not_evaluated'},'mechanical_acceptance':'see new isolated test evidence; never implies research quality','research_evaluation':'not_evaluated','files':{p.relative_to(ROOT).as_posix():{'sha256':hashlib.sha256(p.read_bytes()).hexdigest(),'bytes':p.stat().st_size} for p in paths}}
    (pack/'manifest.json').write_text(json.dumps(manifest,ensure_ascii=False,indent=2)+'\n',encoding='utf-8')
    bundle=ROOT/'benchmarks/packages/sic-research-controls-v2.zip'
    executable=[ROOT/'scripts'/n for n in ('run_compression_research.py','score_compression_research.py','research_scoring.py','research_provider.py','v2_experiment_fixture.py','dataset_adapters.py','prepare_compression_research.py','prepare_llmlingua.py','llmlingua_adapter.py','llmlingua_worker.py','package_llmlingua.py')]+[ROOT/'benchmarks'/n for n in ('core_adapters.py','source_paths.py','prepare_core_lme.py','sources-lock.json','test_research_v2.py','test_llmlingua_adapter.py','test_research_media.py','core_lme_carrier.py','core_lme_memory.py','run_core_lme.py','lme-official-source-lock.json','lme-processor-lock.json','prepare_lme_processor.py','lme-official-requirements.lock.txt','README-core-lme.md')]+[ROOT/'benchmarks/prepared/source-verification.json',ROOT/'benchmarks/prepared/data-licenses.json']+list((ROOT/'benchmarks/packages').glob('sic-*-resource.zip'))+list((ROOT/'benchmarks/configs').glob('sic-lme-*.json'))+list((ROOT/'sic_core').rglob('*.py'))+[ROOT/'sic_core/operations.json']+list(pack.rglob('*'))+[pack.parent/'sources.lock.json',pack.parent/'requirements-research.txt',pack.parent/'requirements-llmlingua.txt',pack.parent/'llmlingua-requirements.lock.txt',pack.parent/'README.md']
    frozen_media=json.loads((pack/'media-lme-sources.lock.json').read_text(encoding='utf-8'))
    for name,expected in frozen_media['files'].items():
        if hashlib.sha256((ROOT/name).read_bytes()).hexdigest()!=expected:raise ValueError('Frozen media/LME source changed: '+name)
        executable.append(ROOT/name)
    with zipfile.ZipFile(bundle,'w',zipfile.ZIP_DEFLATED) as z:
        for source in sorted(set(executable)):
            if source.is_file() and '__pycache__' not in source.parts:z.write(source,source.relative_to(ROOT).as_posix())
    print(json.dumps({'sources':len(report['files']),'all_size_matches':all(f['size_matches_lock'] for f in report['files']),'upstream_originals_cached_and_verified':all(c['cache_matches'] for c in checks),'control_zips':packages,'research_evaluation':'not_evaluated'}))
if __name__=='__main__':main()
