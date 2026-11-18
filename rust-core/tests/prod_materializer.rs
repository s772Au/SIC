use serde_json::{Value, json};
use sic_core_rust::editing;
fn owned_text(material: &Value, id: &str) -> Vec<String> {
    let body = material["body"].as_str().unwrap();
    material["index"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["unit_id"] == id)
        .unwrap()["p_text_ranges"]
        .as_array()
        .unwrap()
        .iter()
        .map(|range| {
            body[range["utf8_start"].as_u64().unwrap() as usize
                ..range["utf8_end"].as_u64().unwrap() as usize]
                .to_owned()
        })
        .collect()
}
#[test]
fn actual_utf8_ranges_exclude_free_add_and_p_while_handling_continuation_flags_and_reorder() {
    let source = json!({"units":[{"unit_id":"u1","text":"甲","anchor":{"tag":"first"}},{"unit_id":"u2","text":"乙","continuation":true,"anchor":{"tag":"second"}}]});
    let assignments = json!([{"editor":"e","unit_ids":["u1","u2"]}]);
    let m=editing::materialize(&source,&assignments,&json!({"e":{"ops":[{"op":"ADD","content":"free","anchor":{"target":"u1","position":"before"}},{"op":"FLAG","target":"u1","content":"risk"},{"op":"ADD","content":"tail","anchor":{"target":"u2","position":"after"}},{"op":"ADD","content":"loose"}],"pvi":{"P":["persist"]}}}),true).unwrap();
    assert_eq!(owned_text(&m, "u1"), vec!["甲\n[FLAG] risk"]);
    assert_eq!(owned_text(&m, "u2"), vec!["乙"]);
    let reordered = editing::materialize(
        &source,
        &assignments,
        &json!({"e":{"ops":[{"op":"REORDER","target":"u2","order":-1}],"pvi":{}}}),
        true,
    )
    .unwrap();
    assert_eq!(reordered["body"], "乙\n甲");
    assert_eq!(reordered["index"][0]["p_text_ranges"][0]["utf8_start"], 4);
    assert_eq!(reordered["index"][1]["p_text_ranges"][0]["utf8_start"], 0);
}
#[test]
fn actual_v_index_preserves_editor_order_and_does_not_attribute_free_pvi_to_a_source() {
    let source = json!({"units":[{"unit_id":"u1","text":"raw-A","anchor":{"source":"A"}},{"unit_id":"u2","text":"raw-B","anchor":{"source":"B"}}]});
    let m=editing::materialize(&source,&json!([{"editor":"a","unit_ids":["u1"]},{"editor":"b","unit_ids":["u2"]}]),&json!({"a":{"ops":[{"op":"COMPRESS","target":"u1","content":"summary-A","lifecycle":"V"}],"pvi":{"V":["free-V"]}},"b":{"ops":[{"op":"COMPRESS","target":"u2","content":"summary-B","lifecycle":"V"}],"pvi":{}}}),true).unwrap();
    assert_eq!(m["volatile"], json!(["summary-A", "free-V", "summary-B"]));
    assert_eq!(m["volatile_index"][0]["source_unit_ids"], json!(["u1"]));
    assert_eq!(m["volatile_index"][1]["source_unit_ids"], json!([]));
    assert_eq!(m["volatile_index"][2]["source_unit_ids"], json!(["u2"]));
    assert_eq!(
        m["volatile_index"][2]["source_anchors"],
        json!([{"source":"B"}])
    );
    assert!(owned_text(&m, "u1").is_empty());
    assert!(owned_text(&m, "u2").is_empty());
}
