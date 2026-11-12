//! Named arithmetic control fixture for F59; this is not a pricing model or a
//! research result. It only rechecks the declared T-COST-CONTROL components.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CostControl {
    pub c: [f64; 3],
    pub f: f64,
    pub h: f64,
    pub p_u: f64,
    pub p_h: f64,
    pub p_o: f64,
    pub o_s: f64,
    pub o_b: f64,
    pub s_w: f64,
    pub d: f64,
    pub cb_background: f64,
    pub r_e: f64,
    pub s_w_prefix: f64,
    pub delta_cb_target: f64,
    pub delta_other: f64,
    pub expected_delta_c: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CostControlResult {
    pub cb_background: f64,
    pub r_e: f64,
    pub s_w_prefix: f64,
    pub delta_cb_target: f64,
    pub delta_other: f64,
    pub delta_c: f64,
    pub matches_expected: bool,
}

/// The published T-COST-CONTROL named special case.
pub fn control_fixture() -> CostControl {
    CostControl {
        c: [10.0, 20.0, 30.0],
        f: 30.0,
        h: 10.0,
        p_u: 2.0,
        p_h: 0.1,
        p_o: 3.0,
        o_s: 5.0,
        o_b: 8.0,
        s_w: 40.0,
        d: 7.0,
        cb_background: 61.0,
        r_e: 0.0,
        s_w_prefix: 40.0,
        delta_cb_target: -9.0,
        delta_other: 7.0,
        expected_delta_c: 19.0,
    }
}

