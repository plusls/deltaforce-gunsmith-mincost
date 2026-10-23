use delta_weapon_solver::model::WeaponPayload;
use delta_weapon_solver::search::{Constraint, SolveOptions, SolveResult, Solver};
use rustc_hash::FxHashMap;

const WEAPON: &str = "w1";
const TUNE_ID: &str = "7001";

fn payload() -> WeaponPayload {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/tuning_round2_boundary.json"
    );
    serde_json::from_str(&std::fs::read_to_string(path).expect("fixture payload"))
        .expect("decode fixture payload")
}

fn prices() -> FxHashMap<String, i64> {
    FxHashMap::from_iter([(WEAPON.to_string(), 1000), ("pz".to_string(), 50)])
}

fn solve(payload: &WeaponPayload, constraints: Vec<Constraint>) -> SolveResult {
    let options = SolveOptions {
        mode: "operations".to_string(),
        laser_enabled: true,
        constraints,
        max_nodes: 100_000,
        max_leaves: 100_000,
        max_results: 3,
        ..SolveOptions::default()
    };
    Solver::new(payload, WEAPON, &prices(), &options).solve()
}

#[test]
fn tuning_candidate_on_a_rounded_band_edge_is_retried() {
    let payload = payload();
    // 0.87 is reachable only through the open band 12.5 < t < 13.5 after
    // two-decimal rounding. The 0.5 grid also contains 12.5 and 13.5, which
    // sit on the edges and fail exact evaluation; 13 is the only grid value
    // inside the band.
    let out = solve(
        &payload,
        vec![Constraint::new(
            "recoil-continuous-vertical",
            Some(0.87),
            Some(0.87),
        )],
    );

    assert!(out.optimal);
    let best = out.best.expect("feasible round2 boundary build");
    assert_eq!(best.total_price, 1050);
    assert!((best.recoil.continuous_v - 0.87).abs() <= 1e-9);
    assert_eq!(best.tuning.len(), 1);
    let value = best
        .tuning
        .iter()
        .find(|(key, _)| key.ends_with(TUNE_ID))
        .map(|(_, value)| *value)
        .expect("tuning assignment");
    assert_eq!(value, 13.0);
}

#[test]
fn tuning_grid_above_128_values_keeps_interior_values() {
    let mut payload = payload();
    let tune = payload
        .parts
        .get_mut("pz")
        .expect("tuned part")
        .tuning_options
        .get_mut(0)
        .expect("tuning option");
    tune.max_value = 200.0;
    tune.step_value = 1.0;
    let curve = &mut tune.display_effects[0].curve_points;
    curve.last_mut().expect("curve endpoint").time = 200.0;
    curve.last_mut().expect("curve endpoint").value = 20.0;

    // The plateau for 0.9 is 9.5 < t < 10.5, so the only grid values in the
    // band are 96..104. Before the fix the whole 201-value grid was dropped
    // and the solver reported no build.
    let out = solve(
        &payload,
        vec![Constraint::new(
            "recoil-continuous-vertical",
            Some(0.9),
            Some(0.9),
        )],
    );

    assert!(out.optimal);
    let best = out.best.expect("feasible wide-grid build");
    assert_eq!(best.total_price, 1050);
    assert!((best.recoil.continuous_v - 0.9).abs() <= 1e-9);
    let value = best
        .tuning
        .iter()
        .find(|(key, _)| key.ends_with(TUNE_ID))
        .map(|(_, value)| *value)
        .expect("tuning assignment");
    assert_eq!(value, 96.0);
}
