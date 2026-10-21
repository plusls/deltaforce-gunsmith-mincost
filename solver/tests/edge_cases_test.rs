use delta_weapon_solver::{solve_request, SolveRequest};

fn run_case(fixture: &str, expected_total_price: i64) {
    let path = format!(
        "{}/tests/fixtures/edge/{fixture}",
        env!("CARGO_MANIFEST_DIR")
    );
    let text = std::fs::read_to_string(&path).unwrap();
    let request: SolveRequest = serde_json::from_str(&text).unwrap();
    let out = solve_request(request).unwrap();
    let best = out
        .best
        .as_ref()
        .unwrap_or_else(|| panic!("{fixture}: no build"));
    assert!(out.optimal, "{fixture}: not optimal");
    assert!(!out.truncated, "{fixture}: truncated");
    assert_eq!(best.total_price, expected_total_price, "{fixture}");
}

#[test]
fn structural_default_keeps_non_structural_alternatives_reachable() {
    run_case("struct_default_alternative.json", 1076);
}

#[test]
fn negative_recoil_bound_does_not_prune_feasible_states() {
    run_case("negative_recoil_bound.json", 1429);
}

#[test]
fn negative_recoil_max_disables_pareto_tuning_pruning() {
    run_case("negative_recoil_max.json", 1352);
}

#[test]
fn laser_adds_every_non_con_addend() {
    run_case("laser_non_pro_addend.json", 1252);
}
