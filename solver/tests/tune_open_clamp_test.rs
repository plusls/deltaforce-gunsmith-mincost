use serde_json::Value;

fn check(name: &str) {
    let path = format!(
        "{}/tests/fixtures/tune_open/{name}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let case: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let payload = serde_json::to_vec(&case["payload"]).unwrap();
    let request = serde_json::to_vec(&case["request"]).unwrap();
    let out = gunsmith_mincost::solve_slices(&payload, &request).unwrap();
    let out: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(out["optimal"], true, "{name}: {out}");
    assert_eq!(
        out["best"]["total_price"], case["expectedPrice"],
        "{name}: {out}"
    );
}

#[test]
fn open_range_crossing_the_tuning_clamp_finds_the_optimum() {
    check("open_discontinuity");
}

#[test]
fn sprint_constraint_across_the_open_clamp_finds_the_optimum() {
    check("open_disc_sprint");
}

#[test]
fn dp_pruning_respects_the_open_clamp() {
    check("m2_false_opt");
    check("m3_false_opt_clamp");
}

#[test]
fn offers_below_the_clamp_are_not_discarded() {
    check("m1_all_below");
    check("m1b_only_open");
}
