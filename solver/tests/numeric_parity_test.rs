//! Regression tests for parity bugs against the site implementation in
//! `/tmp/moli/opt.pretty.js`.
//!
//! N1: `kt` / `Tn` append the weapon's baseline recovery modifiers to the
//! part's normal per-mode lists. Both lists must be consumed for panel,
//! display and main-attribute metrics.
//!
//! N2: the reported movement camera metric is
//! `movementCameraStabilityCurve(dS) / max(0.1, 1 + dS_move / 100)`.
//! `GSMovement_CameraScale` only scales the raw value, never the reported one.

use gunsmith_mincost::metrics::{stability_metric_at, DetailSums, Evaluator};
use gunsmith_mincost::model::WeaponPayload;
use rustc_hash::FxHashMap;
use serde_json::{json, Value};

fn run_payload(payload: &Value, request: Value) -> Value {
    let payload = serde_json::to_vec(payload).unwrap();
    let request = serde_json::to_vec(&request).unwrap();
    let out = gunsmith_mincost::solve_slices(&payload, &request).unwrap();
    serde_json::from_slice(&out).unwrap()
}

fn evaluator<'a>(payload: &'a WeaponPayload, prices: &'a FxHashMap<String, i64>) -> Evaluator<'a> {
    Evaluator {
        payload,
        weapon_id: "w1",
        mode: "operations",
        laser_enabled: true,
        bipod_deployed: false,
        prices,
    }
}

/// Weapon stats shared by the fixtures: every panel attribute starts at 50.
fn base_stats() -> Value {
    json!({
        "byMode": {
            "operations": {
                "base": {"10001": 50, "10005": 50, "10006": 50, "10007": 50, "10008": 50},
                "display": {"10001": 50, "10005": 50, "10006": 50, "10007": 50, "10008": 50}
            }
        }
    })
}

