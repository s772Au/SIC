"""Exact local Editor aliases are a frozen representation, never fuzzy repair."""
import copy,json,unittest
from dataclasses import FrozenInstanceError
from sic_core.editor_aliases import FrozenAliasMap,freeze_aliases,alias_materials,canonical_response
from sic_core.editing import validate_response

class EditorAliasesTests(unittest.TestCase):
    def setUp(self):
        self.ids=['source_664abc:u1','source_other:u2'];self.frozen=freeze_aliases('job-1','alpha',self.ids)
    def decode(self,raw,frozen=None,job='job-1',editor='alpha'):
        return canonical_response(frozen or self.frozen,raw,job_id=job,editor=editor)
    def test_deterministic_frozen_and_json_roundtrip(self):
        original=self.frozen.as_dict();self.ids.append('late-unit');self.assertEqual(self.frozen.unit_ids,('source_664abc:u1','source_other:u2'));self.assertEqual(FrozenAliasMap.from_dict(json.loads(json.dumps(original))),self.frozen);self.assertEqual(freeze_aliases('job-1','alpha',list(self.frozen.unit_ids)).map_id,self.frozen.map_id)
        with self.assertRaises(FrozenInstanceError):self.frozen.editor='beta'
        original['mapping'][0]['unit_id']='tampered';self.assertNotEqual(original,self.frozen.as_dict())
        with self.assertRaises(ValueError):FrozenAliasMap.from_dict(original)
    def test_duplicate_invalid_and_unordered_scope_rejected(self):
        for scope in [['same','same'],[''],[None],'u1',{'u1','u2'},{'u1':1}]:
            with self.subTest(scope=scope),self.assertRaises(ValueError):freeze_aliases('job','alpha',scope)
    def test_material_projection_changes_only_top_level_unit_ids(self):
        units=[{'unit_id':uid,'text':'e1 '+uid,'anchor':{'unit_id':uid,'source_id':'canonical-source'}} for uid in self.frozen.unit_ids];provenance=[{'unit_id':uid,'source_id':uid,'note':'e1 e2 '+uid} for uid in self.frozen.unit_ids];before=copy.deepcopy((units,provenance));projected=alias_materials(self.frozen,units,provenance)
        for key,old in [('editable_units',units),('readonly_provenance',provenance)]:
            for i,(left,right) in enumerate(zip(projected[key],old)):
                self.assertEqual(left['unit_id'],'e'+str(i+1));self.assertEqual({k:v for k,v in left.items() if k!='unit_id'},{k:v for k,v in right.items() if k!='unit_id'})
        self.assertEqual((units,provenance),before);projected['editable_units'][0]['text']='changed';self.assertEqual((units,provenance),before)
    def test_scope_missing_extra_or_duplicate_projection_denied(self):
        for units in [[{'unit_id':'source_664abc:u1'}],[{'unit_id':'outside'},{'unit_id':'source_other:u2'}],[{'unit_id':'source_664abc:u1'},{'unit_id':'source_664abc:u1'}]]:
            with self.assertRaises(ValueError):alias_materials(self.frozen,units,[])
        units=[{'unit_id':uid} for uid in self.frozen.unit_ids]
        with self.assertRaises(ValueError):alias_materials(self.frozen,units,[{'unit_id':'outside'}])
    def test_all_target_ops_and_add_anchor_translate_not_arbitrary_strings(self):
        raw={'ops':[{'op':'KEEP','target':'e1'},{'op':'DELETE','target':'e2'},{'op':'FLAG','target':'e1','content':'e1 source_6664abc:u1','reason':'do not replace e2'},{'op':'REORDER','target':'e1','order':3},{'op':'COMPRESS','target':'e1','content':'e1'},{'op':'REPLACE','target':'e1','content':'e2'},{'op':'ADD','content':'e1','anchor':{'target':'e1','position':'before'}}],'pvi':{'P':['e1'],'V':['source_6664abc:u1'],'I':{'target':'e1'}}};original=copy.deepcopy(raw);canonical=self.decode(raw);self.assertEqual(raw,original);validate_response(self.frozen.unit_ids,canonical['ops'],canonical['pvi'])
        for op in canonical['ops'][:-1]:self.assertIn(op['target'],self.frozen.unit_ids)
        self.assertEqual(canonical['ops'][-1]['anchor']['target'],self.frozen.unit_ids[0]);self.assertEqual(canonical['ops'][2]['content'],raw['ops'][2]['content']);self.assertEqual(canonical['ops'][2]['reason'],raw['ops'][2]['reason']);self.assertEqual(canonical['pvi'],raw['pvi']);canonical['pvi']['P'].append('mutated-copy');self.assertEqual(raw,original)
    def test_unknown_canonical_typo_or_near_alias_not_repaired(self):
        for target in ['source_664abc:u1','source_6664abc:u1','e01','E1',' e1','e3',None,1]:
            raw={'ops':[{'op':'KEEP','target':target}],'pvi':{}};before=copy.deepcopy(raw)
            with self.subTest(target=target),self.assertRaises(ValueError):self.decode(raw)
            self.assertEqual(raw,before)
        with self.assertRaises(ValueError):self.decode({'ops':[{'op':'ADD','content':'x','anchor':{'target':'e3','position':'after'}}],'pvi':{}})
    def test_job_and_multi_editor_isolation(self):
        beta=freeze_aliases('job-1','beta',['private-beta']);self.assertNotEqual(beta.map_id,self.frozen.map_id)
        raw={'ops':[{'op':'KEEP','target':'e1'}],'pvi':{}};self.assertEqual(self.decode(raw)['ops'][0]['target'],'source_664abc:u1');self.assertEqual(self.decode(raw,beta,editor='beta')['ops'][0]['target'],'private-beta')
        with self.assertRaises(ValueError):self.decode(raw,beta)
        with self.assertRaises(ValueError):self.decode(raw,job='job-2')
        with self.assertRaises(ValueError):self.decode({'ops':[{'op':'KEEP','target':'e2'}],'pvi':{}},beta,editor='beta')
    def test_single_lookup_even_when_canonical_id_looks_like_an_alias(self):
        frozen=freeze_aliases('job-1','alpha',['e2','long-canonical']);value=self.decode({'ops':[{'op':'KEEP','target':'e1'}],'pvi':{}},frozen);self.assertEqual(value['ops'][0]['target'],'e2')
    def test_empty_assignment_and_noop_are_supported_without_changing_legacy_ids(self):
        frozen=freeze_aliases('job-1','alpha',[]);self.assertEqual(alias_materials(frozen,[],[]),{'editable_units':[],'readonly_provenance':[]});self.assertEqual(self.decode({'ops':[],'pvi':{}},frozen),{'ops':[],'pvi':{}})
        # Core/manual canonical validation remains unchanged; this helper is opt-in.
        validate_response(self.frozen.unit_ids,[{'op':'KEEP','target':'source_664abc:u1'}],{})
    def test_add_target_or_nonadd_anchor_never_silently_normalized(self):
        for op in [{'op':'ADD','target':None,'content':'x'},{'op':'KEEP','target':'e1','anchor':{'target':'e2','position':'after'}}]:
            with self.assertRaises(ValueError):self.decode({'ops':[op],'pvi':{}})

if __name__=='__main__':unittest.main()
