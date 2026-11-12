use sic_core_rust::cost::{
    added_round_cost, background_cost, general_delta_cases, known_background_with_price,
    route_overlap, shared_prefix_background_cases,
};

fn close(left: f64, right: f64) -> bool {
    (left - right).abs() < 1e-9
}

#[test]
fn named_shared_prefix_algebra_cases_match_the_contract_matrix() {
    for case in shared_prefix_background_cases() {
        let actual = background_cost(
            case.f,
            case.h,
            case.m,
            case.p_u,
            case.p_h,
            case.write_adjust,
        )
        .unwrap();
        assert!(close(actual, case.expected), "{}: {actual}", case.id);
    }
    for (id, components, expected) in general_delta_cases() {
        let actual = components.delta_c().unwrap();
        assert!(close(actual, expected), "{id}: {actual}");
    }
    assert_eq!(added_round_cost(12.0, 3.0).unwrap(), 15.0);
    assert_eq!(
        known_background_with_price(None, Some(2.0), 30.0, 10.0, 0.1),
        None
    );
    assert_eq!(
        known_background_with_price(Some(30.0), None, 30.0, 10.0, 0.1),
        None
    );
    assert!(close(
        known_background_with_price(Some(30.0), Some(2.0), 30.0, 10.0, 0.1).unwrap(),
        61.0
    ));
    assert_eq!(route_overlap(true, (10, 12), (10, 12)), 2);
    assert_eq!(route_overlap(false, (10, 12), (10, 12)), 0);
    assert!(background_cost(30.0, 10.0, 30.0, 0.1, 2.0, 0.0).is_err());
}
