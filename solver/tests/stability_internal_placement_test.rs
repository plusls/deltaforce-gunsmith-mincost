use flate2::read::GzDecoder;
use gunsmith_mincost::constraint_reduction::reduce_stability_constraints;
use gunsmith_mincost::model::WeaponPayload;
use gunsmith_mincost::search::Constraint;
use serde_json::{json, Value};
use std::fs::File;

const WEAPON: &str = "18060000009";
const SIGHT: &str = "13110000057";

fn payload() -> WeaponPayload {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/weapon-18060000009.json.gz"
    );
    let file = File::open(path).expect("fixture payload");
    serde_json::from_reader(GzDecoder::new(file)).expect("decode fixture payload")
}

/// The stock preset installs the sight at slot 11. Pointing the three virtual
/// default nodes at the same sight puts it in four internal sockets, which the
/// assembler really does (see the C15 reproduction).
fn four_internal_sights() -> WeaponPayload {
    let mut payload = payload();
    let preset = payload
        .default_presets
        .get_mut(WEAPON)
        .expect("default preset");
    let mut moved = 0;
    for node in &mut preset.nodes {
        if node.is_root || node.part_id == SIGHT {
            continue;
        }
        node.part_id = SIGHT.to_string();
        node.is_virtual = false;
        moved += 1;
    }
    assert_eq!(moved, 3, "expected three virtual nodes");
    payload
}

fn without_sight_stability(payload: &WeaponPayload) -> WeaponPayload {
    let mut out = payload.clone();
    let part = out.parts.get_mut(SIGHT).expect("sight");
    for list in part.display_mods.values_mut() {
        list.retain(|m| m.attribute_id.as_deref() != Some("10007"));
    }
    out
}

fn mapped_min(payload: &WeaponPayload) -> f64 {
    let constraints = vec![Constraint::new("stability-fire-camera", None, Some(0.2))];
    let reduced = reduce_stability_constraints(payload, WEAPON, "operations", &constraints);
    assert_eq!(reduced.len(), 1, "{reduced:?}");
    assert_eq!(reduced[0].id, "10007", "{reduced:?}");
    reduced[0].min.expect("lower-bound reduction")
}

#[test]
fn four_internal_sights_widen_the_bound_by_every_placement() {
    let multi = four_internal_sights();
    let with_sight = mapped_min(&multi);
    let exact = mapped_min(&without_sight_stability(&multi));

    // Four placements at +6 each contribute 24 to dS while the panel stays
    // flat. A bound that assumes one internal slot (or even two) undercounts.
    let widened = exact - with_sight;
    assert!(
        widened >= 4.0 * 6.0 - 1e-6,
        "bound widened by {widened}, expected at least 24"
    );
}

#[test]
fn single_internal_sight_keeps_the_historic_widening() {
    // The stock preset has one internal placement. The historic bracket counts
    // one internal plus one visible copy, so this stays at 12.
    let stock = payload();
    let with_sight = mapped_min(&stock);
    let exact = mapped_min(&without_sight_stability(&stock));
    assert!((exact - with_sight - 12.0).abs() < 1e-6);
}

fn fire_camera_curve() -> Value {
    json!([
        { "Time": 0, "Value": 1.0 },
        { "Time": 24, "Value": 0.1 }
    ])
}

fn synthetic_payload() -> Value {
    let paths = [
        "weapon:w1/slot:1:A_point",
        "weapon:w1/slot:2:B_point",
        "weapon:w1/slot:3:C_point",
        "weapon:w1/slot:4:D_point",
    ];
    let mut nodes = vec![json!({ "isRoot": true, "partId": "w1" })];
    for path in paths {
        nodes.push(json!({
            "isRoot": false,
            "isVirtual": false,
            "partId": "p_stab",
            "pathKey": path
        }));
    }
    json!({
        "dataVersion": "placement-test",
        "parts": {
            "w1": { "id": "w1", "name": "Test Gun", "officialObjectId": "w1" },
            "p_stab": {
                "id": "p_stab",
                "name": "stability insert",
                "officialObjectId": "p_stab",
                "modifiersByMode": { "operations": [
                    { "attributeId": "10007", "modifierType": "Addend", "delta": 6 }
                ] }
            }
        },
        "slotsByHost": {
            "w1": [
                { "id": "1", "name": "A_point", "candidateIds": [] },
                { "id": "2", "name": "B_point", "candidateIds": [] },
                { "id": "3", "name": "C_point", "candidateIds": [] },
                { "id": "4", "name": "D_point", "candidateIds": [] }
            ]
        },
        "defaultPresetsByWeapon": { "w1": { "nodes": nodes } },
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
                        "fireCameraStability": { "keys": fire_camera_curve() },
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

#[test]
fn synthetic_four_internal_placements_keep_the_feasible_build() {
    let payload = synthetic_payload();
    let constraints = json!([{ "id": "stability-fire-camera", "max": 0.2 }]);

    let reduced = reduce_stability_constraints(
        &serde_json::from_value::<WeaponPayload>(payload.clone()).unwrap(),
        "w1",
        "operations",
        &serde_json::from_value::<Vec<Constraint>>(constraints.clone()).unwrap(),
    );
    assert_eq!(reduced.len(), 1, "{reduced:?}");
    assert_eq!(reduced[0].id, "10007", "{reduced:?}");
    // base 50 + threshold (~21.2) - 4 * 6 - one extra visible copy.
    assert!(
        reduced[0].min.unwrap() < 50.0,
        "mapped bound {:?} would prune the stock build",
        reduced[0].min
    );

    let request = json!({
        "prices": {},
        "weaponId": "w1",
        "mode": "operations",
        "constraints": constraints,
        "maxEvaluations": 100000,
        "maxLeaves": 100000,
        "maxResults": 5,
        "deadlineMs": 30000
    });
    let out = gunsmith_mincost::solve_slices(
        &serde_json::to_vec(&payload).unwrap(),
        &serde_json::to_vec(&request).unwrap(),
    )
    .unwrap();
    let out: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(out["optimal"], true, "{out}");
    let best = &out["best"];
    assert!(
        best["detail"]["fire_camera"].as_f64().unwrap() <= 0.2 + 1e-9,
        "{out}"
    );
}
