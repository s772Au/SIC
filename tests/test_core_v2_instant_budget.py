import json,unittest
from sic_core.memory import bounded_instant

class InstantBudgetTests(unittest.TestCase):
    def test_many_keys_and_long_names_bound_metadata_and_preserve_archive_reference(self):
        for value in ({'note-'+str(i):'x' for i in range(500)}, {'long-name'*500:'tiny'}, {'instant_projection':'user value',**{str(i):'y' for i in range(100)}}):
            for budget in (256,512,2048):
                projected=bounded_instant(value,budget,'cut_'+'a'*32)
                self.assertLessEqual(len(json.dumps(projected,ensure_ascii=False)),budget)
                if len(json.dumps(value,ensure_ascii=False))<=budget:
                    self.assertEqual(projected,value)
                    continue
                manifest=projected['instant_projection']
                self.assertEqual(manifest['total'],len(value))
                self.assertEqual(manifest['omitted'],len(value)-len(projected)+1)
                self.assertEqual(manifest['object_id'],'cut_'+'a'*32)
                self.assertEqual(manifest['read_operation'],'memory.page')
    def test_small_view_is_unchanged(self):
        self.assertEqual(bounded_instant({'note':'ok'},256,None),{'note':'ok'})

if __name__=='__main__':unittest.main()
