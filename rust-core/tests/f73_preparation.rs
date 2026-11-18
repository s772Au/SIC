
//! F73 is a preparation claim, and preparation is a thing that can be checked.
//!
//! The contract's negative cases for F73 are all about pretending work was done:
//! reusing old draft results, claiming research without fetching the source,
//! silently swapping prompt or engine and reporting a single-variable result,
//! and letting mechanical format success stand in for a semantic ideal. This
//! test asserts only the parts that are verifiable from the locked metadata:
//! every source names a commit and a blob SHA, and the excerpt storage stays
//! metadata-sized rather than a copy of upstream text.

use serde_json::Value;

fn sources() -> Value {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("research/f73/sources.json");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|error| panic!("invalid sources.json: {error}"))
}

#[test]
fn every_locked_source_names_a_commit_and_a_blob_sha() {
    let doc = sources();
    assert_eq!(doc["format"], "sic-f73-sources-v1");
    let rows = doc["sources"].as_array().expect("sources must be a list");
    assert!(!rows.is_empty(), "no source is locked, so no research claim is possible");

    for row in rows {
        let id = row["id"].as_str().expect("a source needs a stable id");
        for field in ["repo", "path", "commit", "git_blob_sha", "license", "fetched"] {
            let value = row[field].as_str().unwrap_or("");
            assert!(
                !value.trim().is_empty(),
                "source {id} is missing `{field}`; an unnamed source cannot be re-checked"
            );
        }
        // A commit must look like a full SHA, and the blob SHA like one too, so a
        // branch name or a short prefix cannot pass as a pinned reference.
        for field in ["commit", "git_blob_sha"] {
            let value = row[field].as_str().unwrap();
            assert!(
                value.len() >= 40 && value.chars().all(|c| c.is_ascii_hexdigit()),
                "source {id} field `{field}` is not a full hex SHA: {value}"
            );
        }
    }
}

#[test]
fn locked_metadata_stays_metadata_and_does_not_copy_upstream_text() {
    // Negative case: claiming research while shipping the upstream text instead
    // of a pinned reference. A metadata file that grows to source-file size is
    // the signal that a copy crept in.
    let doc = sources();
    let raw = serde_json::to_string(&doc).unwrap();
    let rows = doc["sources"].as_array().unwrap();
    assert!(
        raw.len() < 24_000,
        "sources.json grew to {} bytes; it is supposed to hold references, not copies",
        raw.len()
    );
    for row in rows {
        // The recorded byte count is upstream size; the stored value must not
        // be the upstream body, so no field may be that long.
        for (key, value) in row.as_object().unwrap() {
            if let Some(text) = value.as_str() {
                assert!(
                    text.len() < 2_000,
                    "source {} field `{key}` holds {} bytes, which looks like content",
                    row["id"],
                    text.len()
                );
            }
        }
    }
}

#[test]
fn a_preparation_package_cannot_claim_a_runtime_result() {
    // Negative case: mechanical format success masquerading as a semantic ideal.
    // The package must say what it is, and it must not carry any field that
    // reads as an executed comparison.
    let doc = sources();
    let rendered = serde_json::to_string(&doc).unwrap().to_lowercase();
    for forbidden in [
        "\"measured\"", "\"score\"", "\"improvement\"", "\"baseline_result\"",
        "\"e1_result\"", "\"e2_result\"",
    ] {
        assert!(
            !rendered.contains(forbidden),
            "sources.json must not carry a result field {forbidden}; it is a preparation record"
        );
    }
    assert!(
        doc["note"].as_str().is_some_and(|note| note.to_lowercase().contains("metadata")),
        "the file must state that it holds metadata only"
    );
}
