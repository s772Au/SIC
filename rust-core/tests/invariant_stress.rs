
//! Cross-cutting invariants exercised by a deterministic randomized sequence.
//!
//! The point is not to reach a particular state but to confirm that whatever
//! state the core reaches still satisfies the append-only and authority rules:
//! published CBs stay replayable, the cut sequence only grows by one, read-only
//! steps never mutate, and nothing in the edited history grants a mode.

use serde_json::{Value, json};
use sic_core_rust::{
    core::{Core, OperationContext},
    storage::Store,
};

fn context(actor: &str, chain: Option<&str>) -> OperationContext {
    OperationContext {
        actor: actor.into(),
        chain_id: chain.map(str::to_owned),
        allow_write: true,
        allow_model: false,
        allow_revoke: false,
    }
}

async fn call(core: &Core, ctx: &OperationContext, op: &str, payload: Value) -> Value {
    core.operate(op, ctx, &payload)
        .await
        .unwrap_or_else(|error| panic!("{op} failed: {error:?}"))["result"]
        .clone()
}

/// A small deterministic PRNG. The sequence must be reproducible so a failure
/// can be replayed exactly; a random seed would make the report useless.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        self.0 >> 33
    }
    fn below(&mut self, bound: u64) -> usize {
        (self.next() % bound) as usize
    }
}

#[tokio::test]
async fn a_random_operation_sequence_keeps_the_append_only_invariants() {
    let core = Core::new(Store::memory().await.unwrap());
    let chain = call(&core, &context("owner", None), "chain.create", json!({})).await["chain_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let ctx = context("owner", Some(&chain));
    let mut rng = Lcg(0x5EED_2026_1114);
    let mut published: Vec<String> = Vec::new();
    let mut last_seq = 0u64;

    for step in 0..24 {
        let kind = rng.below(6);
        if kind == 0 {
            // Read-only steps must not advance the cut. They also must not ingest:
            // a pending source left behind would break the next block, because a
            // WB may only contain the earliest consecutive pending sources.
            let before = call(&core, &ctx, "chain.get", json!({})).await;
            call(&core, &ctx, "accounting.get", json!({})).await;
            call(&core, &ctx, "cut.get", json!({})).await;
            let after = call(&core, &ctx, "chain.get", json!({})).await;
            assert_eq!(
                before["cut_seq"], after["cut_seq"],
                "a read-only step advanced the cut at step {step}"
            );
            continue;
        }

        // Every ingested source is consumed in the same step, so the pending
        // queue never accumulates material this test is not about.
        let source = call(
            &core,
            &ctx,
            "source.ingest",
            json!({"request":{"messages":[{"role":"user","content":format!("random material {step}")}]}}),
        )
        .await["source_id"]
            .as_str()
            .unwrap()
            .to_owned();

        let ops = match kind {
            1 => json!([{"op":"REPLACE","target":"u1","content":format!("replaced {step}")}]),
            2 => json!([{"op":"DELETE","target":"u1","reason":"stress"}]),
            3 => json!([{"op":"FLAG","target":"u1","content":"flagged"}]),
            _ => json!([]),
        };
        call(&core, &ctx, "wb.form", json!({"source_ids":[source]})).await;
        let job = call(
            &core,
            &ctx,
            "edit.prepare",
            json!({"source_id":source,"assignments":[{"editor":"e1","unit_ids":["u1"]}]}),
        )
        .await["job_id"]
            .as_str()
            .unwrap()
            .to_owned();
        call(
            &core,
            &ctx,
            "edit.submit",
            json!({"job_id":job,"editor":"e1","ops":ops,"pvi":{}}),
        )
        .await;
        let state = call(&core, &ctx, "chain.get", json!({})).await;
        let receipt = call(
            &core,
            &ctx,
            "cut.publish",
            json!({"job_id":job,"expected_cut":state["cut_seq"],"idempotency_key":format!("stress-{step}")}),
        )
        .await;
        let cb = receipt["cb_id"].as_str().unwrap().to_owned();
        let seq = receipt["sequence"].as_u64().unwrap();
        assert_eq!(seq, last_seq + 1, "sequence must grow by one at step {step}");
        last_seq = seq;
        published.push(cb);

        // Invariant: an earlier CB still replays. Rewriting it would invalidate
        // every stored prefix digest that referenced it.
        for id in &published {
            let replay = call(&core, &ctx, "memory.replay", json!({"cb_id":id,"step":0})).await;
            assert!(
                replay["body"].as_str().is_some_and(|body| !body.is_empty()),
                "CB {id} no longer replays after step {step}"
            );
        }
    }

    let final_state = call(&core, &ctx, "chain.get", json!({})).await;
    assert_eq!(final_state["chain_id"], chain);
    assert_eq!(final_state["cut_seq"].as_u64().unwrap(), last_seq);

    // Authority invariant: the decision workspace is content, not authorization.
    let decision = call(&core, &ctx, "decision.get", json!({})).await;
    let rendered = decision.to_string().to_lowercase();
    assert!(
        !rendered.contains("\"capability\"") && !rendered.contains("\"granted_mode\""),
        "a decision view must not carry a capability claim: {decision:?}"
    );
}
