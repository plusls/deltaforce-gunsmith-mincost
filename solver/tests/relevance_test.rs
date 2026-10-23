use serde_json::Value;

fn run(name: &str) -> (Value, Value) {
    let path = format!(
        "{}/tests/fixtures/relevance/{name}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let case: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let payload = serde_json::to_vec(&case["payload"]).unwrap();
    let request = serde_json::to_vec(&case["request"]).unwrap();
    let out = gunsmith_mincost::solve_slices(&payload, &request).unwrap();
    (case, serde_json::from_slice(&out).unwrap())
}

#[test]
fn a_relevant_default_part_keeps_its_slot_relevant() {
    let (case, out) = run("default_suboptimal");
    assert_eq!(out["optimal"], true, "{out}");
    assert_eq!(out["best"]["total_price"], case["expectedPrice"], "{out}");
}

#[test]
fn keeping_a_relevant_default_part_avoids_a_false_no_build() {
    let (case, out) = run("default_no_build");
    assert_eq!(out["optimal"], true, "{out}");
    assert_eq!(out["best"]["total_price"], case["expectedPrice"], "{out}");
}
