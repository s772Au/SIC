from sic_workflow.workbench_component import WorkbenchComponent as Component
from lfx.io import BoolInput, CodeInput, DataInput, DropdownInput, StrInput, Output
from lfx.schema import Data, DataFrame
from sic_workflow.editor_views import task_view, rows as task_rows
from sic_workflow.workbench_support import cached, context_of, error_rows, object_value, perform, reset, table, text


class SICEditorTask(Component):
    display_name = 'SIC Editor任务与只读档案'
    description = '程序先冻结编辑范围和所需档案页，再由每位Editor单轮作答；档案与原始交付不成为可改目标。'
    icon = 'ClipboardList'
    name = 'SICEditorTask'
    inputs = [DataInput(name='previous', display_name='前一步', required=False),
              StrInput(name='chain_id', display_name='工作路线', value=''),
              DropdownInput(name='action', display_name='任务动作', options=['查看冻结任务', '准备单Editor', '准备双Editor分工', '执行一位Editor单轮', '并发执行所有Editor'], value='查看冻结任务', real_time_refresh=True),
              StrInput(name='source_id', display_name='待处理WB', value=''),
              StrInput(name='job_id', display_name='已冻结任务', value=''),
              StrInput(name='editor', display_name='Editor身份', value='editor-1'),
              StrInput(name='editor_one_name', display_name='第一Editor身份（与editor:alpha选择对应）', value='editor-1'),
              StrInput(name='editor_two_name', display_name='第二Editor身份', value='editor-2'),
              StrInput(name='editor_one_units', display_name='Editor 1单元（逗号分隔）', value='u1'),
              StrInput(name='editor_two_units', display_name='Editor 2单元（逗号分隔）', value='u2'),
              DataInput(name='archive_page', display_name='导航器已选档案页（只读）', required=False),
              BoolInput(name='include_archive_page', display_name='准备时预取并冻结已选页', value=False),
              CodeInput(name='readonly_reads', display_name='其他只读页定位数组（高级）', value='[]', advanced=True),
              BoolInput(name='allow_write', display_name='允许冻结/执行此任务', value=False),
              BoolInput(name='allow_model', display_name='允许已选Editor模型单轮请求', value=False)]
    outputs = [Output(name='result', display_name='真实任务与范围', method='run_task', group_outputs=True),
               Output(name='scope', display_name='可改单元 / 已冻结分工', method='scope', group_outputs=True),
               Output(name='layout', display_name='冻结计划 / 实际请求 / fallback（只读）', method='layout', group_outputs=True),
               Output(name='aliases', display_name='局部别名 → canonical范围（只读）', method='aliases', group_outputs=True),
               Output(name='background', display_name='只读CB / I / 档案 / 原始响应', method='background', group_outputs=True)]

    def _pre_run_setup(self):
        reset(self)

    def update_build_config(self, build_config, field_value, field_name=None):
        if field_name == 'action':
            preparing = field_value.startswith('准备')
            for key in ('source_id', 'archive_page', 'include_archive_page', 'readonly_reads'):
                build_config[key]['show'] = preparing
            for key in ('editor_one_units', 'editor_two_units', 'editor_one_name', 'editor_two_name'):
                build_config[key]['show'] = field_value == '准备双Editor分工'
            build_config['editor']['show'] = field_value == '执行一位Editor单轮'
            build_config['allow_model']['show'] = field_value in {'执行一位Editor单轮', '并发执行所有Editor'}
        return build_config

    def _act(self):
        context = context_of(self)
        job = self.job_id or context.get('job_id')
        if self.action.startswith('准备'):
            reads = object_value(self.readonly_reads, list)
            if self.include_archive_page:
                page = self.archive_page.data if self.archive_page else {}
                if not page.get('ok') or page.get('operation') != 'memory.page':
                    raise ValueError('Connect a successful immutable memory.page result first')
                body = page['result']
                reads.append({key: body[key] for key in ('kind', 'object_id', 'offset', 'limit')})
            payload = {'source_id': self.source_id or context.get('source_id'), 'readonly_reads': reads}
            if self.action == '准备双Editor分工':
                payload['assignments'] = [{'editor': name, 'unit_ids': [item.strip() for item in units.split(',') if item.strip()]}
                                          for name, units in ((self.editor_one_name,self.editor_one_units),(self.editor_two_name,self.editor_two_units))]
            result = perform(self, 'edit.prepare', payload, context=context)
        elif self.action == '执行一位Editor单轮':
            result = perform(self, 'edit.run', {'job_id': job, 'editor': self.editor}, context=context)
        elif self.action == '并发执行所有Editor':
            result = perform(self, 'edit.run_all', {'job_id': job}, context=context)
        else:
            result = perform(self, 'chain.get', context=context)
        if result.get('ok'):
            snapshot = perform(self, 'chain.get', context=result['context'])
            job = result['context'].get('job_id', job)
            raw_job = snapshot.get('result', {}).get('jobs', {}).get(job, {})
            result['task_view'] = task_view(raw_job) if raw_job else {}
            if isinstance(result.get('result',{}).get('job'),dict):result['result']['job']=result['task_view']
            if result.get('operation')=='chain.get':result['result']={'job_id':job,'job':result['task_view'],'selection_readonly':True}
            if 'base_cut' in result['task_view']:
                result['context']['reviewed_cut'] = result['task_view']['base_cut']
            if self.action.startswith('准备') and reads and not result['task_view'].get('readonly_reads'):
                return {'ok': False, 'context': result['context'], 'operation': 'edit.prepare',
                        'error': {'code': 'archive_prefetch_unavailable', 'message': 'Core created the task but did not freeze the requested archive pages. Do not run this job; reload the supported core or explicitly prepare another task.'},
                        'evidence': {'task_created': True, 'model_called': False}}
        return result

    def run_task(self) -> Data:
        return Data(data=cached(self, self._act))

    def scope(self) -> DataFrame:
        value = cached(self, self._act)
        if not value.get('ok'):
            return table(error_rows(value))
        job = value.get('task_view', {})
        assignments = {uid: assignment['editor'] for assignment in job.get('assignments', []) for uid in assignment['unit_ids']}
        return table([{'单元': unit['unit_id'], '可改正文': unit['text'], '冻结Editor': assignments.get(unit['unit_id'], 'KEEP'),
                       '来源锚（只读）': text(unit.get('anchor', {})), '状态': job.get('status')}
                      for unit in job.get('source_snapshot', {}).get('units', [])])

    def background(self) -> DataFrame:
        value = cached(self, self._act)
        if not value.get('ok'):
            return table(error_rows(value))
        job = value.get('task_view', {})
        rows = [{'来源': '固定CB有限投影', '只读材料': text(job.get('background', [])), '未读与导航': text(job.get('background_manifest', {}))},
                {'来源': '覆写时I快照', '只读材料': text(job.get('instant_reference', {}))},
                {'来源': '程序预取冻结档案页', '只读材料': text(job.get('readonly_reads', []))}]
        rows.extend({'来源': ('canonical转换结果（原模型响应单独引用） / ' if response.get('canonical_response_id') else '原始单轮交付 / ') + editor, '只读材料': text(response)} for editor, response in job.get('responses', {}).items())
        for editor, background in job.get('editor_backgrounds', {}).items():
            rows.append({'来源':'该Editor实际冻结背景 / '+editor, '只读材料':text(background),
                         '当前任务所属':text({'context_id':job.get('context_id'),'level':job.get('level'),'wb_id':job.get('wb_id'),'base_cut':job.get('base_cut')})})
        for editor, instant in job.get('editor_instants', {}).items():
            rows.append({'来源':'该Editor实际冻结I / '+editor,'只读材料':text(instant)})
        return table(rows)

    def layout(self) -> DataFrame:
        value=cached(self,self._act)
        return table(task_rows(value.get('task_view',{}))['prefix_rows']) if value.get('ok') else table(error_rows(value))

    def aliases(self) -> DataFrame:
        value=cached(self,self._act)
        return table(task_rows(value.get('task_view',{}))['alias_rows']) if value.get('ok') else table(error_rows(value))
