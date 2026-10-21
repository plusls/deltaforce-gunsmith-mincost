use serde_json::json;

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
