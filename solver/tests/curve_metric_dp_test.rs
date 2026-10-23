use gunsmith_mincost::constraint_reduction::reduce_stability_constraints;
use gunsmith_mincost::model::WeaponPayload;
use gunsmith_mincost::search::Constraint;
use serde_json::{json, Value};

fn curve_payload(fire_camera_keys: Value) -> Value {
    json!({
        "dataVersion": "curve-test",
        "parts": {
            "w1": { "id": "w1", "name": "Test Gun", "officialObjectId": "w1" },
            "p_zero": {
                "id": "p_zero",
                "name": "zero",
                "officialObjectId": "p_zero",
                "modifiersByMode": { "operations": [
                    { "attributeId": "10007", "modifierType": "Addend", "delta": 0 }
                ] }
            },
            "p_lo": {
                "id": "p_lo",
                "name": "eleven point seven",
                "officialObjectId": "p_lo",
                "modifiersByMode": { "operations": [
                    { "attributeId": "10007", "modifierType": "Addend", "delta": 11.7 }
                ] }
            },
            "p_hi": {
                "id": "p_hi",
                "name": "eleven point nine",
                "officialObjectId": "p_hi",
                "modifiersByMode": { "operations": [
                    { "attributeId": "10007", "modifierType": "Addend", "delta": 11.9 }
                ] }
            },
            "p_hi2": {
                "id": "p_hi2",
                "name": "twelve",
                "officialObjectId": "p_hi2",
                "modifiersByMode": { "operations": [
                    { "attributeId": "10007", "modifierType": "Addend", "delta": 12 }
                ] }
            }
        },
        "slotsByHost": {
            "w1": [
                { "id": "1", "name": "Muzzle_point", "candidateIds": ["p_zero", "p_lo", "p_hi", "p_hi2"] }
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
                        "base": { "10001": 50, "10005": 50, "10006": 50, "10007": 50, "10008": 50 },
                        "display": { "10001": 50, "10005": 50, "10006": 50, "10007": 50, "10008": 50 }
                    }
                }
            }
        },
        "weaponNumericProfilesById": {
            "w1": {
                "linkedTables": {
                    "aimRecoil": {
                        "RecoilShakePitchWithCurve": { "WholeAmplitude": 1.0 },
                        "RecoilShakeYawWithCurve": { "WholeAmplitude": 1.0 }
                    },
                    "recoilCurves": {
                        "recoilControl": { "keys": [{ "Time": 0, "Value": 1.0 }] },
                        "gunkick": { "keys": [{ "Time": 0, "Value": 1.0 }] }
                    },
                    "stabilityCurves": {
                        "breathScale": { "keys": [{ "Time": 0, "Value": 1.0 }] },
                        "breathTime": { "keys": [{ "Time": 0, "Value": 1.0 }] },
                        "fireCameraStability": { "keys": fire_camera_keys },
                        "movementCameraStability": { "keys": [{ "Time": 0, "Value": 1.0 }] },
                        "holdBreathRecoil": {
                            "keys": [{ "Time": 0, "Value": 1.0 }, { "Time": 16, "Value": 0.8 }]
                        }
                    },
                    "holdBreath": [{
                        "StandBreathFactor": 1.0,
                        "RecoilScale": { "X": 1.0, "Y": 1.0 },
                        "WeaponBreathConfig": { "AttenSpeed": 5.0 }
                    }]
                },
                "detail": {
                    "raw_supporting_values": {
                        "stability": { "GSMovement_CameraScale": 1.0 }
                    }
                }
            }
        }
    })
}

fn fire_camera_curve() -> Value {
    json!([
        { "Time": 0, "Value": 1.0, "InterpMode": "RCIM_Linear" },
        { "Time": 8, "Value": 0.3, "InterpMode": "RCIM_Linear" },
        { "Time": 16, "Value": 0.12, "InterpMode": "RCIM_Linear" }
    ])
}

fn reduce(payload: &Value, constraints: Value) -> Vec<Constraint> {
    let payload: WeaponPayload = serde_json::from_value(payload.clone()).unwrap();
    let constraints: Vec<Constraint> = serde_json::from_value(constraints).unwrap();
    reduce_stability_constraints(&payload, "w1", "operations", &constraints)
}

fn run(payload: &Value, request: Value) -> Value {
    let payload = serde_json::to_vec(payload).unwrap();
    let request = serde_json::to_vec(&request).unwrap();
    let out = gunsmith_mincost::solve_slices(&payload, &request).unwrap();
    serde_json::from_slice(&out).unwrap()
}

#[test]
fn fire_camera_max_maps_to_the_stability_panel() {
    let reduced = reduce(
        &curve_payload(fire_camera_curve()),
        json!([{ "id": "stability-fire-camera", "max": 0.21 }]),
    );
    assert_eq!(reduced.len(), 1);
    assert_eq!(reduced[0].id, "10007");
    assert!(reduced[0].max.is_none());
    let min = reduced[0].min.unwrap();
    // round2 boundary: curve(dS) rounds to 0.21 from dS >= 11.777..., not 12.
    assert!((min - 61.777).abs() < 1e-2, "unexpected mapped min {min}");
}

