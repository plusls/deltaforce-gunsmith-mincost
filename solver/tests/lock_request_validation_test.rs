use serde_json::{json, Value};

fn tuning_payload() -> Value {
    json!({
        "dataVersion": "request-validation",
        "weaponId": "w1",
        "weaponIds": ["w1"],
        "parts": {
            "w1": {"id": "w1", "name": "Test Gun", "officialObjectId": "w1"},
            "pz": {
                "id": "pz",
                "name": "pz",
                "officialObjectId": "pz",
                "tuningOptions": [{
                    "tune_id": "7001",
                    "min_value": 0,
                    "max_value": 20,
                    "step_value": 1,
                    "default_value": 0,
                    "display_effects": [{
                        "attribute_name_snapshot": "%额外后坐力控制",
                        "curve_points": [
                            {"Time": 0, "Value": 0, "InterpMode": "RCIM_Linear", "key_index": 0},
                            {"Time": 20, "Value": 20, "InterpMode": "RCIM_Linear", "key_index": 1}
                        ]
                    }],
                    "runtime_targets": [{"runtime_target": "GRecoil_H"}]
                }]
            },
            "plain": {"id": "plain", "name": "plain", "officialObjectId": "plain"}
        },
        "slotsByHost": {"w1": [{
            "id": "1",
            "name": "Muz_point",
            "label": "muzzle",
            "type": "Muzzle",
            "candidateIds": ["pz", "plain"],
            "virtualCandidateIds": [],
            "defaultHidden": false,
            "hideForPlayer": false,
            "allCandidatesVirtual": false
        }]},
        "weaponStatsById": {"w1": {"byMode": {"operations": {
            "base": {"10001": 50, "10005": 50, "10006": 50, "10007": 50, "10008": 50},
            "display": {"10001": 50, "10005": 50, "10006": 50, "10007": 50, "10008": 50}
        }}}},
        "weaponNumericProfilesById": {"w1": {"linkedTables": {
            "recoilCurves": {
                "recoilControl": {"keys": [
                    {"Time": 0, "Value": 1.0, "InterpMode": "RCIM_Linear", "key_index": 0},
                    {"Time": 100, "Value": 0.5, "InterpMode": "RCIM_Linear", "key_index": 1}
                ]},
                "gunkick": {"keys": [
                    {"Time": 0, "Value": 1.0, "InterpMode": "RCIM_Linear", "key_index": 0},
                    {"Time": 100, "Value": 0.8, "InterpMode": "RCIM_Linear", "key_index": 1}
                ]}
            },
            "aimRecoil": {
                "ContinueShootRecoil": {"HorizontalScale": 1.0, "VerticalScale": 1.0},
                "SingleOrBurstShootRecoil": {"HorizontalScale": 1.0, "VerticalScale": 1.0}
            }
        }}},
    })
}

fn base_request() -> Value {
    json!({
        "prices": {"w1": 1000, "pz": 100, "plain": 0},
        "weaponId": "w1",
        "mode": "operations",
        "constraints": [],
        "maxEvaluations": 100000,
        "maxLeaves": 100000,
        "maxResults": 3
    })
}

fn run(payload: &Value, request: &Value) -> Result<Value, String> {
    let payload = serde_json::to_vec(payload).unwrap();
    let request = serde_json::to_vec(request).unwrap();
    let out = gunsmith_mincost::solve_slices(&payload, &request)?;
    Ok(serde_json::from_slice(&out).unwrap())
}

const SLOT: &str = "weapon:w1/slot:1:Muz_point";

#[test]
fn a_locked_part_in_the_exclusion_list_is_rejected() {
    let payload = tuning_payload();
    let mut request = base_request();
    request["lockedPathKeys"] = json!({ SLOT: "pz" });
    request["excludedPartIds"] = json!(["pz"]);
    let err = run(&payload, &request).unwrap_err();
    assert!(err.contains("当前配装与锁定条件不一致"), "{err}");
}

#[test]
fn a_stale_locked_slot_path_is_rejected() {
    let payload = tuning_payload();
    let mut request = base_request();
    request["lockedPathKeys"] = json!({"weapon:w1/slot:9:No_point": null});
    let err = run(&payload, &request).unwrap_err();
    assert!(err.contains("锁定槽位已失效"), "{err}");
}

#[test]
fn a_stale_locked_tuning_is_rejected() {
    let payload = tuning_payload();
    let mut request = base_request();
    let key = format!("{SLOT}\0pz\0missing_tune");
    request["lockedTuningKeys"] = json!({ key: 5.0 });
    let err = run(&payload, &request).unwrap_err();
    assert!(err.contains("锁定精校已失效"), "{err}");
}

#[test]
fn a_locked_tuning_key_for_another_part_is_rejected() {
    let payload = tuning_payload();
    let mut request = base_request();
    let key = format!("{SLOT}\0plain\07001");
    request["lockedPathKeys"] = json!({ SLOT: "pz" });
    request["lockedTuningKeys"] = json!({ key: 5.0 });
    let err = run(&payload, &request).unwrap_err();
    assert!(err.contains("锁定精校已失效"), "{err}");
}

#[test]
fn a_locked_tuning_key_locks_its_slot_to_that_part() {
    let payload = tuning_payload();
    let mut request = base_request();
    let key = format!("{SLOT}\0pz\07001");
    request["lockedTuningKeys"] = json!({ key: 0.0 });
    let out = run(&payload, &request).unwrap();
    assert_eq!(out["optimal"], true, "{out}");
    assert_eq!(out["best"]["selections"][SLOT], "pz", "{out}");
    assert_eq!(out["best"]["total_price"], 1100, "{out}");
}

#[test]
fn a_valid_locked_path_still_solves() {
    let payload = tuning_payload();
    let mut request = base_request();
    request["lockedPathKeys"] = json!({ SLOT: "pz" });
    let out = run(&payload, &request).unwrap();
    assert_eq!(out["optimal"], true, "{out}");
    assert_eq!(out["best"]["selections"][SLOT], "pz", "{out}");
    assert_eq!(out["best"]["total_price"], 1100, "{out}");
}
