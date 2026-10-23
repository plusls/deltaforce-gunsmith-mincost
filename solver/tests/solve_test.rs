use serde_json::{json, Value};
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
    run_payload(&payload(), request)
}

fn run_payload(payload: &serde_json::Value, request: serde_json::Value) -> serde_json::Value {
    let payload = serde_json::to_vec(payload).unwrap();
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

fn panel_payload() -> serde_json::Value {
    let mut payload = payload();
    payload["parts"]["p2"]["modifiersByMode"] = json!({
        "operations": [
            { "attributeId": "10005", "modifierType": "Addend", "delta": 50 }
        ]
    });
    payload
}

#[test]
fn panel_min_constraint_uses_the_dp_and_finds_the_panel_part() {
    let request = json!({
        "prices": { "w1": 1000, "p1": 100, "p2": 50 },
        "weaponId": "w1",
        "mode": "operations",
        "constraints": [
            { "id": "10005", "min": 90 }
        ]
    });
    let out = run_payload(&panel_payload(), request);
    assert_eq!(out["optimal"], true);
    assert_eq!(out["best"]["selections"]["weapon:w1/slot:1:Muzzle_point"], "p2");
    assert_eq!(out["best"]["panel"]["10005"], 100.0);
    assert_eq!(out["best"]["total_price"], 1050);
}

#[test]
fn panel_min_constraint_above_the_bound_is_infeasible() {
    let request = json!({
        "prices": { "w1": 1000, "p1": 100, "p2": 50 },
        "weaponId": "w1",
        "mode": "operations",
        "constraints": [
            { "id": "10005", "min": 101 }
        ]
    });
    let out = run_payload(&panel_payload(), request);
    assert!(out["best"].is_null());
    assert_eq!(out["truncated"], false);
}

fn detail_payload() -> serde_json::Value {
    let mut payload = payload();
    payload["parts"]["p1"]["mainAttributeModifiersByMode"] = json!({
        "operations": [
            { "attributeId": "10006", "modifierType": "Addend", "delta": 10 }
        ]
    });
    payload["parts"]["p1"]["rawModifiersByMode"] = json!({
        "operations": [
            { "targetPath": "GAiming_ADSTime", "modifierType": "Mult_A", "param1": -0.1 }
        ]
    });
    payload["weaponNumericProfilesById"]["w1"]["linkedTables"]["handlingCurves"] = json!({
        "adsTime": {
            "keys": [
                { "Time": 0, "Value": 1.0 },
                { "Time": 100, "Value": 0.5 }
            ]
        },
        "adsStrafeSpeed": {
            "keys": [
                { "Time": 0, "Value": 0.0 },
                { "Time": 100, "Value": 1.0 }
            ]
        }
    });
    payload["weaponNumericProfilesById"]["w1"]["linkedTables"]["movementByMode"] = json!({
        "operations": { "StandRunSpeed": 300, "StandAimSpeedFactor": -0.5 }
    });
    payload
}

#[test]
fn handling_aim_time_max_prefers_the_faster_part() {
    let request = json!({
        "prices": { "w1": 1000, "p1": 100, "p2": 50 },
        "weaponId": "w1",
        "mode": "operations",
        "constraints": [{ "id": "handling-aim-time", "max": 700 }]
    });
    let out = run_payload(&detail_payload(), request);
    assert_eq!(out["optimal"], true);
    assert_eq!(out["best"]["total_price"], 1100);
    assert_eq!(out["best"]["selections"]["weapon:w1/slot:1:Muzzle_point"], "p1");
    assert_eq!(out["best"]["constraints"][0]["id"], "handling-aim-time");
    assert_eq!(out["best"]["constraints"][0]["value"], 629.0);
}

#[test]
fn handling_aim_time_min_keeps_the_bare_weapon() {
    let request = json!({
        "prices": { "w1": 1000, "p1": 100, "p2": 50 },
        "weaponId": "w1",
        "mode": "operations",
        "constraints": [{ "id": "handling-aim-time", "min": 700 }]
    });
    let out = run_payload(&detail_payload(), request);
    assert_eq!(out["optimal"], true);
    assert_eq!(out["best"]["total_price"], 1000);
    assert_eq!(out["best"]["constraints"][0]["value"], 750.0);
}

#[test]
fn handling_aim_time_without_profile_data_is_infeasible() {
    let request = json!({
        "prices": { "w1": 1000, "p1": 100, "p2": 50 },
        "weaponId": "w1",
        "mode": "operations",
        "constraints": [{ "id": "handling-aim-time", "max": 700 }]
    });
    let out = run_payload(&payload(), request);
    assert!(out["best"].is_null());
    assert_eq!(out["truncated"], false);
}

#[test]
fn unsupported_constraint_is_rejected() {
    let payload = serde_json::to_vec(&payload()).unwrap();
    let request = serde_json::to_vec(&json!({
        "weaponId": "w1",
        "constraints": [{ "id": "not-a-metric", "max": 1.0 }]
    }))
    .unwrap();
    let err = delta_weapon_solver::solve_slices(&payload, &request).unwrap_err();
    assert!(err.contains("unsupported constraint"));
}

fn audit_stats() -> Value {
    json!({
        "byMode": {
            "operations": {
                "base": {"10001": 55, "10005": 48, "10006": 50, "10007": 50, "10008": 50},
                "display": {"10001": 55, "10005": 48, "10006": 50, "10007": 50, "10008": 50}
            }
        }
    })
}

fn audit_profile() -> Value {
    json!({
        "linkedTables": {
            "aimRecoil": {
                "ContinueShootRecoil": {"HorizontalScale": 1.5, "VerticalScale": 1.7},
                "SingleOrBurstShootRecoil": {"HorizontalScale": 1.2, "VerticalScale": 1.3},
                "RecoilShakePitchWithCurve": {"WholeAmplitude": 1.0},
                "RecoilShakeYawWithCurve": {"WholeAmplitude": 1.0}
            },
            "recoilCurves": {
                "recoilControl": {"keys": [{"Time": 0, "Value": 1.0}, {"Time": 100, "Value": 0.5}]},
                "gunkick": {"keys": [{"Time": 0, "Value": 1.0}, {"Time": 100, "Value": 0.5}]}
            },
            "handlingCurves": {
                "adsTime": {"keys": [{"Time": 0, "Value": 2.0}, {"Time": 100, "Value": 1.0}]},
                "sprintToFire": {"keys": [{"Time": 0, "Value": 1.0}, {"Time": 100, "Value": 0.5}]},
                "adsStrafeSpeed": {"keys": [{"Time": 0, "Value": 0.0}, {"Time": 100, "Value": 1.0}]},
                "hipSpread": {"keys": [{"Time": 0, "Value": 1.0}, {"Time": 100, "Value": 0.5}]}
            },
            "stabilityCurves": {
                "breathScale": {"keys": [{"Time": 0, "Value": 1.0}, {"Time": 100, "Value": 0.5}]},
                "breathTime": {"keys": [{"Time": 0, "Value": 1.0}, {"Time": 100, "Value": 2.0}]},
                "fireCameraStability": {"keys": [{"Time": 0, "Value": 1.0}, {"Time": 100, "Value": 0.5}]},
                "movementCameraStability": {"keys": [{"Time": 0, "Value": 1.0}, {"Time": 100, "Value": 0.5}]},
                "holdBreathRecoil": {"keys": [{"Time": 0, "Value": 1.0}, {"Time": 100, "Value": 0.5}]}
            },
            "waistSpread": {
                "AreaSize": [10.0],
                "SpreadMax": {"X": 20.0, "Y": 20.0},
                "LuaDisplaySpreadFactor": 1.0,
                "LuaDisplaySpreadUIFactor": 1.25
            },
            "aiming": {"AimingOnAnimTime": 0.5},
            "movementByMode": {"operations": {"StandRunSpeed": 300.0, "StandAimSpeedFactor": -0.5}},
            "holdBreath": [{
                "StandBreathFactor": 1.0,
                "WeaponBreathConfig": {"AttenSpeed": 1.0},
                "RecoilScale": {"X": 1.0, "Y": 1.0}
            }]
        },
        "detail": {
            "raw_weapon_attribute": {
                "ChangeClipTime": 2.0,
                "ChangeClipTimeWhenEmpty": 2.5,
                "EquipTime": 0.7,
                "UnEquipTime": 0.8,
                "SprintToFireSpeed": 0.3,
                "GunSwayBeHitScale": 1.0
            },
            "raw_supporting_values": {
                "handling": {"AimingOnAnimTime": 0.5},
                "stability": {"GSMovement_CameraScale": 1.0}
            }
        }
    })
}

fn audit_payload(slots: Value, parts: Value, extra: Value) -> Value {
    let mut p = json!({
        "dataVersion": "audit",
        "parts": parts,
        "slotsByHost": {"w1": slots},
        "defaultPresetsByWeapon": {"w1": {"nodes": [{"isRoot": true, "partId": "w1"}]}},
        "virtualRulesByWeapon": {"w1": []},
        "candidateOverridesByWeapon": {},
        "weaponStatsById": {"w1": audit_stats()},
        "weaponNumericProfilesById": {"w1": audit_profile()}
    });
    if let Some(obj) = extra.as_object() {
        for (k, v) in obj {
            p[k] = v.clone();
        }
    }
    p
}

fn audit_part(id: &str, extra: Value) -> Value {
    let mut o = json!({"id": id, "name": id});
    if let Some(e) = extra.as_object() {
        for (k, v) in e {
            o[k] = v.clone();
        }
    }
    o
}

fn audit_request(prices: Value, constraints: Value) -> Value {
    json!({
        "prices": prices,
        "weaponId": "w1",
        "mode": "operations",
        "constraints": constraints,
        "maxResults": 3
    })
}

fn audit_best_price(payload: &Value, request: Value) -> Option<i64> {
    let out = run_payload(payload, request);
    out["best"]["total_price"].as_i64()
}

#[test]
fn audit_dp_base_does_not_include_a_removed_default() {
    let parts = json!({
        "w1": {"id": "w1", "name": "gun"},
        "hA": audit_part("hA", json!({"rawRecoilMultipliersByMode": {"operations": {"horizontal": -0.1}}})),
        "hB": audit_part("hB", json!({"rawRecoilMultipliersByMode": {"operations": {"horizontal": -0.1}}})),
        "cA": audit_part("cA", json!({"rawRecoilMultipliersByMode": {"operations": {"horizontal": -0.5}}})),
        "cB": audit_part("cB", json!({"rawRecoilMultipliersByMode": {"operations": {"horizontal": -0.5}}})),
        "mag": audit_part("mag", json!({}))
    });
    let extra = json!({
        "slotsByHost": {
            "w1": [
                {"id": "1", "name": "Muzzle_point", "candidateIds": ["hA", "hB"]},
                {"id": "2", "name": "Magazine_point", "candidateIds": ["mag"]}
            ],
            "hA": [{"id": "11", "name": "ChildA", "candidateIds": ["cA"]}],
            "hB": [{"id": "12", "name": "ChildB", "candidateIds": ["cB"]}]
        },
        "defaultPresetsByWeapon": {"w1": {"nodes": [
            {"isRoot": true, "partId": "w1"},
            {"partId": "mag", "pathKey": "weapon:w1/slot:2:Magazine_point"}
        ]}}
    });
    let payload = audit_payload(json!([]), parts, extra);
    let prices = json!({"w1": 1000, "hA": 100, "hB": 200, "cA": 1000, "cB": 50, "mag": 1000});
    let request = audit_request(
        prices,
        json!([{"id": "recoil-continuous-horizontal", "max": 0.6}]),
    );
    assert_eq!(audit_best_price(&payload, request), Some(1250));
}

#[test]
fn audit_non_dp_search_can_remove_a_default() {
    let parts = json!({
        "w1": {"id": "w1", "name": "gun"},
        "d1": audit_part("d1", json!({
            "rawRecoilMultipliersByMode": {"operations": {"horizontal": -0.3}},
            "mainAttributeModifiersByMode": {"operations": [{"attributeId": "10005", "modifierType": "Addend", "delta": 20}]}
        })),
        "a1": audit_part("a1", json!({"rawRecoilMultipliersByMode": {"operations": {"horizontal": -0.05}}})),
        "a2": audit_part("a2", json!({"mainAttributeModifiersByMode": {"operations": [{"attributeId": "10005", "modifierType": "Addend", "delta": 5}]}}))
    });
    let extra = json!({
        "defaultPresetsByWeapon": {"w1": {"nodes": [
            {"isRoot": true, "partId": "w1"},
            {"partId": "d1", "pathKey": "weapon:w1/slot:1:Muzzle_point"}
        ]}}
    });
    let payload = audit_payload(
        json!([{"id": "1", "name": "Muzzle_point", "candidateIds": ["d1", "a1", "a2"]}]),
        parts,
        extra,
    );
    let prices = json!({"w1": 1000, "d1": 400, "a1": 100, "a2": 20});
    let request = audit_request(
        prices,
        json!([{"id": "recoil-continuous-horizontal", "min": 0.9}]),
    );
    assert_eq!(audit_best_price(&payload, request), Some(1000));
}

#[test]
fn audit_dp_keeps_a_default_that_satisfies_the_constraint() {
    let parts = json!({
        "w1": {"id": "w1", "name": "gun"},
        "d1": audit_part("d1", json!({
            "rawRecoilMultipliersByMode": {"operations": {"horizontal": -0.3}},
            "mainAttributeModifiersByMode": {"operations": [{"attributeId": "10005", "modifierType": "Addend", "delta": 20}]}
        })),
        "a1": audit_part("a1", json!({"rawRecoilMultipliersByMode": {"operations": {"horizontal": -0.05}}})),
        "a2": audit_part("a2", json!({"mainAttributeModifiersByMode": {"operations": [{"attributeId": "10005", "modifierType": "Addend", "delta": 5}]}}))
    });
    let extra = json!({
        "defaultPresetsByWeapon": {"w1": {"nodes": [
            {"isRoot": true, "partId": "w1"},
            {"partId": "d1", "pathKey": "weapon:w1/slot:1:Muzzle_point"}
        ]}}
    });
    let payload = audit_payload(
        json!([{"id": "1", "name": "Muzzle_point", "candidateIds": ["d1", "a1", "a2"]}]),
        parts,
        extra,
    );
    let prices = json!({"w1": 1000, "d1": 400, "a1": 100, "a2": 20});
    let request = audit_request(
        prices,
        json!([{"id": "recoil-continuous-horizontal", "max": 0.8}]),
    );
    // The default `d1` stays internal, and the ordinary price excludes
    // internal parts, so only the weapon price is paid.
    assert_eq!(audit_best_price(&payload, request), Some(1000));
}

#[test]
fn audit_dp_can_remove_a_structural_default() {
    let parts = json!({
        "w1": {"id": "w1", "name": "gun"},
        "h": audit_part("h", json!({"rawRecoilMultipliersByMode": {"operations": {"horizontal": 0.3}}})),
        "c": audit_part("c", json!({"rawRecoilMultipliersByMode": {"operations": {"horizontal": -0.1}}})),
        "x": audit_part("x", json!({"rawRecoilMultipliersByMode": {"operations": {"horizontal": -0.1}}})),
        "y": audit_part("y", json!({"rawRecoilMultipliersByMode": {"operations": {"horizontal": -0.2}}}))
    });
    let extra = json!({
        "slotsByHost": {
            "w1": [
                {"id": "1", "name": "Muzzle_point", "candidateIds": ["h"]},
                {"id": "2", "name": "Grip_point", "candidateIds": ["x"]},
                {"id": "4", "name": "Stock_point", "candidateIds": ["y"]}
            ],
            "h": [{"id": "3", "name": "Underbarrel_point", "candidateIds": ["c"]}]
        },
        "defaultPresetsByWeapon": {"w1": {"nodes": [
            {"isRoot": true, "partId": "w1"},
            {"partId": "h", "pathKey": "weapon:w1/slot:1:Muzzle_point"}
        ]}}
    });
    let payload = audit_payload(json!([]), parts, extra);
    let prices = json!({"w1": 1000, "h": 400, "c": 100, "x": 50, "y": 100});
    let request = audit_request(
        prices,
        json!([{"id": "recoil-continuous-horizontal", "max": 0.85}]),
    );
    assert_eq!(audit_best_price(&payload, request), Some(1100));
}

#[test]
fn audit_split_panel_constraints_use_the_interval_direction() {
    let parts = json!({
        "w1": {"id": "w1", "name": "gun"},
        "a1": audit_part("a1", json!({"modifiersByMode": {"operations": [{"attributeId": "10005", "modifierType": "Addend", "delta": 5}]}})),
        "a2": audit_part("a2", json!({"modifiersByMode": {"operations": [{"attributeId": "10005", "modifierType": "Addend", "delta": 20}]}}))
    });
    let payload = audit_payload(
        json!([{"id": "1", "name": "Muzzle_point", "candidateIds": ["a1", "a2"]}]),
        parts,
        json!({}),
    );
    let prices = json!({"w1": 1000, "a1": 250, "a2": 100});
    let request = audit_request(
        prices,
        json!([
            {"id": "10005", "min": 50.0},
            {"id": "10005", "max": 60.0}
        ]),
    );
    assert_eq!(audit_best_price(&payload, request), Some(1250));
}

#[test]
fn audit_split_aim_constraints_use_the_equality_direction() {
    let parts = json!({
        "w1": {"id": "w1", "name": "gun"},
        "p1": audit_part("p1", json!({
            "mainAttributeModifiersByMode": {"operations": [{"attributeId": "10006", "modifierType": "Addend", "delta": 40}]},
            "rawModifiersByMode": {"operations": [{"targetPath": "GAiming_ADSTime", "modifierType": "Mult_A", "param1": -0.2}]},
            "rawRecoilMultipliersByMode": {"operations": {"horizontal": -0.2}}
        })),
        "p2": audit_part("p2", json!({
            "mainAttributeModifiersByMode": {"operations": [{"attributeId": "10006", "modifierType": "Addend", "delta": 0}]},
            "rawModifiersByMode": {"operations": [{"targetPath": "GAiming_ADSTime", "modifierType": "Mult_A", "param1": 0.2}]},
            "rawRecoilMultipliersByMode": {"operations": {"horizontal": -0.2}}
        }))
    });
    let payload = audit_payload(
        json!([{"id": "1", "name": "Muzzle_point", "candidateIds": ["p1", "p2"]}]),
        parts,
        json!({}),
    );
    let prices = json!({"w1": 1000, "p1": 50, "p2": 200});
    let request = audit_request(
        prices,
        json!([
            {"id": "handling-aim-time", "max": 1800.0},
            {"id": "handling-aim-time", "min": 1300.0},
            {"id": "recoil-continuous-horizontal", "max": 0.85}
        ]),
    );
    assert_eq!(audit_best_price(&payload, request), Some(1200));
}

#[test]
fn audit_aim_scale_applies_to_recoil_feasibility_and_bounds() {
    let parts = json!({
        "w1": {"id": "w1", "name": "gun"},
        "p1": audit_part("p1", json!({
            "rawRecoilMultipliersByMode": {"operations": {"horizontal": -0.2, "vertical": -0.2}}
        })),
        "p2": audit_part("p2", json!({
            "rawRecoilMultipliersByMode": {"operations": {"horizontal": -0.5, "vertical": -0.5}},
            "aimRecoilProfilesByMode": {"operations": {
                "ContinueShootRecoil": {"HorizontalScale": 1.0, "VerticalScale": 0.4},
                "SingleOrBurstShootRecoil": {"HorizontalScale": 1.0, "VerticalScale": 0.4}
            }}
        }))
    });
    let payload = audit_payload(
        json!([{"id": "1", "name": "Muzzle_point", "candidateIds": ["p1", "p2"]}]),
        parts,
        json!({}),
    );
    let prices = json!({"w1": 1000, "p1": 100, "p2": 50});
    let tight = audit_request(
        prices.clone(),
        json!([{"id": "recoil-continuous-vertical", "max": 0.45}]),
    );
    let loose = audit_request(
        prices,
        json!([{"id": "recoil-continuous-vertical", "max": 0.5}]),
    );
    assert_eq!(audit_best_price(&payload, tight), Some(1050));
    let out = run_payload(&payload, loose);
    assert_eq!(out["optimal"], true);
    assert_eq!(out["best"]["total_price"], 1050);
    assert_eq!(out["best"]["recoil"]["continuous_v"], json!(0.12));
}

fn audit_tune_effect(name: &str, tune_id: &str) -> Value {
    json!({
        "tune_id": tune_id,
        "min_value": -20.0,
        "max_value": 20.0,
        "step_value": 1.0,
        "default_value": 0.0,
        "display_effects": [{
            "attribute_name_snapshot": name,
            "curve_points": [
                {"Time": -20.0, "Value": -20.0},
                {"Time": 0.0, "Value": 0.0},
                {"Time": 20.0, "Value": 20.0}
            ]
        }],
        "runtime_targets": []
    })
}

fn audit_breath_payload(effect_name: &str) -> Value {
    let parts = json!({
        "w1": {"id": "w1", "name": "gun"},
        "a1": audit_part("a1", json!({
            "modifiersByMode": {"operations": [{"attributeId": "10007", "modifierType": "Addend", "delta": 10}]},
            "tuningOptions": [audit_tune_effect(effect_name, "t")]
        })),
        "a2": audit_part("a2", json!({"modifiersByMode": {"operations": [{"attributeId": "10007", "modifierType": "Addend", "delta": 2}]}}))
    });
    audit_payload(
        json!([{"id": "1", "name": "Muzzle_point", "candidateIds": ["a1", "a2"]}]),
        parts,
        json!({}),
    )
}

#[test]
fn audit_min_recoil_constraint_does_not_force_tuning() {
    let parts = json!({
        "w1": {"id": "w1", "name": "gun"},
        "a1": audit_part("a1", json!({
            "rawRecoilMultipliersByMode": {"operations": {"horizontal": -0.2, "vertical": -0.2}},
            "modifiersByMode": {"operations": [{"attributeId": "10005", "modifierType": "Addend", "delta": 20}]},
            "tuningOptions": [audit_tune_effect("%额外后坐力控制", "ta")]
        })),
        "a2": audit_part("a2", json!({"mainAttributeModifiersByMode": {"operations": [{"attributeId": "10005", "modifierType": "Addend", "delta": 5}]}}))
    });
    let payload = audit_payload(
        json!([{"id": "1", "name": "Muzzle_point", "candidateIds": ["a1", "a2"]}]),
        parts,
        json!({}),
    );
    let prices = json!({"w1": 1000, "a1": 100, "a2": 50});
    let request = audit_request(
        prices,
        json!([
            {"id": "recoil-continuous-horizontal", "min": 0.75},
            {"id": "10005", "min": 60.0}
        ]),
    );
    let out = run_payload(&payload, request);
    assert_eq!(out["best"]["total_price"], 1100);
    assert_eq!(out["best"]["selections"]["weapon:w1/slot:1:Muzzle_point"], "a1");
    assert_eq!(out["best"]["recoil"]["continuous_h"], 0.8);
}

#[test]
fn audit_breath_and_camera_tuning_follow_the_constraint_direction() {
    let prices = json!({"w1": 1000, "a1": 100, "a2": 50});
    for (effect, id, bound) in [
        ("%瞄准时呼吸稳定性", "stability-breath-range", json!({"max": 0.35})),
        ("%瞄准时呼吸稳定性", "stability-breath-range", json!({"min": 0.45})),
        ("%瞄准时呼吸稳定性", "stability-breath-duration", json!({"max": 95.0})),
        ("%瞄准时呼吸稳定性", "stability-breath-duration", json!({"min": 105.0})),
        ("%移动时据枪稳定性", "stability-movement-camera", json!({"max": 0.85})),
        ("%移动时据枪稳定性", "stability-movement-camera", json!({"min": 1.1})),
    ] {
        let payload = audit_breath_payload(effect);
        let mut constraint = bound.clone();
        constraint["id"] = json!(id);
        let request = audit_request(prices.clone(), json!([constraint]));
        assert_eq!(
            audit_best_price(&payload, request),
            Some(1100),
            "{effect} {id} {bound}"
        );
    }
}

#[test]
fn audit_virtual_subtree_is_included_in_the_bounds() {
    let parts = json!({
        "w1": {"id": "w1", "name": "gun"},
        "h": audit_part("h", json!({})),
        "v": {
            "id": "v", "name": "v", "isVirtual": true,
            "rawModifiersByMode": {"operations": [{"targetPath": "GRecoil_H", "modifierType": "Mult_C", "param1": 0.5}]}
        },
        "x": audit_part("x", json!({"rawRecoilMultipliersByMode": {"operations": {"horizontal": -0.3}}})),
        "y": audit_part("y", json!({"rawRecoilMultipliersByMode": {"operations": {"horizontal": -0.01}}}))
    });
    let extra = json!({
        "slotsByHost": {
            "w1": [
                {"id": "1", "name": "Muzzle_point", "candidateIds": ["h"]},
                {"id": "3", "name": "Grip_point", "candidateIds": ["x", "y"]}
            ],
            "h": [{
                "id": "2", "name": "Underbarrel_point", "candidateIds": [],
                "virtualCandidateIds": ["v"], "allCandidatesVirtual": true
            }]
        },
        "virtualRulesByWeapon": {"w1": [{"socketId": "2", "virtualPartId": "v", "conditionType": "None"}]}
    });
    let payload = audit_payload(json!([]), parts, extra);
    let prices = json!({"w1": 1000, "h": 100, "x": 50, "y": 10});
    let request = audit_request(
        prices,
        json!([{"id": "recoil-continuous-horizontal", "max": 0.6}]),
    );
    assert_eq!(audit_best_price(&payload, request), Some(1100));
}

#[test]
fn audit_laser_pro_panel_is_excluded_when_the_laser_is_off() {
    let parts = json!({
        "w1": {"id": "w1", "name": "gun"},
        "L": audit_part("L", json!({
            "uiEffectsByMode": {"operations": [{"conditionId": 10038}]},
            "rawRecoilMultipliersByMode": {"operations": {"horizontal": -0.3}},
            "modifiersByMode": {"operations": [
                {"attributeId": "10005", "modifierType": "Addend", "delta": 10, "polarity": "pro"},
                {"attributeId": "10005", "modifierType": "Addend", "delta": -5, "polarity": "con"}
            ]}
        })),
        "a1": audit_part("a1", json!({"rawRecoilMultipliersByMode": {"operations": {"horizontal": -0.05}}})),
        "s": audit_part("s", json!({"rawRecoilMultipliersByMode": {"operations": {"horizontal": -0.01}}}))
    });
    let extra = json!({
        "defaultPresetsByWeapon": {"w1": {"nodes": [
            {"isRoot": true, "partId": "w1"},
            {"partId": "a1", "pathKey": "weapon:w1/slot:1:Muzzle_point"}
        ]}}
    });
    let payload = audit_payload(
        json!([
            {"id": "1", "name": "Muzzle_point", "candidateIds": ["L", "a1"]},
            {"id": "2", "name": "Grip_point", "candidateIds": ["s"]}
        ]),
        parts,
        extra,
    );
    let prices = json!({"w1": 1000, "L": 100, "a1": 50, "s": 20});
    let mut request = audit_request(
        prices,
        json!([
            {"id": "recoil-continuous-horizontal", "max": 0.8},
            {"id": "recoil-continuous-vertical", "min": 0.5},
            {"id": "10005", "max": 45.0}
        ]),
    );
    request["laserEnabled"] = json!(false);
    // Explicitly selecting a non-default laser keeps it visible, so the panel
    // exercises the laser inclusion rule.
    request["lockedPathKeys"] = json!({"weapon:w1/slot:1:Muzzle_point": "L"});
    let out = run_payload(&payload, request);
    assert_eq!(out["best"]["total_price"], 1100);
    assert_eq!(out["best"]["panel"]["10005"], 43.0);
}

#[test]
fn audit_nonmonotonic_curve_uses_the_global_minimum() {
    let mut profile = audit_profile();
    profile["linkedTables"]["recoilCurves"]["recoilControl"] =
        json!({"keys": [{"Time": 0, "Value": 1.0}, {"Time": 10, "Value": 0.2}, {"Time": 20, "Value": 1.0}]});
    let parts = json!({
        "w1": {"id": "w1", "name": "gun"},
        "p10": audit_part("p10", json!({"mainAttributeModifiersByMode": {"operations": [{"attributeId": "10005", "modifierType": "Addend", "delta": 10}]}})),
        "p20": audit_part("p20", json!({"mainAttributeModifiersByMode": {"operations": [{"attributeId": "10005", "modifierType": "Addend", "delta": 20}]}}))
    });
    let payload = audit_payload(
        json!([{"id": "1", "name": "Muzzle_point", "candidateIds": ["p10", "p20"]}]),
        parts,
        json!({"weaponNumericProfilesById": {"w1": profile}}),
    );
    let prices = json!({"w1": 1000, "p10": 100, "p20": 50});
    let request = audit_request(
        prices,
        json!([{"id": "recoil-continuous-horizontal", "max": 0.4}]),
    );
    let out = run_payload(&payload, request);
    assert_eq!(out["best"]["total_price"], 1100);
    assert_eq!(out["best"]["selections"]["weapon:w1/slot:1:Muzzle_point"], "p10");
}

#[test]
fn audit_excluded_default_is_never_installed() {
    let parts = json!({
        "w1": {"id": "w1", "name": "gun"},
        "d1": audit_part("d1", json!({"rawRecoilMultipliersByMode": {"operations": {"horizontal": -0.3}}})),
        "a1": audit_part("a1", json!({"rawRecoilMultipliersByMode": {"operations": {"horizontal": -0.05}}}))
    });
    let extra = json!({
        "defaultPresetsByWeapon": {"w1": {"nodes": [
            {"isRoot": true, "partId": "w1"},
            {"partId": "d1", "pathKey": "weapon:w1/slot:1:Muzzle_point"}
        ]}}
    });
    let payload = audit_payload(
        json!([{"id": "1", "name": "Muzzle_point", "candidateIds": ["d1", "a1"]}]),
        parts,
        extra,
    );
    let prices = json!({"w1": 1000, "d1": 100, "a1": 50});
    let mut request = audit_request(
        prices,
        json!([
            {"id": "recoil-continuous-horizontal", "min": 0.5},
            {"id": "recoil-continuous-horizontal", "max": 0.7}
        ]),
    );
    request["excludedPartIds"] = json!(["d1"]);
    let out = run_payload(&payload, request);
    assert!(out["best"].is_null(), "{}", out);
}

#[test]
fn audit_first_solution_respects_the_cost_cap() {
    let parts = json!({
        "w1": {"id": "w1", "name": "gun"},
        "a1": audit_part("a1", json!({"rawRecoilMultipliersByMode": {"operations": {"horizontal": -0.2}}})),
        "a2": audit_part("a2", json!({"rawRecoilMultipliersByMode": {"operations": {"horizontal": -0.3}}}))
    });
    let payload = audit_payload(
        json!([{"id": "1", "name": "Muzzle_point", "candidateIds": ["a1", "a2"]}]),
        parts,
        json!({}),
    );
    let prices = json!({"w1": 1000, "a1": 100, "a2": 300});
    let mut request = audit_request(
        prices,
        json!([{"id": "recoil-continuous-horizontal", "max": 0.9}]),
    );
    request["firstSolution"] = json!(true);
    request["costCap"] = json!(1050);
    let out = run_payload(&payload, request);
    assert!(out["best"].is_null(), "{}", out);
}

#[test]
fn audit_alternatives_do_not_contain_the_best_solution() {
    let parts = json!({
        "w1": {"id": "w1", "name": "gun"},
        "a": audit_part("a", json!({"rawRecoilMultipliersByMode": {"operations": {"horizontal": -0.6}}})),
        "b": audit_part("b", json!({"rawRecoilMultipliersByMode": {"operations": {"horizontal": -0.2}}})),
        "x": audit_part("x", json!({})),
        "y": audit_part("y", json!({"rawRecoilMultipliersByMode": {"operations": {"horizontal": 0.2}}}))
    });
    let payload = audit_payload(
        json!([
            {"id": "1", "name": "Muzzle_point", "candidateIds": ["a", "b"]},
            {"id": "2", "name": "Grip_point", "candidateIds": ["x", "y"]}
        ]),
        parts,
        json!({}),
    );
    let prices = json!({"w1": 1000, "a": 10, "b": 500, "x": 1, "y": 1000});
    let request = audit_request(
        prices,
        json!([
            {"id": "recoil-continuous-horizontal", "min": 0.5},
            {"id": "recoil-continuous-horizontal", "max": 0.9}
        ]),
    );
    let out = run_payload(&payload, request);
    assert_eq!(out["best"]["total_price"], 1500);
    let alternatives = out["alternatives"].as_array().unwrap();
    assert_eq!(alternatives.len(), 1, "{}", out);
    assert_eq!(alternatives[0]["total_price"], 2010);
    let best_price = out["best"]["total_price"].as_i64().unwrap();
    assert!(alternatives
        .iter()
        .all(|a| a["total_price"].as_i64() != Some(best_price)));
}

fn audit_tradeoff_tune(tune_id: &str, name: &str, from: f64, to: f64) -> Value {
    json!({
        "tune_id": tune_id,
        "min_value": 0.0,
        "max_value": 10.0,
        "step_value": 10.0,
        "default_value": 0.0,
        "display_effects": [{
            "attribute_name_snapshot": name,
            "curve_points": [
                {"Time": 0.0, "Value": from},
                {"Time": 10.0, "Value": to}
            ]
        }],
        "runtime_targets": []
    })
}

fn audit_fixed_tune(tune_id: &str, name: &str, value: f64) -> Value {
    json!({
        "tune_id": tune_id,
        "min_value": 0.0,
        "max_value": 0.0,
        "step_value": 0.0,
        "default_value": 0.0,
        "display_effects": [{
            "attribute_name_snapshot": name,
            "curve_points": [
                {"Time": 0.0, "Value": value},
                {"Time": 10.0, "Value": value}
            ]
        }],
        "runtime_targets": []
    })
}

#[test]
fn audit_coupled_tuning_channels_are_not_compared_independently() {
    // The cheaper part's single slider trades recoil control against aim speed,
    // so its per-channel maxima (20 recoil control, 20 aim speed) are not
    // jointly reachable. The pricier part has two independent fixed tunes.
    // Dominance must not treat the trade-off as if both maxima came at once,
    // or it prunes the only feasible build.
    let parts = json!({
        "w1": {"id": "w1", "name": "gun"},
        "x": audit_part("x", json!({
            "tuningOptions": [{
                "tune_id": "tx",
                "min_value": 0.0,
                "max_value": 10.0,
                "step_value": 10.0,
                "default_value": 0.0,
                "display_effects": [
                    {
                        "attribute_name_snapshot": "%额外后坐力控制",
                        "curve_points": [
                            {"Time": 0.0, "Value": 20.0},
                            {"Time": 10.0, "Value": 0.0}
                        ]
                    },
                    {
                        "attribute_name_snapshot": "%举镜瞄准速度",
                        "curve_points": [
                            {"Time": 0.0, "Value": 0.0},
                            {"Time": 10.0, "Value": 20.0}
                        ]
                    }
                ],
                "runtime_targets": []
            }]
        })),
        "y": audit_part("y", json!({
            "tuningOptions": [
                audit_fixed_tune("tyh", "%额外后坐力控制", 15.0),
                audit_fixed_tune("tyo", "%举镜瞄准速度", 15.0)
            ]
        }))
    });
    let payload = audit_payload(
        json!([{"id": "1", "name": "Muzzle_point", "candidateIds": ["x", "y"]}]),
        parts,
        json!({}),
    );
    let prices = json!({"w1": 1000, "x": 100, "y": 200});
    let request = audit_request(
        prices,
        json!([
            {"id": "recoil-continuous-horizontal", "max": 0.9},
            {"id": "handling-aim-time", "max": 1400}
        ]),
    );
    let out = run_payload(&payload, request);
    assert_eq!(out["optimal"], true, "{}", out);
    assert_eq!(out["best"]["total_price"], 1200, "{}", out);
    assert_eq!(
        out["best"]["selections"]["weapon:w1/slot:1:Muzzle_point"],
        "y"
    );
    assert_eq!(out["best"]["recoil"]["continuous_h"], 0.85);
    assert!(
        (out["best"]["detail"]["aim_time"].as_f64().unwrap() - 1500.0 / 1.15).abs() < 1e-6
    );
}
