import json

from sic_workflow.workbench_component import WorkbenchComponent as Component
from lfx.io import BoolInput, CodeInput, DataInput, DropdownInput, IntInput, MultilineInput, StrInput, Output
from lfx.schema import Data, DataFrame
from sic_workflow.prefix_forms import prefix_patch, parse_identity, safe_settings, safe_meta
from sic_workflow.workbench_support import cached, context_of, error_rows, perform, reset, table, text


class SICConfiguration(Component):
    display_name = "SIC 角色、教学与预算"
    description = "只选择宿主允许的固定profile；密钥和端点不进入画布。已开始任务保留原绑定。R03/R27/R34/R40。"
    icon = "Settings2"
    name = "SICConfiguration"
    inputs = [DataInput(name="previous", display_name="前一步", required=False),
              StrInput(name="chain_id", display_name="工作路线", value=""),
              DropdownInput(name="action", display_name="配置动作", options=["查看实际配置", "保存授权配置"], value="查看实际配置"),
              DropdownInput(name="worker_profile", display_name="Worker固定profile", options=[""], value="", refresh_button=True, combobox=True),
              DropdownInput(name="editor_mapping_mode", display_name="Editor映射修改范围", options=["保留现有映射", "合并指定映射", "完整替换映射", "清空映射"], value="合并指定映射", real_time_refresh=True, info="合并保留其他身份；完整替换移除未列身份；清空让新任务使用宿主默认。旧job绑定不变。"),
              StrInput(name="editor_one_name", display_name="Editor自身名称（如alpha，不含editor:前缀）", value="editor-1"),
              DropdownInput(name="editor_one", display_name="上述Editor的固定profile", options=[""], value="", refresh_button=True, combobox=True),
              StrInput(name="editor_two_name", display_name="另一Editor自身名称（可选）", value="editor-2"),
              DropdownInput(name="editor_two", display_name="另一Editor的固定profile（可选）", options=[""], value="", refresh_button=True, combobox=True),
              CodeInput(name="editor_profiles_json", display_name="任意数量Editor映射（名称→允许profile）", value="{}", advanced=True, info="合并模式为增量映射；完整替换模式为全部新映射；清空模式忽略本字段。"),
              BoolInput(name="replace_prefix_layout", display_name="本次更新前缀布局 / B设置", value=False),
              DropdownInput(name="prefix_layout", display_name="请求布局（不保证实际共享）", options=["separate", "shared_i"], value="separate"),
              IntInput(name="shared_prefix_budget", display_name="统一共享投影字符预算（>=512）", value=4096),
              DropdownInput(name="prefix_identity_mode", display_name="非敏感逻辑身份声明", options=["keep", "shared", "per_role"], value="keep", info="keep保留；shared统一三字段；per_role整体替换角色声明。不是物理模型/缓存证明。"),
              StrInput(name="shared_model", display_name="共同声明model", value=""),
              StrInput(name="shared_provider_id", display_name="共同provider逻辑标识（非密钥）", value=""),
              StrInput(name="shared_cache_namespace", display_name="共同cache_namespace（非凭据）", value=""),
              CodeInput(name="shared_prefix_identity_json", display_name="按角色声明worker/editor（高级）", value="{}", advanced=True),
              BoolInput(name="replace_teaching", display_name="本次更新教学与提示", value=False),
              MultilineInput(name="worker_teaching", display_name="Worker工作台与追溯教学", value=""),
              MultilineInput(name="editor_prompt", display_name="Editor任务与编辑提示", value=""),
              BoolInput(name="deduplicate_exact", display_name="规则：精确重复去重", value=True),
              BoolInput(name="drop_empty", display_name="规则：空文本处理", value=False),
              BoolInput(name="normalize_newlines", display_name="规则：规范化换行", value=False),
              DropdownInput(name="schedule_policy", display_name="编辑未完成时", options=["wait", "interleave"], value="wait"),
              IntInput(name="interleave_budget", display_name="每WB额外推进预算", value=1),
              BoolInput(name="allow_write", display_name="允许本次配置变更", value=False)]
    outputs = [Output(name="result", display_name="配置与生效版本", method="run_configuration", group_outputs=True),
               Output(name="profiles", display_name="实际角色 / profile / tokenizer / 容量", method="profiles", group_outputs=True),
               Output(name="teaching", display_name="工作教学与规则", method="teaching", group_outputs=True)]

    def _pre_run_setup(self):
        reset(self)

    def update_build_config(self, build_config, field_value, field_name=None):
        if field_name in {"worker_profile", "editor_one", "editor_two"}:
            result = perform(self, "config.get")
            if result.get("ok"):
                for key in ("worker_profile", "editor_one", "editor_two"):
                    build_config[key]["options"] = [""] + list(result["result"].get("profiles", []))
        if field_name in {"action", "editor_mapping_mode"}:
            mode = field_value if field_name == "editor_mapping_mode" else build_config["editor_mapping_mode"]["value"]
            for key in ("editor_one_name", "editor_one", "editor_two_name", "editor_two"):
                build_config[key]["show"] = mode == "合并指定映射"
            build_config["editor_profiles_json"]["show"] = mode in {"合并指定映射", "完整替换映射"}
        return build_config

    @staticmethod
    def _identity(name):
        if not isinstance(name, str) or not name or name != name.strip() or any(c.isspace() or not c.isprintable() for c in name) or name.startswith("editor:"):
            raise ValueError("Editor名称须为非空、无空白的自身名称；如alpha，而不是角色选择editor:alpha。")
        return name

    @staticmethod
    def _mapping(value):
        def unique(pairs):
            result = {}
            for key, item in pairs:
                if key in result:
                    raise ValueError("同一Editor在JSON中重复出现。")
                result[key] = item
            return result
        result = value if isinstance(value, dict) else json.loads(value or "{}", object_pairs_hook=unique)
        if not isinstance(result, dict):
            raise ValueError("Editor映射须为名称到profile的JSON对象。")
        for name, profile in result.items():
            SICConfiguration._identity(name)
            if not isinstance(profile, str) or not profile:
                raise ValueError("profile须为非空允许名称；删除请明确完整替换或清空。")
        return result

    def _editor_mapping(self, current):
        mode = self.editor_mapping_mode
        if mode == "保留现有映射":
            return None
        if mode == "清空映射":
            return {}
        changes = self._mapping(self.editor_profiles_json)
        if mode == "合并指定映射":
            names = set()
            for name, profile in ((self.editor_one_name, self.editor_one), (self.editor_two_name, self.editor_two)):
                if not profile:
                    continue
                self._identity(name)
                if name in names or name in changes and changes[name] != profile:
                    raise ValueError("同一Editor有冲突或重复的命名输入。")
                names.add(name);changes[name] = profile
            merged = dict(current.get("configuration", {}).get("editor_profiles", {}))
            merged.update(changes)
        elif mode == "完整替换映射":
            merged = changes
        else:
            raise ValueError("未知Editor映射修改范围。")
        allowed = set(current.get("profiles", []))
        if any(profile not in allowed for profile in changes.values()):
            raise ValueError("选择了宿主未允许的profile；未写入配置，也未派发模型。")
        return merged

    def _act(self):
        context = context_of(self)
        if self.action == "保存授权配置":
            payload = {"program_rules": [key for key in ("deduplicate_exact", "drop_empty", "normalize_newlines") if getattr(self, key)],
                       "schedule_policy": self.schedule_policy, "interleave_budget": self.interleave_budget}
            if self.worker_profile:
                payload["worker_profile"] = self.worker_profile
            current = perform(self, "config.get", context=context)
            if not current.get("ok"):
                return current
            try:
                editors = self._editor_mapping(current["result"])
                fields = {key: getattr(self,key) for key in ("replace_prefix_layout","prefix_layout","shared_prefix_budget","prefix_identity_mode","shared_model","shared_provider_id","shared_cache_namespace")}
                if self.replace_prefix_layout and self.prefix_identity_mode=="per_role":fields["shared_prefix_identity"]=parse_identity(self.shared_prefix_identity_json)
                payload.update(prefix_patch(fields))
            except (ValueError, TypeError) as error:
                return {"ok": False, "operation": "config.update", "context": context,
                        "error": {"code": "configuration_invalid", "message": str(error)}}
            if editors is not None:
                payload["editor_profiles"] = editors
            if self.replace_teaching:
                payload.update(worker_prompt=self.worker_teaching, editor_prompt=self.editor_prompt)
            result = perform(self, "config.update", payload, context=context)
            if not result.get("ok"):
                return result
        return perform(self, "config.get", context=context)

    def run_configuration(self) -> Data:
        return Data(data=cached(self, self._act))

    def profiles(self) -> DataFrame:
        envelope = cached(self, self._act)
        if not envelope.get("ok"):
            return table(error_rows(envelope))
        result = envelope["result"]
        rows = []
        for kind in ("safe_default_roles", "safe_profiles"):
            for name, value in result.get(kind, {}).items():
                rows.append({"类别": "默认角色" if kind == "safe_default_roles" else "宿主允许profile", "名称": name,
                             "模型": value.get("model"), "路由标识": value.get("route_id"), "tokenizer": value.get("tokenizer_model"),
                             "tokenizer证据": value.get("tokenizer_status", "unknown"), "最大输入": value.get("max_input_tokens"),
                             "最大输出": value.get("max_output_tokens"), "上下文容量": value.get("context_window"),
                             "兼容收益": "同系列仅待验假设"})
        return table(rows)

    def teaching(self) -> DataFrame:
        envelope = cached(self, self._act)
        if not envelope.get("ok"):
            return table(error_rows(envelope))
        result = envelope["result"]
        configuration = result.get("configuration", {})
        return table([{"项目": "实际Worker教学", "内容": result.get("active_worker_teaching", "unknown")},
                      {"项目": "当前Editor身份→profile（未绑定者用默认）", "内容": text(configuration.get("editor_profiles", {}))},
                      {"项目": "Editor提示", "内容": configuration.get("editor_prompt", "宿主默认；以冻结job为准")},
                      {"项目": "程序规则", "内容": text(configuration.get("program_rules", []))},
                      {"项目": "推进政策与预算", "内容": text({key: configuration.get(key) for key in ("schedule_policy", "interleave_budget")})},
                      {"项目": "前缀布局声明（非物理命中证明）", "内容": text(safe_settings(configuration))},
                      {"项目": "最近实际请求布局 / fallback", "内容": text(safe_meta(result.get("prefix_layout_status", {})))},
                      {"项目": "冻结计划只读分页定位", "内容": text({"kind":"prefix_plan","object_ids":result.get("shared_prefix_plan_ids",[])})},
                      {"项目": "生效范围", "内容": text(result.get("activation", "未来任务/发布；已开始job不变"))}])
