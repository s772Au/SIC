"""Render independent verified experiment through both official workbenches."""
import argparse
import json
from pathlib import Path
import sys

ROOT = Path(__file__).resolve().parents[2]
sys.path[:0] = [str(ROOT), str(ROOT / 'integrations')]
from components.sic.accounting import SICAccounting
from sic_workflow.accounting_graph import graph


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('run_directory', help='Project-relative completed run')
    args = parser.parse_args()
    component = SICAccounting()
    component.set(action='读取独立核验实验', run_directory=args.run_directory)
    component._pre_run_setup()
    result = component.run_accounting().data
    assert result['ok'], result
    assert result['verification']['status'] == 'passed'
    assert len(component.charges()) > 0 and len(component.unknowns()) > 0
    studio = graph.invoke({'action': 'verified_experiment', 'run_directory': args.run_directory})
    assert studio['result']['ok'] and studio['verification']['status'] == 'passed'
    assert studio['charge_rows'] and studio['unknown'] is not None
    assert studio['result']['result'] == result['result']
    assert result['result']['quality']['status'] == 'not_evaluated'
    report = {'status': 'passed', 'official_langflow_component': True, 'official_langgraph_graph': True,
              'independent_manifest_verified_before_display': True, 'matching_comparison': True,
              'quality': 'not_evaluated', 'model_calls': 0, 'gui_visual_verified': False}
    (ROOT / '.local/integration-test-results/experiment-workbench-acceptance.json').write_text(json.dumps(report, indent=2) + '\n', encoding='utf-8')
    print(json.dumps(report))


if __name__ == '__main__':
    main()
