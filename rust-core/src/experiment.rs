//! Deterministic, model-free scoring helpers for synthetic experiments.
//!
//! These helpers do not judge semantic quality. They only verify whether
//! pre-declared fact markers survive in the material, so a synthetic harness
//! can compare workflows without asking a model to grade itself.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RetentionScore {
    pub expected: usize,
    pub retained: usize,
    pub ratio: f64,
}

/// Exact-marker retention over the supplied material. Duplicate facts count
/// once; missing facts lower the ratio. An empty expectation scores zero.
pub fn exact_fact_retention(facts: &[String], material: &str) -> RetentionScore {
    let expected = facts.len();
    let retained = facts
        .iter()
        .filter(|fact| !fact.is_empty() && material.contains(fact.as_str()))
        .count();
    RetentionScore {
        expected,
        retained,
        ratio: if expected == 0 {
            0.0
        } else {
            retained as f64 / expected as f64
        },
    }
}

/// Prefix baseline: concatenate material in order and keep at most
/// `budget_chars` Unicode scalar values. This models a fixed-budget window
/// without SIC editing.
pub fn prefix_baseline(material: &[String], budget_chars: usize) -> String {
    let mut out = String::new();
    let mut used = 0usize;
    for item in material {
        let remaining = budget_chars.saturating_sub(used);
        if remaining == 0 {
            break;
        }
        let take = item.chars().take(remaining).collect::<String>();
        used += take.chars().count();
        out.push_str(&take);
    }
    out
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum EvaluationStatus {
    Evaluated,
    NotEvaluated,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RubricDimension {
    pub name: String,
    pub weight: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Rubric {
    pub version: String,
    pub reference: String,
    pub dimensions: Vec<RubricDimension>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DimensionOutcome {
    pub status: EvaluationStatus,
    pub score: Option<f64>,
    pub reason: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RubricResult {
    pub status: EvaluationStatus,
    pub score: Option<f64>,
    pub dimensions: std::collections::BTreeMap<String, DimensionOutcome>,
    pub reason: Option<String>,
}

/// Score only a frozen rubric over independently produced evidence. Mock,
/// fixture, synthetic, or unknown provenance is never quality evidence; a
/// missing dimension stays unknown instead of becoming zero.
pub fn score_rubric(
    rubric: &Rubric,
    evidence: &std::collections::BTreeMap<String, Option<f64>>,
    provenance: &str,
) -> RubricResult {
    if provenance != "real_model" {
        return RubricResult {
            status: EvaluationStatus::NotEvaluated,
            score: None,
            dimensions: std::collections::BTreeMap::new(),
            reason: Some(format!(
                "provenance {provenance:?} is not independent quality evidence"
            )),
        };
    }
    if rubric.version.trim().is_empty()
        || rubric.reference.trim().is_empty()
        || rubric.dimensions.is_empty()
    {
        return RubricResult {
            status: EvaluationStatus::NotEvaluated,
            score: None,
            dimensions: std::collections::BTreeMap::new(),
            reason: Some("rubric is not frozen".into()),
        };
    }
    let mut dimensions = std::collections::BTreeMap::new();
    let mut weighted = 0.0;
    let mut weights = 0.0;
    let mut complete = true;
    for dimension in &rubric.dimensions {
        let (outcome, value) = match evidence.get(&dimension.name).copied().flatten() {
            Some(score) if (0.0..=1.0).contains(&score) && dimension.weight > 0.0 => (
                DimensionOutcome {
                    status: EvaluationStatus::Evaluated,
                    score: Some(score),
                    reason: None,
                },
                Some(score),
            ),
            Some(_) => {
                complete = false;
                (
                    DimensionOutcome {
                        status: EvaluationStatus::NotEvaluated,
                        score: None,
                        reason: Some("score or weight is out of range".into()),
                    },
                    None,
                )
            }
            None => {
                complete = false;
                (
                    DimensionOutcome {
                        status: EvaluationStatus::NotEvaluated,
                        score: None,
                        reason: Some("missing evidence (unknown, not zero)".into()),
                    },
                    None,
                )
            }
        };
        if let Some(score) = value {
            weighted += score * dimension.weight;
            weights += dimension.weight;
        }
        dimensions.insert(dimension.name.clone(), outcome);
    }
    if !complete || weights == 0.0 {
        return RubricResult {
            status: EvaluationStatus::NotEvaluated,
            score: None,
            dimensions,
            reason: Some("at least one dimension is unknown or invalid".into()),
        };
    }
    RubricResult {
        status: EvaluationStatus::Evaluated,
        score: Some(weighted / weights),
        dimensions,
        reason: None,
    }
}
