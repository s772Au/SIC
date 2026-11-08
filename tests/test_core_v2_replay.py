import copy,unittest
from sic_core.editing import materialize,replay

class OperationReplayTests(unittest.TestCase):
    def test_each_intermediate_state_uses_the_same_archived_source(self):
        source={'units':[{'unit_id':'u1','text':'原始🧭针','anchor':{}},{'unit_id':'u2','text':'second','anchor':{}}]}
        operations=[{'op':'REPLACE','target':'u1','content':'精炼'}, {'op':'FLAG','target':'u1','content':'待核'}, {'op':'REORDER','target':'u2','order':-1}, {'op':'DELETE','target':'u2'}, {'op':'ADD','content':'new','anchor':{'target':'u1','position':'before'}}]
        value=materialize(source,[{'editor':'e','unit_ids':['u1','u2']}],{'e':{'ops':operations,'pvi':{}}});original=copy.deepcopy(source)
        expected=['原始🧭针\nsecond','精炼\nsecond','精炼\n[FLAG] 待核\nsecond','second\n精炼\n[FLAG] 待核','精炼\n[FLAG] 待核','new\n精炼\n[FLAG] 待核']
        for step,body in enumerate(expected):self.assertEqual(replay(source,value['ops'],step)['body'],body)
        self.assertEqual(replay(source,value['ops'],len(value['ops']))['body'],value['body']);self.assertEqual(source,original)
    def test_program_keeps_and_multi_editor_pvi_preserve_final_order(self):
        source={'units':[{'unit_id':'u1','text':'a','anchor':{}},{'unit_id':'u2','text':'b','anchor':{}}]}
        value=materialize(source,[{'editor':'e1','unit_ids':['u1']},{'editor':'e2','unit_ids':['u2']}],{
            'e1':{'ops':[{'op':'COMPRESS','target':'u1','content':'A'},{'op':'ADD','content':'add-one','lifecycle':'V'}],'pvi':{'P':['persistent-one']}},
            'e2':{'ops':[{'op':'ADD','content':'add-two'},{'op':'ADD','content':'current','lifecycle':'I','view_key':'now'}],'pvi':{'P':['persistent-two']}}})
        result=replay(source,value['ops'],len(value['ops']))
        for key in ('body','volatile','instant'):self.assertEqual(result[key],value[key])
        self.assertFalse(result['external_effects_replayed'])
        with self.assertRaises(ValueError):replay(source,value['ops'],len(value['ops'])+1)

if __name__=='__main__':unittest.main()
