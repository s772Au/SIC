use sic_experiments::experiment::{
    EvaluationStatus, Rubric, RubricDimension, exact_fact_retention, score_rubric,
};
use std::collections::BTreeMap;

fn rubric() -> Rubric {
    Rubric {
        version: "r1".into(),
        reference: "frozen task reference".into(),
        dimensions: vec![
            RubricDimension {
                name: "fact_recall".into(),
                weight: 1.0,
            },
            RubricDimension {
                name: "trace".into(),
                weight: 1.0,
            },
        ],
    }
}

#[test]
fn mock_and_fixture_outputs_are_not_evaluated_and_missing_is_not_zero() {
    let mut complete = BTreeMap::new();
    complete.insert("fact_recall".to_string(), Some(1.0));
    complete.insert("trace".to_string(), Some(0.5));

    let mock = score_rubric(&rubric(), &complete, "mock");
    assert_eq!(mock.status, EvaluationStatus::NotEvaluated);
    assert_eq!(mock.score, None);
    assert!(mock.dimensions.is_empty());

    let mut missing = BTreeMap::new();
    missing.insert("fact_recall".to_string(), Some(1.0));
    let partial = score_rubric(&rubric(), &missing, "real_model");
    assert_eq!(partial.status, EvaluationStatus::NotEvaluated);
    assert_eq!(partial.score, None);
    assert_eq!(partial.dimensions["fact_recall"].score, Some(1.0));
    assert_eq!(partial.dimensions["trace"].score, None);
    assert!(
        partial.dimensions["trace"]
            .reason
            .as_deref()
            .unwrap()
            .contains("unknown")
    );

    let evaluated = score_rubric(&rubric(), &complete, "real_model");
    assert_eq!(evaluated.status, EvaluationStatus::Evaluated);
    assert!((evaluated.score.unwrap() - 0.75).abs() < 1e-9);

    let mut invalid = complete.clone();
    invalid.insert("trace".to_string(), Some(2.0));
    assert_eq!(
        score_rubric(&rubric(), &invalid, "real_model").status,
        EvaluationStatus::NotEvaluated
    );

    let empty = Rubric {
        version: String::new(),
        reference: String::new(),
        dimensions: Vec::new(),
    };
    assert_eq!(
        score_rubric(&empty, &complete, "real_model").status,
        EvaluationStatus::NotEvaluated
    );
}

// ---- F61 negative cases -------------------------------------------------
// An official evaluator that ran on a fixture, a self-authored rubric standing
// in for the official one, and two arms silently compared across different
// tasks. These check the gate, not any model.

fn two_dimension_rubric() -> Rubric {
    Rubric {
        version: "r1".into(),
        reference: "frozen task reference".into(),
        dimensions: vec![
            RubricDimension { name: "fact_recall".into(), weight: 1.0 },
            RubricDimension { name: "trace".into(), weight: 1.0 },
        ],
    }
}

#[test]
fn a_self_authored_rubric_must_not_be_reported_as_the_official_one() {
    // The gate cannot know which rubric is official; what it must not do is
    // label an author-written rubric as an official score. Rubric identity is
    // carried in `version` and `reference`, so a blank or ad-hoc rubric must
    // fail closed rather than produce a comparable number.
    let complete: BTreeMap<String, Option<f64>> = [
        ("fact_recall".to_string(), Some(1.0)),
        ("trace".to_string(), Some(1.0)),
    ]
    .into_iter()
    .collect();

    let anonymous = Rubric {
        version: "".into(),
        reference: "".into(),
        dimensions: two_dimension_rubric().dimensions,
    };
    let result = score_rubric(&anonymous, &complete, "real_model");
    assert_eq!(
        result.status,
        EvaluationStatus::NotEvaluated,
        "an anonymous rubric must not produce a score: {result:?}"
    );
    assert!(
        result.score.is_none(),
        "no number may be reported without a rubric identity"
    );
}

#[test]
fn two_arms_from_different_sources_cannot_be_presented_as_one_comparison() {
    // A comparison is only meaningful if both arms ran the same frozen task
    // text. This asserts the measurement primitive makes that visible instead
    // of hiding it: retention is computed against the exact facts supplied,
    // so an arm that saw different facts cannot report a comparable ratio.
    let task_facts: Vec<String> = vec!["ALPHA fact".into(), "BETA fact".into()];
    let same_arm = exact_fact_retention(&task_facts, "ALPHA fact\nBETA fact");
    assert_eq!(same_arm.retained, 2);
    assert_eq!(same_arm.ratio, 1.0);

    // A different task's material retains none of *this* task's facts, even
    // when it is full of facts of its own. Reporting 0 here is correct and is
    // exactly what stops a cross-task comparison from looking like a win.
    let other_arm = exact_fact_retention(&task_facts, "ALPHA other\nGAMMA other");
    assert_eq!(other_arm.retained, 0);
    assert_eq!(other_arm.ratio, 0.0);
    assert!(
        other_arm.ratio != same_arm.ratio,
        "two arms must not be reported as the same measurement"
    );
}

#[test]
fn an_out_of_range_dimension_invalidates_the_whole_score() {
    // A partially valid rubric is not a partially valid score. A single
    // out-of-range dimension must fail the evaluation closed rather than
    // average into a number that looks defensible.
    let mut mixed: BTreeMap<String, Option<f64>> = [
        ("fact_recall".to_string(), Some(1.0)),
        ("trace".to_string(), Some(3.5)),
    ]
    .into_iter()
    .collect();
    let result = score_rubric(&two_dimension_rubric(), &mixed, "real_model");
    assert_eq!(
        result.status,
        EvaluationStatus::NotEvaluated,
        "one bad dimension must fail the whole score: {result:?}"
    );
    assert!(result.score.is_none());

    mixed.insert("trace".to_string(), Some(1.0));
    let fixed = score_rubric(&two_dimension_rubric(), &mixed, "real_model");
    assert_eq!(fixed.status, EvaluationStatus::Evaluated);
    assert_eq!(fixed.score, Some(1.0));
}