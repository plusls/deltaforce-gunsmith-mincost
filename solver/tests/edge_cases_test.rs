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

fn run_slices(payload_file: &str, request_file: &str, expected_total_price: i64) {
    let base = format!("{}/tests/fixtures/edge", env!("CARGO_MANIFEST_DIR"));
    let payload = std::fs::read(format!("{base}/{payload_file}")).unwrap();
    let request = std::fs::read(format!("{base}/{request_file}")).unwrap();
    let out: serde_json::Value =
        serde_json::from_slice(&delta_weapon_solver::solve_slices(&payload, &request).unwrap())
            .unwrap();
    assert_eq!(out["optimal"], true, "{payload_file}");
    assert_eq!(out["truncated"], false, "{payload_file}");
    assert_eq!(
        out["best"]["total_price"], expected_total_price,
        "{payload_file}"
    );
}

#[test]
fn narrow_curve_spike_is_not_mistaken_for_monotone() {
    run_slices(
        "narrow_spike.payload.json",
        "narrow_spike.request.json",
        1100,
    );
}

#[test]
fn narrow_curve_notch_is_not_mistaken_for_monotone() {
    run_slices(
        "narrow_notch.payload.json",
        "narrow_notch.request.json",
        1100,
    );
}