fn baseline_payload() -> Value {
    json!({
        "dataVersion": "test",
        "parts": {
            "w1": { "id": "w1", "name": "Test Gun" },
            "p1": {
                "id": "p1",
                "name": "P1",
                "modifiersByMode": {
                    "operations": [
                        { "attributeId": "10005", "modifierType": "Addend", "delta": 5 }
                    ]
                },
                "mainAttributeModifiersByMode": {
                    "operations": [
                        { "attributeId": "10006", "modifierType": "Addend", "delta": 0 }
                    ]
                },
                "baselineRecoveryModifiersByWeaponMode": {
                    "w1": {
                        "operations": [
                            { "attributeId": "10005", "modifierType": "Addend", "delta": 8 },
                            { "attributeId": "10006", "modifierType": "Addend", "delta": 12 }
                        ]
                    }
                },
                "mainAttributeBaselineRecoveryModifiersByWeaponMode": {
                    "w1": {
                        "operations": [
                            { "attributeId": "10006", "modifierType": "Addend", "delta": 10 }
                        ]
                    }
                }
            }
        },
        "slotsByHost": {
            "w1": [
                { "id": "1", "name": "Muzzle_point", "candidateIds": ["p1"] }
            ]
        },
        "defaultPresetsByWeapon": {
            "w1": { "nodes": [{ "isRoot": true, "partId": "w1" }] }
        },
        "virtualRulesByWeapon": { "w1": [] },
        "candidateOverridesByWeapon": {},
        "weaponStatsById": { "w1": base_stats() },
        "weaponNumericProfilesById": {
            "w1": {
                "linkedTables": {
                    "aimRecoil": {},
                    "handlingCurves": {
                        "adsTime": {
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

fn movement_payload() -> Value {
    json!({
        "dataVersion": "test",
        "parts": {
            "w1": { "id": "w1", "name": "Test Gun" }
        },
        "slotsByHost": { "w1": [] },
        "defaultPresetsByWeapon": {
            "w1": { "nodes": [{ "isRoot": true, "partId": "w1" }] }
        },
        "virtualRulesByWeapon": { "w1": [] },
        "candidateOverridesByWeapon": {},
        "weaponStatsById": { "w1": base_stats() },
        "weaponNumericProfilesById": {
            "w1": {
                "linkedTables": {
                    "aimRecoil": {
                        "RecoilShakePitchWithCurve": { "WholeAmplitude": 1.0 },
                        "RecoilShakeYawWithCurve": { "WholeAmplitude": 1.0 }
                    },
                    "stabilityCurves": {
                        "breathScale": {
                            "keys": [
                                { "Time": -20, "Value": 1.0 },
                                { "Time": 20, "Value": 1.0 }
                            ]
                        },
                        "breathTime": {
                            "keys": [
                                { "Time": -20, "Value": 1.0 },
                                { "Time": 20, "Value": 1.0 }
                            ]
                        },
                        "fireCameraStability": {
                            "keys": [
                                { "Time": -20, "Value": 1.0 },
                                { "Time": 20, "Value": 1.0 }
                            ]
                        },
                        "movementCameraStability": {
                            "keys": [
                                { "Time": -20, "Value": 2.0 },
                                { "Time": 0, "Value": 1.0 },
                                { "Time": 20, "Value": 0.3 }
                            ]
                        },
                        "holdBreathRecoil": {
                            "keys": [
                                { "Time": -20, "Value": 1.0 },
                                { "Time": 20, "Value": 1.0 }
                            ]
                        }
                    },
                    "holdBreath": [
                        {
                            "StandBreathFactor": 1.0,
                            "WeaponBreathConfig": { "AttenSpeed": 1.0 },
                            "RecoilScale": { "X": 1.0, "Y": 1.0 }
                        }
                    ]
                },
                "detail": {
                    "raw_supporting_values": {
                        "stability": { "GSMovement_CameraScale": 2.0 }
                    }
                }
            }
        }
    })
}

#[test]
fn n1_display_mods_for_appends_baseline_after_normal_and_scopes_weapon() {
    let payload: WeaponPayload = serde_json::from_value(json!({
        "parts": {
            "p1": {
                "id": "p1",
                "modifiersByMode": {
                    "operations": [
                        {
                            "attributeId": "10001",
                            "modifierType": "Initial",
                            "param1": 5,
                            "name": "normal"
                        }
                    ]
                },
                "baselineRecoveryModifiersByWeaponMode": {
                    "w1": {
                        "operations": [
                            {
                                "attributeId": "10001",
                                "modifierType": "Initial",
                                "param1": 9,
                                "name": "baseline"
                            }
                        ]
                    },
                    "w2": {
                        "operations": [
                            {
                                "attributeId": "10001",
                                "modifierType": "Initial",
                                "param1": 7,
                                "name": "other-weapon"
                            }
                        ]
                    }
                }
            }
        }
    }))
    .unwrap();
    let part = payload.parts.get("p1").unwrap();
    let names: Vec<&str> = part
        .display_mods_for("w1", "operations")
        .map(|m| m.name.as_str())
        .collect();
    assert_eq!(names, vec!["normal", "baseline"]);
    let other: Vec<&str> = part
        .display_mods_for("w2", "operations")
        .map(|m| m.name.as_str())
        .collect();
    assert_eq!(other, vec!["normal", "other-weapon"]);
}

#[test]
fn n1_panel_merges_baseline_recovery_modifiers() {
    let payload_json = baseline_payload();
    let payload: WeaponPayload = serde_json::from_value(payload_json).unwrap();
    let prices: FxHashMap<String, i64> = FxHashMap::default();
    let ev = evaluator(&payload, &prices);
    let part = payload.parts.get("p1").unwrap();
    let panel = ev.panel(&[part]);
    // 50 base + 5 own + 8 baseline; 50 base + 12 baseline.
    assert_eq!(panel.get("10005"), Some(&63.0));
    assert_eq!(panel.get("10006"), Some(&62.0));
}

#[test]
fn n1_constrained_solve_uses_baseline_main_modifiers() {
    let out = run_payload(
        &baseline_payload(),
        json!({
            "prices": { "w1": 1000, "p1": 100 },
            "weaponId": "w1",
            "mode": "operations",
            "constraints": [
                { "id": "handling-aim-time", "min": 0, "max": 720 }
            ],
            "maxResults": 1
        }),
    );
    let best = out["best"]
        .as_object()
        .expect("handling-aim-time must be reachable through the baseline +10");
    // Raw handling 50 -> 750 ms; baseline raises it to 60 -> 699 ms.
    assert_eq!(best["detail"]["aim_time"], 699.0);
    assert_eq!(best["selections"]["weapon:w1/slot:1:Muzzle_point"], "p1");
    assert_eq!(best["panel"]["10005"], 63.0);
}

#[test]
fn n2_movement_camera_metric_ignores_camera_scale() {
    let payload_json = movement_payload();
    let payload: WeaponPayload = serde_json::from_value(payload_json).unwrap();
    let profile = payload.numeric_profiles.get("w1").unwrap();
    assert_eq!(
        stability_metric_at(profile, "stability-movement-camera", 0.0),
        Some(1.0)
    );
    assert_eq!(
        stability_metric_at(profile, "stability-movement-camera", -10.0),
        Some(1.5)
    );
    let prices: FxHashMap<String, i64> = FxHashMap::default();
    let ev = evaluator(&payload, &prices);
    let detail = ev.detail_with(&[], &DetailSums::default());
    assert_eq!(detail.movement_camera, Some(1.0));
}

#[test]
fn n2_constraint_uses_scale_free_movement_camera() {
    let out = run_payload(
        &movement_payload(),
        json!({
            "prices": { "w1": 1000 },
            "weaponId": "w1",
            "mode": "operations",
            "constraints": [
                { "id": "stability-movement-camera", "max": 1.5 }
            ],
            "maxResults": 1
        }),
    );
    let best = out["best"]
        .as_object()
        .expect("scale-free metric at dS=0 is 1.0 and satisfies max 1.5");
    assert_eq!(best["detail"]["movement_camera"], 1.0);
    assert_eq!(best["panel"]["10007"], 50.0);
}
