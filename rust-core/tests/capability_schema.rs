//! The capability table is only useful if it cannot drift from the dispatch table.
//!
//! Two lists describing the same set of operations will diverge unless something
//! fails when they do. These tests make that divergence a build failure, and also
//! check that a declared required field is really the field a handler names.

use serde_json::Value;
use sic_core_rust::{capability_schema::OPERATION_PAYLOADS, core::Core};

#[test]
fn every_operation_has_exactly_one_payload_entry() {
    let caps: Value = Core::capabilities();
    let ops = caps["operations"].as_array().expect("operations list");
    let mut dispatch: Vec<&str> = ops
        .iter()
        .filter_map(|row| row["operation"].as_str())
        .collect();
    dispatch.sort_unstable();

    let mut table: Vec<&str> = OPERATION_PAYLOADS.iter().map(|row| row.operation).collect();
    table.sort_unstable();

    let missing: Vec<&&str> = dispatch.iter().filter(|op| !table.contains(op)).collect();
    let extra: Vec<&&str> = table.iter().filter(|op| !dispatch.contains(op)).collect();
    assert!(
        missing.is_empty(),
        "operations with no payload entry (a host cannot learn their fields): {missing:?}"
    );
    assert!(
        extra.is_empty(),
        "payload entries with no operation (a stale row the host would trust): {extra:?}"
    );

    let unique: std::collections::BTreeSet<&str> = table.iter().copied().collect();
    assert_eq!(
        unique.len(),
        table.len(),
        "the payload table lists the same operation twice"
    );
}

#[test]
fn every_operation_reports_its_payload_in_the_capability_response() {
    let caps: Value = Core::capabilities();
    for row in caps["operations"].as_array().unwrap() {
        let op = row["operation"].as_str().unwrap();
        let payload = &row["payload"];
        assert!(
            payload.is_object(),
            "{op} reports no payload object, so a host cannot tell required from optional"
        );
        let required = payload["required"].as_array().unwrap();
        let optional = payload["optional"].as_array().unwrap();
        for field in required.iter().chain(optional) {
            assert!(
                field["name"].as_str().is_some_and(|name| !name.is_empty()),
                "{op} declares an unnamed field: {field}"
            );
            assert!(
                field["kind"].as_str().is_some_and(|kind| !kind.is_empty()),
                "{op} declares a field without a kind: {field}"
            );
        }
        let req: Vec<&str> = required
            .iter()
            .filter_map(|f| f["name"].as_str())
            .collect();
        let opt: Vec<&str> = optional
            .iter()
            .filter_map(|f| f["name"].as_str())
            .collect();
        for name in &req {
            assert!(
                !opt.contains(name),
                "{op} lists {name} as both required and optional"
            );
        }
    }
}

#[tokio::test]
async fn declared_required_fields_are_the_ones_an_empty_call_names() {
    use serde_json::json;
    use sic_core_rust::{core::OperationContext, storage::Store};
    let core = Core::new(Store::memory().await.unwrap());
    let owner = OperationContext {
        actor: "owner".into(),
        chain_id: None,
        allow_write: true,
        allow_model: false,
        allow_revoke: false,
    };
    let created = core.operate("chain.create", &owner, &json!({})).await.unwrap();
    let chain = created["result"]["chain_id"].as_str().unwrap().to_owned();
    let ctx = OperationContext {
        actor: "owner".into(),
        chain_id: Some(chain),
        allow_write: true,
        allow_model: false,
        allow_revoke: false,
    };

    let mut checked = 0usize;
    for row in OPERATION_PAYLOADS {
        if row.operation == "chain.create" {
            continue;
        }
        let Err(error) = core.operate(row.operation, &ctx, &json!({})).await else {
            continue;
        };
        // A first-missing-field error is exactly one of:
        //   "<field> must be a nonempty string"
        //   "<field> must be a nonnegative integer"
        //   "<field> must be boolean"
        //   "<field> required"   (one word, then the word required)
        // Anything else is prose about state rather than a payload field:
        // "a published cut is required" names no field, so it is excluded.
        let message = error.message.as_str();
        let named = match message.split_once(" must be ") {
            Some((head, _)) if !head.contains(' ') => head,
            _ => match message.strip_suffix(" required") {
                Some(head) if !head.contains(' ') && !head.is_empty() => head,
                _ => continue,
            },
        };
        if row.required.iter().any(|field| field.name == named) {
            checked += 1;
            continue;
        }
        panic!(
            "{} refuses an empty payload naming `{}`, but the payload table does not list it: {:?}",
            row.operation,
            named,
            row.required.iter().map(|field| field.name).collect::<Vec<_>>()
        );
    }
    assert!(
        checked >= 20,
        "only {checked} operations were cross-checked; the probe is not exercising the table"
    );
}

