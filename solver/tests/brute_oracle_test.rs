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

fn lock_conflict_payload() -> Value {
    json!({
        "dataVersion": "lock-conflict",
        "parts": {
            "w1": {"id": "w1", "name": "gun"},
            "p1": {
                "id": "p1",
                "name": "conflicting-kit",
                "extraOccupySocketIds": ["3"]
            },
            "p2": {"id": "p2", "name": "locked-mag"}
        },
        "slotsByHost": {
            "w1": [
                {"id": "2", "name": "S2", "candidateIds": ["p1"]},
                {"id": "3", "name": "S3", "candidateIds": ["p2"]}
            ]
        },
        "weaponStatsById": {
            "w1": {
                "byMode": {
                    "operations": {
                        "base": {"10001": 55},
                        "display": {"10001": 55}
                    }
                }
            }
        }
    })
}

fn lock_conflict_request() -> Value {
    json!({
        "weaponId": "w1",
        "mode": "operations",
        "laserEnabled": false,
        "bipodDeployed": false,
        "prices": {"w1": 1000, "p1": 0, "p2": 200},
        "constraints": [{"id": "10001", "min": 50, "max": 60}],
        "lockedPathKeys": {"weapon:w1/slot:3:S3": "p2"}
    })
}

#[test]
fn brute_rejects_a_witness_whose_lock_was_displaced() {
    let result = run_bf(
        lock_conflict_payload(),
        lock_conflict_request(),
        64,
        "lock-conflict",
    );
    let brute = &result["brute"];
    // Without the lock check the cheaper p1 branch would report 1000 while the
    // resolver had already dropped the locked p2.
    assert_eq!(brute["minTotalPrice"], json!(1200));
    assert_eq!(brute["assemblies"], json!(2));
    assert_eq!(brute["feasibleCount"], json!(1));
    assert_eq!(brute["lockRejectedAssemblies"], json!(1));
    assert_eq!(brute["witness"]["totalPrice"], json!(1200));
    assert_eq!(
        brute["witness"]["selections"]["weapon:w1/slot:2:S2"],
        Value::Null
    );
    assert_eq!(
        brute["witness"]["selections"]["weapon:w1/slot:3:S3"],
        json!("p2")
    );
    assert_eq!(brute["witnessLock"]["valid"], json!(true));
    assert_eq!(brute["witnessLock"]["checked"][0]["ok"], json!(true));
}

#[test]
fn brute_accepts_an_explicit_empty_lock() {
    let mut request = lock_conflict_request();
    request["lockedPathKeys"] = json!({
        "weapon:w1/slot:2:S2": null,
        "weapon:w1/slot:3:S3": "p2"
    });
    let result = run_bf(lock_conflict_payload(), request, 64, "lock-null");
    let brute = &result["brute"];
    assert_eq!(brute["minTotalPrice"], json!(1200));
    assert_eq!(brute["lockRejectedAssemblies"], json!(0));
    assert_eq!(brute["witnessLock"]["valid"], json!(true));
    assert_eq!(brute["witnessLock"]["checked"].as_array().unwrap().len(), 2);
    assert_eq!(brute["witnessLock"]["checked"][0]["ok"], json!(true));
    assert_eq!(brute["witnessLock"]["checked"][1]["ok"], json!(true));
}

#[test]
fn brute_without_locks_keeps_using_the_conflicting_part() {
    let mut request = lock_conflict_request();
    request.as_object_mut().unwrap().remove("lockedPathKeys");
    let result = run_bf(lock_conflict_payload(), request, 64, "no-lock");
    let brute = &result["brute"];
    assert_eq!(brute["minTotalPrice"], json!(1000));
    assert_eq!(brute["feasibleCount"], json!(3));
    assert_eq!(brute["lockRejectedAssemblies"], json!(0));
    assert_eq!(brute["witnessLock"]["valid"], json!(true));
    assert_eq!(brute["witnessLock"]["checked"].as_array().unwrap().len(), 0);
}

fn laser_payload() -> Value {
    json!({
        "dataVersion": "laser-default",
        "parts": {
            "w1": {"id": "w1", "name": "gun"},
            "L": {
                "id": "L",
                "name": "laser",
                "uiEffectsByMode": {"operations": [{"conditionId": 10038}]},
                "modifiersByMode": {"operations": [
                    {"attributeId": "10005", "modifierType": "Addend", "delta": 10, "polarity": "pro"}
                ]}
            }
        },
        "slotsByHost": {
            "w1": [{"id": "1", "name": "Muz_point", "candidateIds": ["L"]}]
        },
        "weaponStatsById": {
            "w1": {
                "byMode": {
                    "operations": {
                        "base": {"10001": 50, "10005": 50},
                        "display": {"10001": 50, "10005": 50}
                    }
                }
            }
        }
    })
}

#[test]
fn brute_treats_missing_laser_enabled_as_false() {
    let mut request = json!({
        "weaponId": "w1",
        "mode": "operations",
        "prices": {"w1": 1000, "L": 100},
        "lockedPathKeys": {"weapon:w1/slot:1:Muz_point": "L"}
    });
    let off = run_bf(laser_payload(), request.clone(), 64, "laser-default");
    assert_eq!(off["brute"]["witness"]["panel"]["10005"], json!(50.0));

    request["laserEnabled"] = json!(true);
    let on = run_bf(laser_payload(), request, 64, "laser-enabled");
    assert_eq!(on["brute"]["witness"]["panel"]["10005"], json!(60.0));
}
