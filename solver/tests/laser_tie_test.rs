use serde_json::Value;

fn run(name: &str) -> (Value, Value) {
    let path = format!(
        "{}/tests/fixtures/laser_tie/{name}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let case: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let payload = serde_json::to_vec(&case["payload"]).unwrap();
    let request = serde_json::to_vec(&case["request"]).unwrap();
    let out = gunsmith_mincost::solve_slices(&payload, &request).unwrap();
    (case, serde_json::from_slice(&out).unwrap())
}

fn check(name: &str) {
    let (case, out) = run(name);
    assert_eq!(out["optimal"], true, "{name}: {out}");
    assert_eq!(
        out["best"]["total_price"], case["expectedPrice"],
        "{name}: {out}"
    );
}

#[test]
fn tied_lasers_keep_the_first_part() {
    check("min_spec2");
}

#[test]
fn multi_laser_ties_match_the_site_selection() {
    for name in [
        "laser_mods_c2021",
        "laser_mods_c2162",
        "laser_mods_c2189",
        "laser_mods_c2253",
        "lase_c10191",
    ] {
        check(name);
    }
}
