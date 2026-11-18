//! Validate offline prompt-evaluation decisions with the actual SIC edit engine.
//! Usage: cargo run -p sic-core-rust --example validate_editor_decisions -- CASES RESULTS
//! Input files are read-only; this performs no tools, model calls or publication.
use serde_json::{Value, json};
use std::{env, fs};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();
    if args.len() != 3 {
        return Err("expected CASES.json RESULTS.json".into());
    }
    let cases: Value = serde_json::from_str(&fs::read_to_string(&args[1])?)?;
    let decisions: Value = serde_json::from_str(&fs::read_to_string(&args[2])?)?;
    let mut checks = Vec::new();
    for decision in decisions.as_array().ok_or("results must be an array")? {
        let case = cases
            .as_array()
            .ok_or("cases must be an array")?
            .iter()
            .find(|case| case["id"] == decision["case_id"])
            .ok_or("unknown case id")?;
        for call in decision["next_calls"]
            .as_array()
            .ok_or("next_calls must be an array")?
        {
            if call["name"] != "sic_edit_blocks" {
                continue;
            }
            let units: Vec<Value> = case["material"].as_object().ok_or("edit case must have material")?
                .iter().map(|(id,text)| json!({"unit_id":id,"text":text,"anchor":{"virtual_case":case["id"],"unit_id":id}})).collect();
            let source = json!({"units":units});
            let assignment = json!([{"editor":"offline-editor","unit_ids":case["activation"]["assigned_unit_ids"]}]);
            let result = sic_core_rust::editing::materialize(
                &source,
                &assignment,
                &json!({"offline-editor":call["arguments"]}),
                false,
            );
            checks.push(match result {
                Ok(material) => json!({"case_id":case["id"],"accepted":true,"material":material}),
                Err(error) => json!({"case_id":case["id"],"accepted":false,"error":error}),
            });
        }
    }
    println!("{}", serde_json::to_string_pretty(&checks)?);
    Ok(())
}
