use serde_json::{json, Value};

fn load(name: &str) -> Value {
    let path = format!(
        "{}/tests/fixtures/constraint_tolerance/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

fn run(payload: &Value, request: &Value) -> Value {
    let payload = serde_json::to_vec(payload).unwrap();
    let request = serde_json::to_vec(request).unwrap();
    let out = gunsmith_mincost::solve_slices(&payload, &request).unwrap();
    serde_json::from_slice(&out).unwrap()
}

fn case(constraint: Value) -> (Value, Value) {
    let payload = load("payload.json");
    let mut request = load("request_max_1e-12.json");
    request["constraints"] = json!([constraint]);
    (payload, request)
}

#[test]
fn a_max_violation_within_tolerance_is_feasible() {
    let (payload, request) = case(json!({"id": "recoil-fire-stability", "max": 0.999999999999}));
    let out = run(&payload, &request);
    assert_eq!(out["optimal"], true, "{out}");
    assert_eq!(out["best"]["total_price"], 1000, "{out}");
    assert_eq!(out["best"]["constraints"][0]["ok"], true, "{out}");
    assert_eq!(out["best"]["constraints"][0]["value"], 1.0, "{out}");
}

#[test]
fn a_max_violation_beyond_tolerance_has_no_build() {
    let (payload, request) = case(json!({"id": "recoil-fire-stability", "max": 0.99999999}));
    let out = run(&payload, &request);
    assert_eq!(out["optimal"], false, "{out}");
    assert!(out["best"].is_null(), "{out}");
    assert_eq!(out["message"], "no build satisfies the constraints", "{out}");
}

#[test]
fn a_min_violation_within_tolerance_is_feasible() {
    let (payload, request) = case(json!({"id": "recoil-fire-stability", "min": 1.000000000001}));
    let out = run(&payload, &request);
    assert_eq!(out["optimal"], true, "{out}");
    assert_eq!(out["best"]["total_price"], 1000, "{out}");
    assert_eq!(out["best"]["constraints"][0]["ok"], true, "{out}");
}

#[test]
fn a_min_violation_beyond_tolerance_has_no_build() {
    let (payload, request) = case(json!({"id": "recoil-fire-stability", "min": 1.00000001}));
    let out = run(&payload, &request);
    assert_eq!(out["optimal"], false, "{out}");
    assert!(out["best"].is_null(), "{out}");
    assert_eq!(out["message"], "no build satisfies the constraints", "{out}");
}
