use serde_json::Value;

fn case(name: &str) -> Value {
    let path = format!(
        "{}/tests/fixtures/dp_internal_stability/{name}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

fn run(name: &str) -> Value {
    let case = case(name);
    let payload = serde_json::to_vec(&case["payload"]).unwrap();
    let request = serde_json::to_vec(&case["request"]).unwrap();
    let out = gunsmith_mincost::solve_slices(&payload, &request).unwrap();
    serde_json::from_slice(&out).unwrap()
}

fn check(name: &str) {
    let case = case(name);
    let out = run(name);
    assert_eq!(out["optimal"], true, "{name}: {out}");
    assert_eq!(
        out["best"]["total_price"], case["expectedPrice"],
        "{name}: {out}"
    );
}

#[test]
fn removing_the_hidden_default_beats_the_visible_candidate() {
    check("nm");
}

#[test]
fn two_opposite_hidden_defaults_are_not_collapsed_into_one_state() {
    check("min");
}

#[test]
fn hidden_stability_part_keeps_the_exact_remove_option() {
    check("def_structural8096");
}

#[test]
fn hidden_stability_part_survives_a_max_bound() {
    check("def_structural11080");
}
