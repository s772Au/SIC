#[path = "../src/decision.rs"]
mod decision;

use decision::{apply, empty, from_editor};
use serde_json::{Value, json};

#[test]
fn empty_workspace_has_four_views_and_discovery_prompt() {
    let value = empty();
    assert_eq!(value["revision"], 0);
    assert_eq!(value["discovery_prompt"], "What have I missed?");
    for name in ["Working on", "Failed Before", "Not Tried Yet", "Overlooked"] {
        assert_eq!(value["views"][name], json!([]));
    }
}

#[test]
fn event_batch_populates_views_with_evidence_and_host_completion() {
    let start = empty();
    let events = vec![
        json!({"id":"work-1","kind":"work","text":"Try the bounded parser","evidence":["task-17"]}),
        json!({"id":"fail-1","kind":"failure","text":"The first parser rejected UTF-8 offsets","evidence":["run-2"]}),
        json!({"id":"candidate-1","kind":"candidate","text":"Try byte-to-codepoint mapping","evidence":["source-4"]}),
        json!({"id":"missed-1","kind":"overlooked","text":"Check combining marks","evidence":["review-note"]}),
        json!({"id":"complete-1","kind":"complete","target":"work-1","text":"Parser passed the fixture","evidence":["fixture-3"]}),
    ];
    let next = apply(&start, 0, &events).unwrap();
    assert_eq!(next["revision"], 1);
    assert_eq!(next["views"]["Working on"][0]["status"], "completed");
    assert_eq!(
        next["views"]["Working on"][0]["completion_evidence"],
        json!(["fixture-3"])
    );
    assert_eq!(
        next["views"]["Failed Before"][0]["evidence"],
        json!(["run-2"])
    );
    assert_eq!(
        next["views"]["Not Tried Yet"][0]["text"],
        "Try byte-to-codepoint mapping"
    );
    assert_eq!(next["views"]["Overlooked"][0]["status"], "recorded");
    assert_eq!(start, empty(), "pure function leaves input unchanged");
}

#[test]
fn event_ids_are_idempotent_and_revision_is_compare_and_swap() {
    let first = apply(
        &empty(),
        0,
        &[json!({"id":"event-1","kind":"work","text":"inspect","evidence":[]})],
    )
    .unwrap();
    let repeated = apply(
        &first,
        1,
        &[json!({"id":"event-1","kind":"work","text":"inspect","evidence":[]})],
    )
    .unwrap();
    assert_eq!(repeated["views"]["Working on"].as_array().unwrap().len(), 1);
    assert_eq!(repeated["event_ids"], json!(["event-1"]));
    assert_eq!(repeated["revision"], 2);
    assert!(
        apply(
            &first,
            0,
            &[json!({"kind":"work","text":"stale","evidence":[]})]
        )
        .is_err()
    );
    assert!(
        apply(
            &empty(),
            0,
            &[
                json!({"id":"same","kind":"work","text":"one","evidence":[]}),
                json!({"id":"same","kind":"work","text":"different","evidence":[]}),
            ],
        )
        .is_err()
    );
}

#[test]
fn invalid_late_event_rejects_entire_batch_without_partial_change() {
    let current = empty();
    let events = vec![
        json!({"id":"valid","kind":"work","text":"would be added","evidence":[]}),
        json!({"id":"invalid","kind":"failure","text":"no evidence supplied"}),
    ];
    assert!(apply(&current, 0, &events).is_err());
    assert_eq!(current, empty());
}

#[test]
fn editor_replacement_checks_revision_shape_and_unknown_fields() {
    let current = empty();
    let raw = json!({"views": {
        "working_on": [{"text":"continue","evidence":["cb-1"]}],
        "Failed Before": [],
        "Not Tried Yet": ["candidate clue"],
        "Overlooked": []
    }});
    let next = from_editor(&current, 0, &raw).unwrap();
    assert_eq!(next["revision"], 1);
    assert_eq!(next["views"]["Working on"][0]["status"], "active");
    assert_eq!(next["views"]["Not Tried Yet"][0]["status"], "recorded");
    assert!(
        from_editor(&next, 0, &raw).is_err(),
        "stale Editor snapshot rejected"
    );

    let unknown = json!({"views":{"Working on":[{"text":"x","evidence":[]}],"mystery":[]}});
    assert!(from_editor(&current, 0, &unknown).is_err());
    let unknown_item = json!({"Working on":[{"text":"x","evidence":[],"grant_authority":true}]});
    assert!(from_editor(&current, 0, &unknown_item).is_err());
    let claimed_complete = json!({"Working on":[{"text":"x","evidence":[],"status":"completed"}]});
    assert!(from_editor(&current, 0, &claimed_complete).is_err());
}

#[test]
fn event_batch_with_missing_completion_target_is_atomic() {
    let start: Value = empty();
    let events = vec![
        json!({"id":"work-1","kind":"work","text":"planned","evidence":[]}),
        json!({"id":"complete-1","kind":"complete","target":"missing","text":"done","evidence":[]}),
    ];
    assert!(apply(&start, 0, &events).is_err());
    assert_eq!(start, empty());
}
