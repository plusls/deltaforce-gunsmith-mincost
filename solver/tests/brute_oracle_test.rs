use serde_json::{json, Value};
use std::process::Command;

fn fixture_payload() -> Value {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/tuning_round2_boundary.json"
    );
    serde_json::from_str(&std::fs::read_to_string(path).expect("fixture payload"))
        .expect("decode fixture payload")
}

fn request(threshold: f64) -> Value {
    json!({
        "weaponId": "w1",
        "mode": "operations",
        "laserEnabled": true,
        "bipodDeployed": false,
        "prices": { "w1": 1000, "pz": 50 },
        "constraints": [
            { "id": "recoil-continuous-vertical", "min": threshold, "max": threshold }
        ]
    })
}

fn run_bf(payload: Value, request: Value, max_values: u64, tag: &str) -> Value {
    let spec = json!({
        "payload": payload,
        "request": request,
        "maxAssemblies": 1000,
        "maxEvaluations": 100_000,
        "maxValuesPerTune": max_values,
    });
    let mut path = std::env::temp_dir();
    path.push(format!("dws-bf-{}-{tag}.json", std::process::id()));
    std::fs::write(&path, serde_json::to_vec(&spec).unwrap()).expect("write spec");
    let out = Command::new(env!("CARGO_BIN_EXE_bf"))
        .arg(&path)
        .output()
        .expect("run bf");
    let _ = std::fs::remove_file(&path);
    assert!(
        out.status.success(),
        "bf failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("decode bf output")
}

fn assert_matches(result: &Value, price: i64) {
    let brute = &result["brute"];
    assert_eq!(brute["minTotalPrice"], json!(price));
    assert_eq!(brute["assembliesTruncated"], json!(false));
    assert_eq!(brute["evalsTruncated"], json!(false));
    assert_eq!(brute["skippedTuningGroups"], json!(0));

    let solve = &result["solve"];
    assert_eq!(solve["ok"], json!(true));
    assert_eq!(solve["optimal"], json!(true));
    assert_eq!(solve["totalPrice"], json!(price));

    let recheck = &result["recheckSolution"];
    assert_eq!(recheck["feasible"], json!(true));
    assert_eq!(recheck["totalPrice"], json!(price));
}

#[test]
fn brute_oracle_agrees_on_a_rounding_band_edge() {
    let result = run_bf(fixture_payload(), request(0.87), 64, "edge");
    assert_matches(&result, 1050);
}

#[test]
fn brute_oracle_agrees_on_a_wide_tuning_grid() {
    let mut payload = fixture_payload();
    let tune = &mut payload["parts"]["pz"]["tuningOptions"][0];
    tune["max_value"] = json!(200.0);
    tune["step_value"] = json!(1.0);
    let curve = &mut tune["display_effects"][0]["curve_points"];
    curve[1]["Time"] = json!(200.0);
    curve[1]["Value"] = json!(20.0);

    let result = run_bf(payload, request(0.9), 1024, "wide");
    assert_matches(&result, 1050);
}
