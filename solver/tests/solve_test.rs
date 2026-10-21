use serde_json::json;
use std::fs;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

fn payload() -> serde_json::Value {
    json!({
        "dataVersion": "test",
        "parts": {
            "w1": { "id": "w1", "name": "Test Gun" },
            "p1": {
                "id": "p1",
                "name": "P1",
                "rawRecoilMultipliersByMode": {
                    "operations": { "horizontal": -0.2, "vertical": -0.2 }
                }
            },
            "p2": {
                "id": "p2",
                "name": "P2",
                "mainAttributeModifiersByMode": {
                    "operations": [
                        { "attributeId": "10005", "modifierType": "Addend", "delta": 50 }
                    ]
                },
                "rawRecoilMultipliersByMode": {
                    "operations": { "horizontal": -0.5, "vertical": -0.5 }
                }
            }
        },
        "slotsByHost": {
            "w1": [
                { "id": "1", "name": "Muzzle_point", "candidateIds": ["p1", "p2"] }
            ]
        },
        "defaultPresetsByWeapon": {
            "w1": { "nodes": [{ "isRoot": true, "partId": "w1" }] }
        },
        "virtualRulesByWeapon": { "w1": [] },
        "candidateOverridesByWeapon": {},
        "weaponStatsById": {
            "w1": {
                "byMode": {
                    "operations": {
                        "base": {
                            "10001": 50, "10005": 50, "10006": 50, "10007": 50, "10008": 50
                        },
                        "display": {
                            "10001": 50, "10005": 50, "10006": 50, "10007": 50, "10008": 50
                        }
                    }
                }
            }
        },
        "weaponNumericProfilesById": {
            "w1": {
                "linkedTables": {
                    "aimRecoil": {},
                    "recoilCurves": {
                        "recoilControl": {
                            "keys": [
                                { "Time": 0, "Value": 1.0 },
                                { "Time": 100, "Value": 0.5 }
                            ]
                        },
                        "gunkick": {
                            "keys": [
                                { "Time": 0, "Value": 1.0 },
                                { "Time": 100, "Value": 0.5 }
                            ]
                        }
                    }
                }
            }
        }
    })
}

fn run(request: serde_json::Value) -> serde_json::Value {
    let payload = serde_json::to_vec(&payload()).unwrap();
    let request = serde_json::to_vec(&request).unwrap();
    let out = delta_weapon_solver::solve_slices(&payload, &request).unwrap();
    serde_json::from_slice(&out).unwrap()
}

fn base_request() -> serde_json::Value {
    json!({
        "prices": { "w1": 1000, "p1": 100, "p2": 50 },
        "weaponId": "w1",
        "mode": "operations",
        "constraints": [
            { "id": "recoil-continuous-horizontal", "max": 0.4 }
        ],
        "maxResults": 2
    })
}

#[test]
fn picks_the_cheapest_part_that_meets_the_constraint() {
    let out = run(base_request());
    let best = out["best"].as_object().unwrap();
    assert_eq!(out["optimal"], true);
    assert_eq!(best["total_price"], 1050);
    assert_eq!(best["selections"]["weapon:w1/slot:1:Muzzle_point"], "p2");
    assert_eq!(best["panel"]["10005"], 50.0);
    assert_eq!(
        best["installed_part_keys"][0],
        "weapon:w1/slot:1:Muzzle_point\u{0}p2"
    );
}

#[test]
fn first_solution_returns_a_feasible_build_without_optimality_claim() {
    let mut request = base_request();
    request["firstSolution"] = json!(true);
    let out = run(request);
    assert_eq!(out["optimal"], false);
    assert!(out["best"]["total_price"].as_i64().is_some());
    assert_eq!(out["best"]["recoil"]["continuous_h"], 0.375);
}

#[test]
fn infeasible_constraints_return_no_build() {
    let mut request = base_request();
    request["constraints"] = json!([
        { "id": "recoil-continuous-horizontal", "max": 0.1 }
    ]);
    let out = run(request);
    assert!(out["best"].is_null());
    assert_eq!(out["truncated"], false);
}

static CLI_DIR: AtomicU64 = AtomicU64::new(0);

fn cli_dir() -> std::path::PathBuf {
    let id = CLI_DIR.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("dws-cli-{}-{id}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn cli_inputs(prices: serde_json::Value) -> (std::path::PathBuf, std::path::PathBuf) {
    let dir = cli_dir();
    let payload_path = dir.join("payload.json");
    let prices_path = dir.join("prices.json");
    fs::write(&payload_path, serde_json::to_vec(&payload()).unwrap()).unwrap();
    fs::write(&prices_path, serde_json::to_vec(&json!({ "prices": prices })).unwrap()).unwrap();
    (payload_path, prices_path)
}

fn cli_command(payload_path: &std::path::Path, prices_path: &std::path::Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_solve"));
    command
        .args(["--payload", payload_path.to_str().unwrap()])
        .args(["--prices", prices_path.to_str().unwrap()])
        .args(["--weapon", "w1"])
        .args(["--constraint", "recoil-continuous-horizontal:0.9"]);
    command
}

#[test]
fn cli_strict_prices_exclude_unpriced_parts() {
    let (payload_path, prices_path) = cli_inputs(json!({ "w1": 1000, "p1": 100 }));
    let output = cli_command(&payload_path, &prices_path).output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let out: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(out["best"]["total_price"], 1100);
    assert_eq!(out["best"]["selections"]["weapon:w1/slot:1:Muzzle_point"], "p1");
}

#[test]
fn cli_allow_missing_prices_keeps_unpriced_parts() {
    let (payload_path, prices_path) = cli_inputs(json!({ "w1": 1000, "p1": 100 }));
    let output = cli_command(&payload_path, &prices_path)
        .arg("--allow-missing-prices")
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let out: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(out["best"]["total_price"], 1000);
    assert_eq!(out["best"]["selections"]["weapon:w1/slot:1:Muzzle_point"], "p2");
}

#[test]
fn cli_rejects_a_missing_weapon_price() {
    let (payload_path, prices_path) = cli_inputs(json!({ "p1": 100, "p2": 50 }));
    let output = cli_command(&payload_path, &prices_path).output().unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("has no price"));
}
