use sic_core_rust::cost::{CostControl, control_fixture};

#[test]
fn named_control_fixture_recomputes_delta_c_without_becoming_a_model() {
    let fixture = control_fixture();
    assert_eq!(fixture.c, [10.0, 20.0, 30.0]);
    assert_eq!(fixture.f, 30.0);
    assert_eq!(fixture.h, 10.0);
    assert_eq!(fixture.p_u, 2.0);
    assert_eq!(fixture.p_h, 0.1);
    assert_eq!(fixture.p_o, 3.0);
    let result = fixture.recompute().unwrap();
    assert_eq!(result.cb_background, 61.0);
    assert_eq!(result.r_e, 0.0);
    assert_eq!(result.s_w_prefix, 40.0);
    assert_eq!(result.delta_cb_target, -9.0);
    assert_eq!(result.delta_other, 7.0);
    assert_eq!(result.delta_c, 19.0);
    assert!(result.matches_expected);

    let mut mutated = fixture.clone();
    mutated.delta_other = 8.0;
    let mutated_result = mutated.recompute().unwrap();
    assert_eq!(mutated_result.delta_c, 20.0);
    assert!(!mutated_result.matches_expected);

    let invalid = CostControl {
        delta_other: f64::NAN,
        ..fixture
    };
    assert!(invalid.recompute().is_err());
}