impl CostControl {
    pub fn recompute(&self) -> Result<CostControlResult, String> {
        if self
            .c
            .iter()
            .any(|value| !value.is_finite() || *value < 0.0)
            || [
                self.f,
                self.h,
                self.p_u,
                self.p_h,
                self.p_o,
                self.o_s,
                self.o_b,
                self.s_w,
                self.d,
                self.cb_background,
                self.r_e,
                self.s_w_prefix,
            ]
            .iter()
            .any(|value| !value.is_finite() || *value < 0.0)
            || [
                self.delta_cb_target,
                self.delta_other,
                self.expected_delta_c,
            ]
            .iter()
            .any(|value| !value.is_finite())
        {
            return Err("cost control inputs must be finite and nonnegative".into());
        }
        let delta_c = self.cb_background + self.r_e - self.s_w_prefix
            + self.delta_cb_target
            + self.delta_other;
        Ok(CostControlResult {
            cb_background: self.cb_background,
            r_e: self.r_e,
            s_w_prefix: self.s_w_prefix,
            delta_cb_target: self.delta_cb_target,
            delta_other: self.delta_other,
            delta_c,
            matches_expected: (delta_c - self.expected_delta_c).abs() < 1e-9,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BackgroundCase {
    pub id: String,
    pub f: f64,
    pub h: f64,
    pub m: f64,
    pub p_u: f64,
    pub p_h: f64,
    pub write_adjust: f64,
    pub expected: f64,
}

/// C_E,CB-bg = p_h*(F+H) + (p_u-p_h)*M + write_adjust.
pub fn background_cost(
    f: f64,
    h: f64,
    m: f64,
    p_u: f64,
    p_h: f64,
    write_adjust: f64,
) -> Result<f64, String> {
    let inputs = [f, h, m, p_u, p_h, write_adjust];
    if inputs
        .iter()
        .any(|value| !value.is_finite() || *value < 0.0)
        || p_u < p_h
    {
        return Err("background cost inputs must be finite, nonnegative, and p_u >= p_h".into());
    }
    Ok(p_h * (f + h) + (p_u - p_h) * m + write_adjust)
}

pub fn shared_prefix_background_cases() -> Vec<BackgroundCase> {
    vec![
        BackgroundCase {
            id: "C01-A-cold-then-hit".into(),
            f: 30.0,
            h: 10.0,
            m: 30.0,
            p_u: 2.0,
            p_h: 0.1,
            write_adjust: 0.0,
            expected: 61.0,
        },
        BackgroundCase {
            id: "C02-A-all-cold".into(),
            f: 30.0,
            h: 10.0,
            m: 40.0,
            p_u: 2.0,
            p_h: 0.1,
            write_adjust: 0.0,
            expected: 80.0,
        },
        BackgroundCase {
            id: "C03-B-ideal-conditional".into(),
            f: 30.0,
            h: 10.0,
            m: 0.0,
            p_u: 2.0,
            p_h: 0.1,
            write_adjust: 0.0,
            expected: 4.0,
        },
        BackgroundCase {
            id: "C04-B-partial-miss".into(),
            f: 30.0,
            h: 10.0,
            m: 5.0,
            p_u: 2.0,
            p_h: 0.1,
            write_adjust: 0.0,
            expected: 13.5,
        },
        BackgroundCase {
            id: "C05-inclusive-cache-write".into(),
            f: 30.0,
            h: 10.0,
            m: 30.0,
            p_u: 2.0,
            p_h: 0.1,
            write_adjust: 15.0,
            expected: 76.0,
        },
    ]
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DeltaComponents {
    pub c_e_cb_bg: f64,
    pub r_e: f64,
    pub s_w_prefix: f64,
    pub delta_c_wb_once: f64,
    pub delta_c_cb_target: f64,
    pub delta_c_worker_cb_first: f64,
    pub d_front: f64,
    pub delta_c_mix: f64,
    pub delta_c_other: f64,
}

impl DeltaComponents {
    pub fn delta_c(&self) -> Result<f64, String> {
        let values = [
            self.c_e_cb_bg,
            self.r_e,
            self.s_w_prefix,
            self.delta_c_wb_once,
            self.delta_c_cb_target,
            self.delta_c_worker_cb_first,
            self.d_front,
            self.delta_c_mix,
            self.delta_c_other,
        ];
        if values.iter().any(|value| !value.is_finite()) {
            return Err("delta components must be finite".into());
        }
        Ok(self.c_e_cb_bg + self.r_e - self.s_w_prefix
            + self.delta_c_wb_once
            + self.delta_c_cb_target
            + self.delta_c_worker_cb_first
            + self.d_front
            + self.delta_c_mix
            + self.delta_c_other)
    }
}

pub fn general_delta_cases() -> Vec<(&'static str, DeltaComponents, f64)> {
    vec![
        (
            "C06-general-delta",
            DeltaComponents {
                c_e_cb_bg: 61.0,
                r_e: 12.0,
                s_w_prefix: 40.0,
                delta_c_wb_once: 0.0,
                delta_c_cb_target: 0.0,
                delta_c_worker_cb_first: 0.0,
                d_front: 0.0,
                delta_c_mix: 3.0,
                delta_c_other: 7.0,
            },
            43.0,
        ),
        (
            "C07-unmatched-common-items",
            DeltaComponents {
                c_e_cb_bg: 61.0,
                r_e: 12.0,
                s_w_prefix: 40.0,
                delta_c_wb_once: 5.0,
                delta_c_cb_target: -9.0,
                delta_c_worker_cb_first: 2.0,
                d_front: 4.0,
                delta_c_mix: 3.0,
                delta_c_other: 7.0,
            },
            45.0,
        ),
    ]
}

/// C08: a full extra-round prompt cost and paired-mix growth are added once.
pub fn added_round_cost(r_e_extra: f64, paired_mix_growth: f64) -> Result<f64, String> {
    if !r_e_extra.is_finite()
        || !paired_mix_growth.is_finite()
        || r_e_extra < 0.0
        || paired_mix_growth < 0.0
    {
        return Err("added-round inputs must be finite and nonnegative".into());
    }
    Ok(r_e_extra + paired_mix_growth)
}

/// C09: missing usage or price stays unknown; it is never coerced to zero.
pub fn known_background_with_price(
    m: Option<f64>,
    price: Option<f64>,
    f: f64,
    h: f64,
    p_h: f64,
) -> Option<f64> {
    let m = m?;
    let p_u = price?;
    background_cost(f, h, m, p_u, p_h, 0.0).ok()
}

/// C10: overlap counted only within the same route.
pub fn route_overlap(same_route: bool, left: (u64, u64), right: (u64, u64)) -> u64 {
    if !same_route {
        return 0;
    }
    left.1.min(right.1).saturating_sub(left.0.max(right.0))
}
