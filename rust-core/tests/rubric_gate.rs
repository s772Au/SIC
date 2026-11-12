use sic_core_rust::experiment::{EvaluationStatus, Rubric, RubricDimension, score_rubric};
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
