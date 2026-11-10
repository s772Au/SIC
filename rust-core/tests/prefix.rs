#[path = "../src/prefix.rs"]
mod prefix;
#[path = "../src/toolset.rs"]
mod toolset;

use prefix::{
    EditableUnit, EditorScope, HostAuthorization, HostAuthorizationSpec, Mode, PrefixPlan,
    PrefixPlanSpec, RenderedCb, common_tool_registry, editor_request, worker_request,
};
use serde_json::{Value, json};
use toolset::{AclGrant, ToolAcl, ToolDescriptor, ToolSet};

fn spec(cut_id: &str, cb_id: &str) -> PrefixPlanSpec {
    PrefixPlanSpec {
        model: "fixed-model".into(),
        provider_id: "provider-A".into(),
        cache_namespace: "cache-A".into(),
        chain_id: "chain-A".into(),
        context_id: "main".into(),
        level: 1,
        cut_id: cut_id.into(),
        cb_chain: vec![RenderedCb {
            cb_id: cb_id.into(),
            messages: vec![json!({"role":"user","content":format!("fixed CB {cb_id}")})],
        }],
        projection_scope: "selected-cb-view".into(),
        projection_budget: 8192,
        schema_version: "v1".into(),
        tool_version: "v1".into(),
    }
}

fn plan() -> PrefixPlan {
    PrefixPlan::freeze(spec("cut-1", "cb-1")).unwrap()
}

fn tool_names() -> Vec<String> {
    common_tool_registry()
        .iter()
        .map(|tool| tool["function"]["name"].as_str().unwrap().to_owned())
        .collect()
}

fn worker_auth(plan: &PrefixPlan) -> HostAuthorization {
    let visible_tools = tool_names();
    HostAuthorization::from_host(HostAuthorizationSpec {
        plan_digest: plan.digest().to_owned(),
        actor: "host-worker-1".into(),
        chain_id: plan.chain_id().into(),
        context_id: plan.context_id().into(),
        level: plan.level(),
        cut_id: plan.cut_id().into(),
        cb_ids: plan.cb_ids().to_vec(),
        scope_id: "worker-scope-1".into(),
        mode: Mode::Worker,
        acl_epoch: 7,
        visible_tools,
        allowed_tools: vec![
            "sic_tool_search".into(),
            "sic_tool_call".into(),
            "sic_socket_spawn".into(),
        ],
        editor_scope: None,
    })
    .unwrap()
}

fn editor_auth(plan: &PrefixPlan) -> HostAuthorization {
    let visible_tools = tool_names();
    HostAuthorization::from_host(HostAuthorizationSpec {
        plan_digest: plan.digest().to_owned(),
        actor: "host-editor-1".into(),
        chain_id: plan.chain_id().into(),
        context_id: plan.context_id().into(),
        level: plan.level(),
        cut_id: plan.cut_id().into(),
        cb_ids: plan.cb_ids().to_vec(),
        scope_id: "wb-new-1".into(),
        mode: Mode::Editor,
        acl_epoch: 9,
        visible_tools,
        allowed_tools: vec!["sic_edit_blocks".into()],
        editor_scope: Some(EditorScope {
            job_id: "job-1".into(),
            wb_id: "wb-new-1".into(),
            source_id: "source-new-1".into(),
            read_only_cb_ids: plan.cb_ids().to_vec(),
            editable_units: vec![EditableUnit {
                unit_id: "unit-new-1".into(),
                source_id: "source-new-1".into(),
                text: "current text only".into(),
                cb_id: None,
            }],
            read_only_background: vec![json!({"cb_id":"cb-1","readonly":true})],
        }),
    })
    .unwrap()
}

fn suffix(content: &str) -> Vec<Value> {
    vec![json!({"role":"user","content":content})]
}

