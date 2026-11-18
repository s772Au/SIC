#[path = "../src/editing.rs"]
mod editing;

use editing::{materialize, replay};
use serde_json::{Value, json};

fn source() -> Value {
    json!({"units":[
        {"unit_id":"u1","text":"alpha","anchor":{"source_id":"s","message":0},"continuation":false},
        {"unit_id":"u2","text":"世界","anchor":{"source_id":"s","message":1},"continuation":false},
        {"unit_id":"u3","text":"尾","anchor":{"source_id":"s","message":2},"continuation":false}
    ]})
}

fn one_editor(unit_ids: Value, ops: Value, pvi: Value) -> (Value, Value) {
    (
        json!([{"editor":"e1","unit_ids":unit_ids}]),
        json!({"e1":{"ops":ops,"pvi":pvi}}),
    )
}

#[test]
fn explicit_keep_preserves_current_text_and_replays_without_panicking() {
    let original = source();
    let (assignments, responses) = one_editor(
        json!(["u1", "u2", "u3"]),
        json!([{"op":"KEEP","target":"u1"},{"op":"REPLACE","target":"u2","content":"edited"},{"op":"KEEP","target":"u2"}]),
        json!({}),
    );
    let material = materialize(&original, &assignments, &responses, false).unwrap();
    assert_eq!(material["body"], "alpha\nedited\n尾");
    let replayed = replay(
        &original,
        &material["ops"],
        material["ops"].as_array().unwrap().len(),
        false,
    )
    .unwrap();
    assert_eq!(replayed["body"], material["body"]);
}

#[test]
fn ordered_ops_reorder_flag_utf8_and_default_keep() {
    let original = source();
    let assignments = json!([{"editor":"e1","unit_ids":["u2","u3"]}]);
    let responses = json!({"e1":{"ops":[
        {"op":"REPLACE","target":"u2","content":"改写"},
        {"op":"FLAG","target":"u2","content":"核对來源"},
        {"op":"REORDER","target":"u3","order":0}
    ],"pvi":{}}});
    let result = materialize(&original, &assignments, &responses, false).unwrap();
    assert_eq!(result["body"], "alpha\n尾\n改写\n[FLAG] 核对來源");
    assert_eq!(result["ops"][3]["op"], "KEEP");
    assert_eq!(result["ops"][3]["targets"], json!(["u1"]));
    assert_eq!(result["index"][0]["anchor"], original["units"][0]["anchor"]);
}

#[test]
fn add_before_after_and_unanchored_keep_order() {
    let original = json!({"units":[
        {"unit_id":"u1","text":"A","anchor":"a"},
        {"unit_id":"u2","text":"B","anchor":"b"}
    ]});
    let (assignments, responses) = one_editor(
        json!(["u1", "u2"]),
        json!([
            {"op":"ADD","content":"before","anchor":{"target":"u1","position":"before"}},
            {"op":"ADD","content":"after","anchor":{"target":"u1","position":"after"}},
            {"op":"ADD","content":"tail"}
        ]),
        json!({"P":["persistent"]}),
    );
    let result = materialize(&original, &assignments, &responses, false).unwrap();
    assert_eq!(result["body"], "before\nA\nafter\nB\ntail\npersistent");
    assert_eq!(result["ops"][3]["origin"], "editor-P");
}

#[test]
fn pvi_routes_content_and_validates_primary_decision_views() {
    let original = source();
    let assignments = json!([{"editor":"e1","unit_ids":["u1","u2","u3"]}]);
    let decision_tree = json!({"views":{
        "Working on":[{"text":"current","evidence":[]}],
        "Failed Before":[],"Not Tried Yet":[],"Overlooked":[]
    }});
    let responses = json!({"e1":{"ops":[
        {"op":"COMPRESS","target":"u1","content":"volatile replacement","lifecycle":"V"},
        {"op":"ADD","content":"temporary","lifecycle":"V"},
        {"op":"FLAG","target":"u2","content":decision_tree.to_string(),"lifecycle":"I","view_key":"decision_tree"}
    ],"pvi":{"P":["durable"],"V":["also temporary"],"I":{"status":{"ready":true}}}}});
    let result = materialize(&original, &assignments, &responses, true).unwrap();
    assert_eq!(result["body"], "世界\n尾\ndurable");
    assert_eq!(
        result["volatile"],
        json!(["volatile replacement", "temporary", "also temporary"])
    );
    assert_eq!(result["instant"]["status"], json!({"ready":true}));
    assert_eq!(result["instant"]["decision_tree"], decision_tree);

    let invalid = json!({"e1":{"ops":[],"pvi":{"I":{"decision_tree":{"views":{"Unknown":[]}}}}}});
    assert!(materialize(&original, &assignments, &invalid, true).is_err());
}

