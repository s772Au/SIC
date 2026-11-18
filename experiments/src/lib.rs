//! Paper-support helpers that are deliberately NOT part of the SIC core crate.
//!
//! The core provides mechanism and measurement. This crate holds the two things
//! that only exist to support an experiment or a manuscript argument:
//!
//! * `cost` -- the conditional cost algebra (`C_CB-background`, `S_worker-prefix`,
//!   `ΔC_total`) as arithmetic over numbers a host supplies. It prices nothing
//!   itself and contains no provider rate table.
//! * `experiment` -- deterministic, model-free scoring helpers: exact-marker
//!   retention, a fixed-budget baseline, and a frozen-rubric gate that refuses to
//!   score anything whose provenance is not an independent real-model result.
//!
//! Keeping them out of the core means a production deployment does not carry
//! research scaffolding, and a change to a formula cannot alter runtime
//! behaviour.

pub mod cost;
pub mod experiment;