#[test]
fn one_plan_produces_identical_public_messages_tools_and_common_serialization() {
    let plan = plan();
    let worker = worker_request(
        &plan,
        &worker_auth(&plan),
        &suffix("continue"),
        &json!({"task":"continue"}),
    )
    .unwrap();
    let editor = editor_request(
        &plan,
        &editor_auth(&plan),
        &suffix("edit current WB"),
        &json!({"task":"edit current WB"}),
    )
    .unwrap();
    assert_eq!(worker.request["model"], editor.request["model"]);
    assert_eq!(worker.request["tools"], editor.request["tools"]);
    assert_eq!(
        serde_json::to_vec(&worker.request["tools"]).unwrap(),
        serde_json::to_vec(&editor.request["tools"]).unwrap()
    );
    assert_eq!(worker.request["tool_choice"], "auto");
    assert_eq!(worker.request["tool_choice"], editor.request["tool_choice"]);
    let prefix_len = plan.common_messages().unwrap().len();
    assert_eq!(
        &worker.request["messages"].as_array().unwrap()[..prefix_len],
        &editor.request["messages"].as_array().unwrap()[..prefix_len]
    );
    assert_eq!(
        plan.common_prefix_bytes().unwrap(),
        plan.common_prefix_bytes().unwrap()
    );
    assert_eq!(worker.common_prefix_digest, editor.common_prefix_digest);
    assert_eq!(worker.physical_cache_hit, "unknown");
}