#[test]
fn instant_never_becomes_working_source_and_old_instant_is_not_reapplied() {
    let original = source();
    let assignments = json!([{"editor":"e1","unit_ids":["u1","u2","u3"]}]);
    // F25: a later round with no P/V/I must not resurrect the previous I text.
    let first = json!({"e1":{"ops":[],"pvi":{"I":{"status":{"ready":true}}}}});
    let first_result = materialize(&original, &assignments, &first, true).unwrap();
    assert_eq!(first_result["instant"]["status"], json!({"ready":true}));
    let second = json!({"e1":{"ops":[],"pvi":{}}});
    let second_result = materialize(&original, &assignments, &second, true).unwrap();
    // The body must be unchanged: I is a projection, never appended WB text.
    assert_eq!(second_result["body"], first_result["body"]);
    assert!(!second_result["body"].as_str().unwrap().contains("ready"));
    // The previous round's I must not be carried into this round: it is a
    // per-round projection, not accumulated state.
    assert_ne!(
        second_result["instant"],
        first_result["instant"],
        "a later round must not inherit the earlier instant view"
    );

    // I is not an editable target: naming it as a target must be refused.
    let edit_instant = json!({"e1":{"ops":[{"op":"DELETE","target":"instant"}],"pvi":{}}});
    assert!(materialize(&original, &assignments, &edit_instant, true).is_err());
    // Nor may ordinary I text carry a capability field into the editable view.
    let forged_mode = json!({"e1":{"ops":[{"op":"ADD","content":"x","lifecycle":"I","view_key":"mode"}],"pvi":{"I":{"mode":"editor"}}}});
    assert!(materialize(&original, &assignments, &forged_mode, true).is_err());
}

#[test]
fn rejects_scope_escalation_unknown_protocol_fields_and_instant_conflicts() {
    let original = source();
    let assignments = json!([{"editor":"e1","unit_ids":["u1"]}]);
    let escaped = json!({"e1":{"ops":[{"op":"DELETE","target":"u2"}],"pvi":{}}});
    assert!(materialize(&original, &assignments, &escaped, false).is_err());

    let protocol_edit = json!({"e1":{"ops":[{"op":"REPLACE","target":"u1","content":"x","tool_call_id":"forged"}],"pvi":{}}});
    assert!(materialize(&original, &assignments, &protocol_edit, false).is_err());

    let conflict = json!({"e1":{"ops":[{"op":"ADD","content":"x","lifecycle":"I","view_key":"state"}],"pvi":{"I":{"state":"other"}}}});
    assert!(materialize(&original, &assignments, &conflict, false).is_err());

    let unknown_pvi = json!({"e1":{"ops":[],"pvi":{"P":[],"credential":"no"}}});
    assert!(materialize(&original, &assignments, &unknown_pvi, false).is_err());
}

#[test]
fn rejects_add_to_deleted_anchor_and_flag_after_delete() {
    let original = source();
    let assignments = json!([{"editor":"e1","unit_ids":["u1","u2","u3"]}]);
    let add_to_deleted = json!({"e1":{"ops":[
        {"op":"DELETE","target":"u2"},
        {"op":"ADD","content":"orphan","anchor":{"target":"u2","position":"after"}}
    ],"pvi":{}}});
    assert!(materialize(&original, &assignments, &add_to_deleted, false).is_err());
    let flag_after_delete = json!({"e1":{"ops":[
        {"op":"DELETE","target":"u2"},
        {"op":"FLAG","target":"u2","content":"invalid"}
    ],"pvi":{}}});
    assert!(materialize(&original, &assignments, &flag_after_delete, false).is_err());
}

#[test]
fn replay_rebuilds_arbitrary_operation_prefix_without_external_effects_or_source_mutation() {
    let original = source();
    let snapshot = original.clone();
    let records = json!([
        {"op":"REPLACE","target":"u1","content":"first","editor":"e1"},
        {"op":"KEEP","targets":["u2","u3"],"origin":"implicit-program"},
        {"op":"ADD","content":"last","editor":"e1"}
    ]);
    let first = replay(&original, &records, 1, false).unwrap();
    let final_state = replay(&original, &records, 3, false).unwrap();
    assert_eq!(first["body"], "first\n世界\n尾");
    assert_eq!(final_state["body"], "first\n世界\n尾\nlast");
    assert_eq!(first["external_effects_replayed"], false);
    assert_eq!(final_state["operation_count"], 3);
    assert_eq!(original, snapshot);
    assert!(replay(&original, &records, 4, false).is_err());
}

#[test]
fn primary_decision_i_content_rejects_duplicate_json_keys() {
    let original = source();
    let assignments = json!([{"editor":"e1","unit_ids":["u1"]}]);
    let responses = json!({"e1":{"ops":[{"op":"FLAG","target":"u1","content":"{\"Working on\":[],\"Working on\":[]}","lifecycle":"I","view_key":"decision_tree"}],"pvi":{}}});
    assert!(materialize(&original, &assignments, &responses, true).is_err());
}
