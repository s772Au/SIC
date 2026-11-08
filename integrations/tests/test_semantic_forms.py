"""Typed forms preserve scope, time bounds, and historical unknowns."""
import math
from pathlib import Path
import sys
import unittest
import os
import tempfile
ROOT=Path(__file__).resolve().parents[2]
sys.path[:0]=[str(ROOT),str(ROOT/'integrations')]
from pydantic import ValidationError
from sic_workflow.domain_graphs import RuntimeForm,runtime_request,EditorForm,editor_request
from sic_workflow.hierarchy_forms import HierarchyForm,hierarchy_request,hierarchy_rows

class SemanticForms(unittest.TestCase):
    def test_board_claim_uses_user_task_text_instead_of_component_metadata(self):
        sys.path.insert(0,str(ROOT/'scripts'))
        from v2_experiment_fixture import OwnedFixture
        from components.sic.board import SICBoard
        saved={k:os.environ.get(k) for k in ('SIC_CORE_URL','SIC_CORE_TOKEN','SIC_CORE_ACTOR')}
        try:
            with tempfile.TemporaryDirectory(prefix='sic-board-input-') as directory, OwnedFixture(Path(directory)/'core.db') as fixture:
                fixture.client.call('chain.create')
                os.environ.update(SIC_CORE_URL=fixture.client.origin,SIC_CORE_TOKEN=fixture.client.token,SIC_CORE_ACTOR='host')
                component=SICBoard()
                component.set(chain_id=fixture.client.context['chain_id'],action='认领或更新自己的任务',
                              task_id='owned-task',description='Actual operator task, not the component description.',allow_write=True)
                component._pre_run_setup()
                result=component.run_board().data
                self.assertTrue(result['ok'],result)
                actual=fixture.client.call('board.get')['value']['tasks']['owned-task']['description']
                self.assertEqual(actual,'Actual operator task, not the component description.')
                self.assertNotEqual(actual,SICBoard.description)
        finally:
            for key,value in saved.items():
                if value is None:os.environ.pop(key,None)
                else:os.environ[key]=value

    def test_coverage_uses_current_author_contract_not_superseded_65_rows(self):
        from components.sic.coverage import SICCoverage
        component=SICCoverage();component.set(requirement_filter='')
        rows=component.requirements().to_dict('records')
        self.assertEqual({r['需求'] for r in rows},{f'F{i:02}' for i in range(1,74)})
        self.assertTrue(all(r['完整通过'] is False for r in rows))
        component.set(requirement_filter='F56')
        selected=component.requirements().to_dict('records')
        self.assertEqual(len(selected),1);self.assertEqual(selected[0]['需求'],'F56')
    def test_deadline_explicit_and_invalid_values_rejected(self):
        form=RuntimeForm(action='configure',max_pending_seconds=.25).model_dump()
        self.assertEqual(runtime_request(form)[1]['max_pending_seconds'],.25)
        for n in [0,-1,86401,math.inf,math.nan]:
            with self.assertRaises(ValidationError):RuntimeForm(max_pending_seconds=n)
    def test_explicit_second_editor_can_have_empty_scope(self):
        form=EditorForm(action='prepare',two_editors=True,editor_one_name='alpha',editor_two_name='beta',editor_one_units=['u1']).model_dump()
        self.assertEqual(editor_request(form)[1]['assignments'],[{'editor':'alpha','unit_ids':['u1']},{'editor':'beta','unit_ids':[]}])
    def test_member_order_and_private_context_are_not_normalized_away(self):
        op,p=hierarchy_request({'action':'form','level':3,'context_id':'editor-notes','cb_ids':['cb2','cb1']})
        self.assertEqual(op,'hierarchy.form');self.assertEqual(p['cb_ids'],['cb2','cb1']);self.assertEqual(p['context_id'],'editor-notes')
    def test_legacy_does_not_invent_level_or_wb_order(self):
        rows=hierarchy_rows({'cb_id':'old','wb_id':'wb','level':None,'hierarchy_index_status':'legacy-unindexed','wb_formation_order':'unknown','known_cb_order':{'membership_index':0}})
        self.assertIsNone(rows[0]['阶次']);self.assertEqual(rows[0]['WB形成顺序'],'unknown')

if __name__=='__main__':unittest.main()