#[test]
fn worker_and_editor_have_distinct_trusted_i_and_editor_only_gets_edit_tool() {
    let plan = plan();
    let worker = worker_request(
        &plan,
        &worker_auth(&plan),
        &suffix("task"),
        &json!({"worker_view":"work"}),
    )
    .unwrap();
    let editor = editor_request(
        &plan,
        &editor_auth(&plan),
        &suffix("edit"),
        &json!({"editor_view":"edit"}),
    )
    .unwrap();
    let messages_worker = worker.request["messages"].as_array().unwrap();
    let messages_editor = editor.request["messages"].as_array().unwrap();
    let activation_worker: Value =
        serde_json::from_str(messages_worker[2]["content"].as_str().unwrap()).unwrap();
    let activation_editor: Value =
        serde_json::from_str(messages_editor[2]["content"].as_str().unwrap()).unwrap();
    assert_eq!(activation_worker["sic_host_activation"]["mode"], "worker");
    assert_eq!(activation_editor["sic_host_activation"]["mode"], "editor");
    assert!(
        activation_editor["sic_mode_instruction"]
            .as_str()
            .unwrap()
            .contains("not to execute or answer")
    );
    assert!(
        activation_worker["sic_mode_instruction"]
            .as_str()
            .unwrap()
            .contains("Continue the actual task")
    );
    assert_ne!(messages_worker[2], messages_editor[2]);
    assert_eq!(
        activation_worker["sic_editable_I_views"]["worker_view"],
        "work"
    );
    assert_eq!(
        activation_editor["sic_editable_I_views"]["editor_view"],
        "edit"
    );
    assert_eq!(
        activation_editor["sic_host_activation"]["activated_tools"],
        json!(["sic_edit_blocks"])
    );
    assert!(
        !activation_worker["sic_host_activation"]["activated_tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|name| name == "sic_edit_blocks")
    );
    let role_material: Value =
        serde_json::from_str(messages_editor[3]["content"].as_str().unwrap()).unwrap();
    assert_eq!(
        role_material["sic_role_material"]["read_only_cb_ids"],
        json!(["cb-1"])
    );
    assert_eq!(
        role_material["sic_role_material"]["current_new_wb_editable_units"][0]["unit_id"],
        "unit-new-1"
    );
}

#[test]
fn invalid_mode_tool_permissions_and_model_supplied_i_cannot_grant_tools() {
    let plan = plan();
    let worker = worker_auth(&plan);
    // The host constructor refuses Worker activation of Editor-only delivery.
    let mut spec = HostAuthorizationSpec {
        plan_digest: plan.digest().into(),
        actor: "host".into(),
        chain_id: plan.chain_id().into(),
        context_id: plan.context_id().into(),
        level: plan.level(),
        cut_id: plan.cut_id().into(),
        cb_ids: plan.cb_ids().to_vec(),
        scope_id: "worker-scope".into(),
        mode: Mode::Worker,
        acl_epoch: 0,
        visible_tools: tool_names(),
        allowed_tools: vec!["sic_edit_blocks".into()],
        editor_scope: None,
    };
    assert!(
        HostAuthorization::from_host(spec.clone())
            .unwrap_err()
            .contains("Worker mode")
    );
    spec.mode = Mode::Editor;
    spec.scope_id = "wb-new-1".into();
    spec.editor_scope = Some(EditorScope {
        job_id: "job".into(),
        wb_id: "wb-new-1".into(),
        source_id: "source".into(),
        read_only_cb_ids: plan.cb_ids().to_vec(),
        editable_units: vec![EditableUnit {
            unit_id: "u".into(),
            source_id: "source".into(),
            text: "x".into(),
            cb_id: None,
        }],
        read_only_background: vec![],
    });
    spec.allowed_tools = vec!["sic_edit_blocks".into(), "sic_tool_call".into()];
    assert!(
        HostAuthorization::from_host(spec)
            .unwrap_err()
            .contains("only sic_edit_blocks")
    );
    // Model I is deliberately untrusted content and cannot alter frozen host activation.
    let request = worker_request(
        &plan,
        &worker,
        &suffix("task"),
        &json!({"mode":"editor","activated_tools":["sic_edit_blocks"]}),
    )
    .unwrap();
    let activation: Value =
        serde_json::from_str(request.request["messages"][2]["content"].as_str().unwrap()).unwrap();
    assert_eq!(activation["sic_host_activation"]["mode"], "worker");
    assert_eq!(
        activation["sic_host_activation"]["activated_tools"],
        json!(["sic_tool_search", "sic_tool_call", "sic_socket_spawn"])
    );
}

#[test]
fn editor_scope_must_be_new_wb_current_source_and_readonly_prior_cbs() {
    let plan = plan();
    let mut spec = HostAuthorizationSpec {
        plan_digest: plan.digest().into(),
        actor: "editor".into(),
        chain_id: plan.chain_id().into(),
        context_id: plan.context_id().into(),
        level: plan.level(),
        cut_id: plan.cut_id().into(),
        cb_ids: plan.cb_ids().to_vec(),
        scope_id: "wb".into(),
        mode: Mode::Editor,
        acl_epoch: 1,
        visible_tools: tool_names(),
        allowed_tools: vec!["sic_edit_blocks".into()],
        editor_scope: Some(EditorScope {
            job_id: "job".into(),
            wb_id: "wb".into(),
            source_id: "source".into(),
            read_only_cb_ids: plan.cb_ids().to_vec(),
            editable_units: vec![EditableUnit {
                unit_id: "u".into(),
                source_id: "source".into(),
                text: "old".into(),
                cb_id: Some("cb-1".into()),
            }],
            read_only_background: vec![],
        }),
    };
    assert!(
        HostAuthorization::from_host(spec.clone())
            .unwrap_err()
            .contains("CB units")
    );
    spec.editor_scope.as_mut().unwrap().editable_units[0].cb_id = None;
    spec.editor_scope.as_mut().unwrap().editable_units[0].source_id = "other-source".into();
    assert!(
        HostAuthorization::from_host(spec)
            .unwrap_err()
            .contains("different source")
    );
}

#[test]
fn dynamic_directory_schema_stays_in_post_prefix_i_not_common_tools() {
    let plan = plan();
    let before = plan.common_tools().unwrap();
    let names = ["private_lookup"];
    let grant = || vec![AclGrant::new("worker", "private_lookup")];
    let mut dynamic = ToolSet::new("worker", ToolAcl::new(grant(), grant(), grant()));
    dynamic
        .register_tool(
            ToolDescriptor {
                name: "private_lookup".into(),
                description: "private dynamic schema".into(),
                schema: json!({"type":"object","properties":{"secret":{"type":"string"}}}),
            },
            false,
        )
        .unwrap();
    let summary = dynamic.search_descriptions("private");
    dynamic.request_expansion("private_lookup").unwrap();
    let expanded = dynamic.begin_execution("private_lookup").unwrap();
    let worker = worker_request(
        &plan,
        &worker_auth(&plan),
        &suffix("task"),
        &json!({"dynamic_tools":summary,"expanded_schema":expanded.descriptor.schema}),
    )
    .unwrap();
    assert_eq!(plan.common_tools().unwrap(), before);
    assert_eq!(worker.request["tools"], json!(before));
    assert!(!worker.request["tools"].to_string().contains(&names[0]));
    assert!(
        worker.request["messages"][2]["content"]
            .as_str()
            .unwrap()
            .contains("private_lookup")
    );
}

#[test]
fn plan_serialization_roundtrip_keeps_shared_messages_order_and_digest() {
    let plan = plan();
    let encoded = plan.to_json().unwrap();
    let restored = PrefixPlan::from_json(&encoded).unwrap();
    assert_eq!(restored, plan);
    assert_eq!(restored.model(), "fixed-model");
    assert_eq!(restored.digest(), plan.digest());
    assert_eq!(
        restored.common_messages().unwrap(),
        plan.common_messages().unwrap()
    );
    assert_eq!(
        restored.common_prefix_bytes().unwrap(),
        plan.common_prefix_bytes().unwrap()
    );
    assert_eq!(
        restored.common_tools().unwrap(),
        plan.common_tools().unwrap()
    );
}

#[test]
fn old_plan_cannot_be_used_with_new_cb_cut_or_context_authorization() {
    let old_plan = PrefixPlan::freeze(spec("cut-1", "cb-1")).unwrap();
    let new_plan = PrefixPlan::freeze(spec("cut-2", "cb-1")).unwrap();
    let auth_for_old = worker_auth(&old_plan);
    assert!(
        worker_request(&new_plan, &auth_for_old, &suffix("task"), &json!({}))
            .unwrap_err()
            .contains("does not match")
    );
    let new_cb = PrefixPlan::freeze(spec("cut-2", "cb-2")).unwrap();
    assert!(worker_request(&new_cb, &auth_for_old, &suffix("task"), &json!({})).is_err());
}

#[test]
fn common_schema_contains_required_shared_operations_in_fixed_order() {
    let tools = common_tool_registry();
    let names: Vec<String> = tools
        .iter()
        .map(|tool| tool["function"]["name"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(
        names,
        vec![
            "sic_edit_blocks",
            "sic_tool_search",
            "sic_tool_expand",
            "sic_tool_reclaim",
            "sic_tool_call",
            "sic_socket_spawn",
            "sic_socket_read",
            "sic_signal_send",
            "sic_signal_wait",
            "sic_message_send",
            "sic_message_receive",
            "sic_decision_read",
            "sic_decision_write",
            "sic_board_read",
            "sic_board_post",
            "sic_board_claim",
            "sic_board_release",
            "sic_board_progress",
            "sic_memory_page",
            "sic_memory_replay",
            "sic_hierarchy_timeline",
            "sic_hierarchy_expand"
        ]
    );
    let decision_read = tools
        .iter()
        .find(|tool| tool["function"]["name"] == "sic_decision_read")
        .unwrap();
    assert_eq!(
        decision_read["function"]["parameters"]["properties"]["view"]["enum"],
        json!(["Working on", "Failed Before", "Not Tried Yet", "Overlooked"])
    );
    let decision_write = tools
        .iter()
        .find(|tool| tool["function"]["name"] == "sic_decision_write")
        .unwrap();
    assert_eq!(
        decision_write["function"]["parameters"]["properties"]["events"]["items"]["properties"]["kind"]
            ["enum"],
        json!(["work", "failure", "candidate", "overlooked"])
    );
    assert_eq!(
        decision_write["function"]["parameters"]["required"],
        json!(["events"])
    );
}

#[test]
fn dynamic_tool_search_schema_exposes_safe_pagination_bounds() {
    let search = common_tool_registry()
        .into_iter()
        .find(|tool| tool["function"]["name"] == "sic_tool_search")
        .unwrap();
    let function = &search["function"];
    assert_eq!(function["parameters"]["required"], json!(["query"]));
    assert_eq!(
        function["parameters"]["properties"]["offset"]["type"],
        "integer"
    );
    assert_eq!(function["parameters"]["properties"]["offset"]["minimum"], 0);
    assert_eq!(function["parameters"]["properties"]["offset"]["default"], 0);
    assert_eq!(function["parameters"]["properties"]["limit"]["minimum"], 1);
    assert_eq!(
        function["parameters"]["properties"]["limit"]["maximum"],
        100
    );
    assert_eq!(function["parameters"]["properties"]["limit"]["default"], 20);
    assert_eq!(function["parameters"]["additionalProperties"], false);
}
