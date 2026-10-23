use gunsmith_mincost::SolveRequest;
use serde_json::{json, Value};

#[test]
fn laser_enabled_defaults_to_false() {
    let req: SolveRequest = serde_json::from_value(json!({
        "weaponPayload": {"parts": {"w1": {"id": "w1"}}},
        "weaponId": "w1"
    }))
    .unwrap();
    assert!(!req.laser_enabled);
    let explicit: SolveRequest = serde_json::from_value(json!({
        "weaponPayload": {"parts": {"w1": {"id": "w1"}}},
        "weaponId": "w1",
        "laserEnabled": true
    }))
    .unwrap();
    assert!(explicit.laser_enabled);
}

#[test]
fn an_omitted_laser_flag_matches_the_explicit_false_request() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/laser_tie/lase_c10191.json"
    );
    let case: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let payload = serde_json::to_vec(&case["payload"]).unwrap();
    let mut omitted = case["request"].clone();
    omitted.as_object_mut().unwrap().remove("laserEnabled");
    let mut disabled = case["request"].clone();
    disabled["laserEnabled"] = json!(false);
    let run = |request: &Value| -> Value {
        let request = serde_json::to_vec(request).unwrap();
        let out = gunsmith_mincost::solve_slices(&payload, &request).unwrap();
        serde_json::from_slice(&out).unwrap()
    };
    assert_eq!(run(&omitted), run(&disabled));
}
