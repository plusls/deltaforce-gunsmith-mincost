use serde_json::Value;

fn run(name: &str) -> (Value, Value) {
    let path = format!(
        "{}/tests/fixtures/cannot_satisfy/{name}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let case: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let payload = serde_json::to_vec(&case["payload"]).unwrap();
    let request = serde_json::to_vec(&case["request"]).unwrap();
    let out = gunsmith_mincost::solve_slices(&payload, &request).unwrap();
    (case, serde_json::from_slice(&out).unwrap())
}

#[test]
fn removing_a_default_part_can_release_a_hidden_slot() {
    let (case, out) = run("remove_default_structural11051");
    assert_eq!(out["optimal"], true, "{out}");
    assert_eq!(out["best"]["total_price"], case["expectedPrice"], "{out}");
}
