"""New dedicated official-component acceptance over a real owned Core HTTP server."""
import copy
import json
import os
from pathlib import Path
import sys
import tempfile

PROJECT = Path(__file__).resolve().parents[2]
sys.path[:0] = [str(PROJECT), str(PROJECT / "integrations"), str(PROJECT / "scripts")]
from v2_experiment_fixture import OwnedFixture
from components.sic import SICWorkspace, SICCandidate, SICDecision, SICConfiguration, SICMemory, SICRoutes, SICBoard, SICNativeInput, SICEditorTask, SICRuntime, SICAccounting, SICCoverage


def main():
    saved = {key: os.environ.get(key) for key in ("SIC_CORE_URL", "SIC_CORE_TOKEN", "SIC_CORE_ACTOR")}
    evidence = []
    try:
        with tempfile.TemporaryDirectory(prefix="sic-workbenches-") as directory:
            with OwnedFixture(Path(directory) / "core.db") as fixture:
                os.environ.update(SIC_CORE_URL=fixture.client.origin, SIC_CORE_TOKEN=fixture.client.token, SIC_CORE_ACTOR="host")
                client = fixture.client
                client.call("chain.create", {"creators": []})
                chain = client.context["chain_id"]
                source = client.call("source.ingest", {"format": "openai-chat", "request": {"messages": [
                    {"role": "system", "content": "Protected system constraint"},
                    {"role": "user", "content": "First source evidence"}, {"role": "user", "content": "Second source stays KEEP"}]}})["source_id"]
                job = client.call("edit.prepare", {"source_id": source})["job_id"]
                client.call("edit.run_all", {"job_id": job})
                original = client.call("edit.candidate.get", {"job_id": job})

                def component(cls, **values):
                    value = cls()
                    value.set(chain_id=chain, **values)
                    value._pre_run_setup()
                    return value

                candidate = component(SICCandidate, job_id=job, action="修改一项操作", expected_revision=0,
                                      operation_number=1, op="REPLACE", target="u1", content="Human-reviewed candidate", reason="Clarify evidence", allow_write=True)
                updated = candidate.run_candidate().data
                assert updated["ok"] and updated["result"]["revision"] == 1, updated
                assert len(candidate.operations()) > 0
                assert len(candidate.originals()) > 0
                assert client.call("edit.candidate.get", {"job_id": job})["revision"] == 1, "Table output duplicated mutation"
                pvi = component(SICCandidate, job_id=job, action="修改 P/V/I", expected_revision=1, persistent_text="Reviewed P",
                                volatile_text="Reviewed V", instant_views=json.dumps({"note": "Reviewed I"}), allow_write=True)
                assert pvi.run_candidate().data["result"]["revision"] == 2
                assert len(pvi.lifecycles()) == 3
                current = client.call("edit.candidate.get", {"job_id": job})
                assert current["original_responses"] == original["original_responses"]
                stale = component(SICCandidate, job_id=job, action="追加操作", expected_revision=0, op="FLAG", target="u1", content="stale", allow_write=True).run_candidate().data
                assert stale["ok"] is False and stale["error"]["code"] == "candidate_stale"
                validate = component(SICCandidate, job_id=job, action="校验候选", expected_revision=2)
                assert validate.run_candidate().data["ok"]
                publication = component(SICCandidate, job_id=job, action="发布此版本", expected_revision=2, expected_cut=0, allow_write=True).run_candidate().data
                assert publication["ok"], publication
                view = client.call("cut.get")
                frozen = copy.deepcopy(view["view"]["cbs"][0])
                assert "Human-reviewed candidate" in frozen["body"] and "Reviewed P" in frozen["body"]
                evidence.append("model candidate CAS edit / original provenance / no duplicate write / stale rejection / PVI / immutable publication")

                workspace = component(SICWorkspace, source_id=source)
                assert workspace.read_workspace().data["ok"]
                comparison = workspace.comparison().to_dict("records")
                protocol = workspace.protocol().to_dict("records")
                assert len(comparison) == 2 and all(row["状态"] == "已发布 / 不可改写" for row in comparison)
                assert any(row["类别"] == "受保护指令" for row in protocol)
                evidence.append("original text / native protocol / fixed CB comparison with protected controls")

                for action in ("CB形成时序", "从WB找CB", "从CB找WB", "读取原文片段", "读取固定CB片段", "读取持久op片段", "读取原始提案与修订链", "读取历史I片段", "有限背景投影"):
                    memory = component(SICMemory, action=action, source_id=source, cb_id=frozen["cb_id"], cut_id=view["cut_id"])
                    assert memory.navigate().data["ok"], memory.navigate().data
                    assert len(memory.records()) > 0, action
                evidence.append("eight real memory directions: timeline / both source mappings / body and op/I pages / explicit bounded projection")

                decision = component(SICDecision, action="记录失败与原因", expected_revision=0, event_text="First approach failed because the file was wrong", evidence_source=source)
                decision.set(allow_write=True)
                assert decision.run_decision().data["ok"]
                assert any(row.get("事项", "").startswith("First approach") for row in decision.views().to_dict("records"))
                client.call("harness.event", {"type": "tool_start", "call_id": "workbench-tool"})
                client.call("harness.event", {"type": "tool_end", "call_id": "workbench-tool", "status": "failed", "error": "Owned tool fixture failed"})
                maintained = component(SICDecision, action="从新材料运行维护", allow_write=True)
                assert maintained.run_decision().data["ok"]
                assert "Owned tool fixture failed" in json.dumps(maintained.views().to_dict("records"))
                evidence.append("four-view actual sourced event and program maintenance, not a same-name JSON field only")

                configuration = component(SICConfiguration)
                assert configuration.run_configuration().data["ok"]
                assert len(configuration.profiles()) >= 2 and len(configuration.teaching()) > 0
                evidence.append("safe role/profile tokenizer-budget table and actual teaching")

                checkpoint = client.call("chain.checkpoint", {})["checkpoint_id"]
                routes = component(SICRoutes, action="浏览我的路线")
                assert routes.run_route().data["ok"] and len(routes.routes()) > 0
                history = component(SICRoutes, action="查看路线与历史点")
                assert any(row.get("历史checkpoint") == checkpoint for row in history.routes().to_dict("records"))
                evidence.append("owner-scoped real chain list / SIC historical checkpoints")

                board = component(SICBoard, action="认领或更新自己的任务", expected_revision=0, task_id="workbench-task", description="Inspect evidence", allow_write=True)
                assert board.run_board().data["ok"]
                assert any(row["认领路线"] == chain for row in board.tasks().to_dict("records"))
                assert client.call("cut.get")["view"]["cbs"][0] == frozen
                evidence.append("shared CAS board task / immutable CB unchanged across all workbench operations")
                client.call('runtime.run', {'max_steps': 8, 'continue_worker': False, 'editor_mode': 'program'})
                raw_text = r'{"literal":"C:\\example\\file", "expression":"$context.not_an_expression"}'
                native = component(SICNativeInput, working_text=raw_text, allow_write=True)
                new_source = native.ingest().data
                assert new_source['ok'], new_source
                assert client.call('source.read', {'source_id': new_source['context']['source_id']})['source']['units'][0]['text'] == raw_text
                assert len(native.units()) > 0
                page = component(SICMemory, action='读取固定CB片段', cb_id=frozen['cb_id']).navigate()
                task = component(SICEditorTask, action='准备单Editor', source_id=new_source['context']['source_id'],
                                 archive_page=page, include_archive_page=True, allow_write=True)
                prepared_task = task.run_task().data
                assert prepared_task['ok'], prepared_task
                assert prepared_task['task_view']['readonly_reads'][0]['object_id'] == frozen['cb_id']
                assert len(task.scope()) > 0 and len(task.background()) >= 3
                run = component(SICEditorTask, action='并发执行所有Editor', job_id=prepared_task['context']['job_id'], allow_write=True, allow_model=True)
                assert run.run_task().data['ok']
                material = json.loads(fixture.model_requests[-1]['request']['messages'][-1]['content'])
                assert material['readonly_archive_reads'][0]['object_id'] == frozen['cb_id']
                assert material['editable_text_units'][0]['unit_id'] == 'u1'
                runtime = component(SICRuntime)
                assert runtime.run_runtime().data['ok'] and len(runtime.events()) > 0
                ledger = component(SICAccounting)
                assert ledger.run_accounting().data['ok'] and len(ledger.charges()) > 0 and len(ledger.unknowns()) > 0
                coverage = SICCoverage()
                coverage._pre_run_setup()
                assert len(coverage.requirements()) == 65
                evidence.append('literal native text / program-prefetched archive frozen before Editor / actual model sees readonly page / runtime / ledger / 65 requirement statuses')
                report = {"status": "passed", "evidence": evidence, "official_component_types": 12,
                          "owned_http_model_calls": len(fixture.model_requests), "real_provider_calls": 0,
                          "browser_gui_verified": False, "studio_gui_verified": False,
                          "scope": "Dedicated components over Core HTTP; GUI acceptance remains separate"}
                destination = PROJECT / ".local/integration-test-results/workbench-acceptance.json"
                destination.parent.mkdir(parents=True, exist_ok=True)
                destination.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
                print(json.dumps(report, ensure_ascii=False))
    finally:
        for key, value in saved.items():
            if value is None:
                os.environ.pop(key, None)
            else:
                os.environ[key] = value


if __name__ == "__main__":
    main()