#[test]
fn fire_camera_rounding_boundary_is_conservative() {
    let payload = curve_payload(fire_camera_curve());
    let out = run(
        &payload,
        json!({
            "prices": { "w1": 1000, "p_zero": 50, "p_lo": 100, "p_hi": 200, "p_hi2": 300 },
            "weaponId": "w1",
            "mode": "operations",
            "constraints": [{ "id": "stability-fire-camera", "max": 0.21 }],
            "maxResults": 2
        }),
    );
    let best = out["best"].as_object().unwrap();
    assert_eq!(out["optimal"], true);
    // p_lo reaches 0.21675 which rounds to 0.22 and must be rejected;
    // p_hi reaches 0.21225 which rounds to 0.21 and is the cheapest match.
    assert_eq!(best["total_price"], 1200);
    assert_eq!(best["detail"]["fire_camera"], 0.21);
    let constraints = best["constraints"].as_array().unwrap();
    assert_eq!(constraints[0]["id"], "stability-fire-camera");
    assert_eq!(constraints[0]["ok"], true);
}

#[test]
fn non_monotone_curve_is_not_reduced() {
    let payload = curve_payload(json!([
        { "Time": 0, "Value": 1.0 },
        { "Time": 8, "Value": 0.1 },
        { "Time": 16, "Value": 0.5 }
    ]));
    let reduced = reduce(
        &payload,
        json!([{ "id": "stability-fire-camera", "max": 0.21 }]),
    );
    assert_eq!(reduced.len(), 1);
    assert_eq!(reduced[0].id, "stability-fire-camera");
}

#[test]
fn extra_stability_attribute_blocks_reduction() {
    let mut payload = curve_payload(fire_camera_curve());
    payload["parts"]["p_extra"] = json!({
        "id": "p_extra",
        "name": "extra stability attr",
        "modifiersByMode": { "operations": [
            { "attributeId": "10044", "modifierType": "Addend", "delta": 1 }
        ] }
    });
    let reduced = reduce(
        &payload,
        json!([{ "id": "stability-fire-camera", "max": 0.21 }]),
    );
    assert_eq!(reduced[0].id, "stability-fire-camera");
}

#[test]
fn movement_camera_with_tuning_effects_is_not_reduced() {
    let mut payload = curve_payload(fire_camera_curve());
    payload["parts"]["p_zero"]["tuningOptions"] = json!([{
        "tune_id": "t1",
        "min_value": -10,
        "max_value": 10,
        "step_value": 1,
        "display_effects": [{
            "attribute_name_snapshot": "%移动时据枪稳定性",
            "curve_points": [{ "time": -10, "value": 0 }, { "time": 10, "value": 5 }]
        }]
    }]);
    let reduced = reduce(
        &payload,
        json!([{ "id": "stability-movement-camera", "max": 0.5 }]),
    );
    assert_eq!(reduced[0].id, "stability-movement-camera");
}

#[test]
fn breath_range_with_a_breath_multiplier_is_not_reduced() {
    let mut payload = curve_payload(fire_camera_curve());
    payload["parts"]["p_extra"] = json!({
        "id": "p_extra",
        "name": "breath multiplier",
        "rawModifiersByMode": { "operations": [
            { "modifierType": "Mult_C", "targetPath": "X.StandBreathFactor", "param1": 0.9 }
        ] }
    });
    let reduced = reduce(
        &payload,
        json!([{ "id": "stability-breath-range", "max": 0.3 }]),
    );
    assert_eq!(reduced[0].id, "stability-breath-range");
}

#[test]
fn hold_breath_recoil_max_is_reduced() {
    let reduced = reduce(
        &curve_payload(fire_camera_curve()),
        json!([{ "id": "stability-hold-breath-recoil", "max": 0.9 }]),
    );
    assert_eq!(reduced[0].id, "10007");
    let min = reduced[0].min.unwrap();
    assert!((min - 58.0).abs() < 1e-2, "unexpected mapped min {min}");
}

#[test]
fn real_fire_camera_case_uses_the_dp() {
    let payload = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/rm277-stability-fire-camera.payload.json"
    ))
    .unwrap();
    let request = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/rm277-stability-fire-camera.request.json"
    ))
    .unwrap();
    let out = gunsmith_mincost::solve_slices(&payload, &request).unwrap();
    let out: Value = serde_json::from_slice(&out).unwrap();
    let best = out["best"].as_object().unwrap();
    assert_eq!(out["optimal"], true);
    assert_eq!(out["truncated"], false);
    // 348828 was previous best; exact tuning-bound inversion finds a cheaper
    // build sitting exactly on the 0.77 fire-stability boundary.
    assert_eq!(best["total_price"], 345302);
    assert_eq!(best["attachment_price"], 239437);
    assert_eq!(best["detail"]["fire_camera"], 0.21);
    for check in best["constraints"].as_array().unwrap() {
        assert_eq!(check["ok"], true, "constraint failed: {check}");
    }
    // The full enumeration path needs 1M leaves; the reduced DP path is tiny.
    assert!(out["leaves"].as_u64().unwrap() < 5000);
}
