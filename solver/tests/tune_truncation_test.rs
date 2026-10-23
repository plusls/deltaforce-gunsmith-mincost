use serde_json::Value;

fn run(payload_file: &str) -> Value {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/tune_truncation");
    let payload: Value = serde_json::from_slice(
        &std::fs::read(format!("{dir}/{payload_file}")).unwrap(),
    )
    .unwrap();
    let request: Value =
        serde_json::from_slice(&std::fs::read(format!("{dir}/request.json")).unwrap()).unwrap();
    let payload = serde_json::to_vec(&payload).unwrap();
    let request = serde_json::to_vec(&request).unwrap();
    let out = gunsmith_mincost::solve_slices(&payload, &request).unwrap();
    serde_json::from_slice(&out).unwrap()
}

#[test]
fn a_sampled_tuning_grid_is_never_reported_as_optimal() {
    let out = run("cache_mask_payload.json");
    assert_eq!(out["optimal"], false, "{out}");
    assert_eq!(out["truncated"], false, "{out}");
    assert_eq!(out["best"]["total_price"], 1500, "{out}");
    assert!(
        out["message"]
            .as_str()
            .unwrap()
            .contains("tuning grid was sampled"),
        "{out}"
    );
}

#[test]
fn a_full_tuning_grid_stays_optimal() {
    let out = run("control_payload_max65535.json");
    assert_eq!(out["optimal"], true, "{out}");
    assert_eq!(out["best"]["total_price"], 1050, "{out}");
}