#[tokio::test]
async fn ingestion_payload_schema_matches_accepted_request_shape() {
    use serde_json::json;
    use sic_core_rust::{core::OperationContext, storage::Store};

    let caps = Core::capabilities();
    for name in ["source.ingest", "hierarchy.context.ingest"] {
        let spec = caps["operations"].as_array().unwrap().iter()
            .find(|item| item["operation"] == name).unwrap();
        let required = spec["payload"]["required"].as_array().unwrap();
        let request = required.iter().find(|field| field["name"] == "request").unwrap();
        assert_eq!(request["kind"], "object", "{name} must describe the actual Chat object");
        let optional = spec["payload"]["optional"].as_array().unwrap();
        for name in ["format", "idempotency_key"] {
            assert!(optional.iter().any(|field| field["name"] == name && field["kind"] == "string"));
        }
    }

    let core = Core::new(Store::memory().await.unwrap());
    let owner = OperationContext { actor: "owner".into(), chain_id: None,
        allow_write: true, allow_model: false, allow_revoke: false };
    let created = core.operate("chain.create", &owner, &json!({})).await.unwrap();
    let ctx = OperationContext { chain_id: Some(created["result"]["chain_id"].as_str().unwrap().into()),
        ..owner };
    core.operate("hierarchy.context.create", &ctx,
                 &json!({"context_id":"editor:e1","editor":"e1"})).await.unwrap();
    for (name, extra) in [
        ("source.ingest", json!({})),
        ("hierarchy.context.ingest", json!({"context_id":"editor:e1"})),
    ] {
        let mut valid = extra.clone();
        valid["request"] = json!({"messages":[{"role":"user","content":"schema test"}]});
        valid["format"] = json!("openai-chat");
        core.operate(name, &ctx, &valid).await.unwrap_or_else(|error| panic!("{name}: {error:?}"));
        let mut invalid = extra;
        invalid["request"] = json!([{"role":"user","content":"not a request object"}]);
        assert!(core.operate(name, &ctx, &invalid).await.is_err(), "{name} accepted an array");
    }
}

#[test]
fn notes_never_name_a_field_the_lists_omitted() {
    // How the table drifted the first time: a `notes` string described a field
    // (`cb_id`/`wb_id`, `policy`, `descriptor`) while the field lists stayed
    // empty. A host reading only the lists then could not build a working call,
    // and the note made it look like the table had covered the case.
    for row in OPERATION_PAYLOADS {
        let listed: Vec<&str> = row
            .required
            .iter()
            .chain(row.optional.iter())
            .map(|field| field.name)
            .collect();
        // Only tokens shaped like a wire field can be checked mechanically. A
        // note is prose, and "payload" in "accepts no payload" is an English
        // word that happens to also be the name of the envelope key; matching it
        // would make the test fail on correct text.
        for token in row.notes.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_')) {
            if !token.contains('_') || token.len() < 3 {
                continue;
            }
            if OPERATION_PAYLOADS
                .iter()
                .flat_map(|other| other.required.iter().chain(other.optional.iter()))
                .any(|field| field.name == token)
                && !listed.contains(&token)
            {
                panic!(
                    "{} mentions `{}` in its notes but does not list it; a host reading the lists cannot send it",
                    row.operation, token
                );
            }
        }
    }
}
