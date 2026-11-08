from sic_workflow.workbench_component import WorkbenchComponent as Component
from lfx.io import BoolInput, CodeInput, DataInput, DropdownInput, IntInput, MultilineInput, StrInput, Output
from lfx.schema import Data, DataFrame
from sic_workflow.hierarchy_forms import hierarchy_request, hierarchy_rows
from sic_workflow.workbench_support import cached, context_of, error_rows, perform, reset, table


class SICHierarchy(Component):
    display_name = 'SIC 双链与递归工作台'
    description = '本阶WB/CB各自时序、同阶对应、有序成员形成真实高阶WB、角色采用和逐级回溯；旧未知顺序不补造。'
    icon = 'Network'
    name = 'SICHierarchy'
    inputs = [DataInput(name='previous', display_name='前一步', required=False), StrInput(name='chain_id', display_name='Agent工作路线', value=''),
              DropdownInput(name='action', display_name='层级动作', options=['overview', 'wb_timeline', 'cb_timeline', 'form', 'expand_cb', 'expand_wb', 'select', 'maintain', 'create_context', 'ingest_context', 'legacy', 'replay'], value='overview', real_time_refresh=True),
              StrInput(name='context_id', display_name='内部上下文（不创建Agent）', value='main'),
              IntInput(name='level', display_name='表示阶次（形成高阶须≥2）', value=1),
              MultilineInput(name='cb_ids', display_name='连续下阶CB成员（按序，每行一个）', value=''),
              StrInput(name='object_id', display_name='要展开/回放的CB或WB', value=''),
              MultilineInput(name='roles', display_name='采用角色（每行一个：worker/editor/editor:alpha/editor:beta）', value='worker\neditor'),
              MultilineInput(name='shared_contexts', display_name='共享已有上下文（每行一个）', value='main'),
              CodeInput(name='working_text', display_name='Editor独有持续正文（原样，不执行）', value=''),
              IntInput(name='fanout', display_name='程序维护每组成员数', value=2),
              BoolInput(name='deduplicate_exact', display_name='程序规则：精确去重', value=True),
              BoolInput(name='drop_empty', display_name='程序规则：空文本移出', value=False),
              BoolInput(name='normalize_newlines', display_name='程序规则：换行规范化', value=False),
              IntInput(name='offset', display_name='目录页起点', value=0), IntInput(name='limit', display_name='目录本页条数（≤100）', value=20),
              IntInput(name='step', display_name='回放op前缀（0原文，-1全部）', value=-1),
              BoolInput(name='allow_write', display_name='允许形成/采用/内部材料写入', value=False)]
    outputs = [Output(name='result', display_name='真实结果与版本绑定', method='run_hierarchy', group_outputs=True),
               Output(name='records', display_name='双链/成员/角色/回放表', method='records', group_outputs=True)]
    for _field in inputs:
        if _field.name not in {'previous','chain_id','action','allow_write'}: _field.show = False

    def _pre_run_setup(self): reset(self)

    def update_build_config(self, config, value, field_name=None):
        if field_name != 'action': return config
        fields = {'overview': set(), 'wb_timeline': {'context_id','level','offset','limit'}, 'cb_timeline': {'context_id','level','offset','limit'},
                  'form': {'context_id','level','cb_ids'}, 'expand_cb': {'object_id'}, 'expand_wb': {'object_id'},
                  'select': {'context_id','level','roles'}, 'maintain': {'context_id','fanout','roles','deduplicate_exact','drop_empty','normalize_newlines'},
                  'create_context': {'context_id','shared_contexts'}, 'ingest_context': {'context_id','working_text'},
                  'legacy': {'offset','limit'}, 'replay': {'object_id','step'}}[value]
        for field in self.inputs:
            if field.name not in {'previous','chain_id','action','allow_write'}:config[field.name]['show'] = field.name in fields
        return config

    def _act(self):
        lines = lambda x: [s.strip() for s in x.splitlines() if s.strip()]
        context = context_of(self)
        f = {key: getattr(self,key) for key in ('action','context_id','level','fanout','offset','limit','step','working_text')}
        f.update(cb_ids=lines(self.cb_ids), roles=lines(self.roles), shared_contexts=lines(self.shared_contexts),
                 object_id=self.object_id or context.get('cb_id',''),
                 rules=[key for key in ('deduplicate_exact','drop_empty','normalize_newlines') if getattr(self,key)])
        operation, payload = hierarchy_request(f)
        return perform(self, operation, payload, context=context)

    def run_hierarchy(self) -> Data: return Data(data=cached(self,self._act))

    def records(self) -> DataFrame:
        value = cached(self,self._act)
        return table(hierarchy_rows(value['result']) if value.get('ok') else error_rows(value))
