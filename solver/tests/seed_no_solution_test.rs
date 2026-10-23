use serde_json::{json, Value};

fn spec(bad_parts: usize) -> (Value, Value) {
    let tune = json!({
        "tune_id": "t0",
        "min_value": -100.0,
        "max_value": 100.0,
        "step_value": 10.0,
        "default_value": 0.0,
        "display_effects": [
            {
                "attribute_name_snapshot": "%额外后坐力控制",
                "curve_points": [{"Time": -100, "Value": -100}, {"Time": 100, "Value": 100}]
            },
            {
                "attribute_name_snapshot": "%开火时武器稳定性",
                "curve_points": [{"Time": -100, "Value": 100}, {"Time": 100, "Value": -100}]
            }
        ],
        "runtime_targets": []
    });
    let mut parts = serde_json::Map::new();
    let mut prices = serde_json::Map::new();
    let mut candidates = Vec::new();
    parts.insert("w1".to_string(), json!({"id": "w1", "name": "gun"}));
    prices.insert("w1".to_string(), json!(1000));
    for i in 0..bad_parts {
        let id = format!("B{i:05}");
        parts.insert(
            id.clone(),
            json!({
                "id": id,
                "name": id,
                "rawModifiersByMode": {"operations": [
                    {"modifierType": "Mult_C", "param1": 1.0 - i as f64 * 0.00001, "targetPath": "GRecoil_H"},
                    {"modifierType": "Mult_C", "param1": 1.0 + i as f64 * 0.00001, "targetPath": "GGunkickSpring"}
                ]},
                "tuningOptions": [tune]
            }),
        );
        prices.insert(id.clone(), json!(0));
        candidates.push(id);
    }
    parts.insert(
        "F".to_string(),
        json!({
            "id": "F",
            "name": "F",
            "rawModifiersByMode": {"operations": [
                {"modifierType": "Mult_C", "param1": 0.2, "targetPath": "GRecoil_H"},
                {"modifierType": "Mult_C", "param1": 0.2, "targetPath": "GGunkickSpring"}
            ]}
        }),
    );
    prices.insert("F".to_string(), json!(0));
    candidates.push("F".to_string());
    let payload = json!({
        "dataVersion": "seedcap",
        "parts": parts,
        "slotsByHost": {"w1": [{
            "id": "1",
            "name": "Slot0",
            "candidateIds": candidates,
            "virtualCandidateIds": [],
            "defaultHidden": false,
            "hideForPlayer": false,
            "allCandidatesVirtual": false
        }]},
        "defaultPresetsByWeapon": {"w1": {"nodes": [{"isRoot": true, "partId": "w1"}]}},
        "virtualRulesByWeapon": {"w1": []},
        "candidateOverridesByWeapon": {},
        "weaponStatsById": {"w1": {"byMode": {"operations": {
            "base": {"10001": 55, "10005": 48, "10006": 50, "10007": 48, "10008": 50},
            "display": {"10001": 55, "10005": 48, "10006": 50, "10007": 48, "10008": 50}
        }}}},
        "weaponNumericProfilesById": {"w1": {"linkedTables": {
            "recoilCurves": {
                "recoilControl": {"keys": [
                    {"Time": 0, "Value": 1.0, "InterpMode": "RCIM_Linear", "key_index": 0},
                    {"Time": 100, "Value": 1.0, "InterpMode": "RCIM_Linear", "key_index": 1}
                ]},
                "gunkick": {"keys": [
                    {"Time": 0, "Value": 1.0, "InterpMode": "RCIM_Linear", "key_index": 0},
                    {"Time": 100, "Value": 1.0, "InterpMode": "RCIM_Linear", "key_index": 1}
                ]}
            },
            "aimRecoil": {
                "ContinueShootRecoil": {"HorizontalScale": 1.0, "VerticalScale": 1.0},
                "SingleOrBurstShootRecoil": {"HorizontalScale": 1.0, "VerticalScale": 1.0}
            }
        }}},
    });
    let request = json!({
        "prices": prices,
        "weaponId": "w1",
        "mode": "operations",
        "laserEnabled": true,
        "bipodDeployed": false,
        "lockedPathKeys": {},
        "lockedTuningKeys": {},
        "excludedPartIds": [],
        "constraints": [
            {"id": "recoil-continuous-horizontal", "max": 0.5},
            {"id": "recoil-fire-stability", "max": 0.5}
        ],
        "maxEvaluations": 5000000,
        "maxLeaves": 2000000,
        "maxResults": 3,
        "costCap": null,
        "firstSolution": false,
        "deadlineMs": 120000
    });
    (payload, request)
}

fn run(bad_parts: usize) -> Value {
    let (payload, request) = spec(bad_parts);
    let payload = serde_json::to_vec(&payload).unwrap();
    let request = serde_json::to_vec(&request).unwrap();
    let out = gunsmith_mincost::solve_slices(&payload, &request).unwrap();
    serde_json::from_slice(&out).unwrap()
}

#[test]
#[ignore = "slow: 8192 candidate seed cap boundary"]
fn seed_without_a_build_still_runs_the_exact_pass() {
    let out = run(8192);
    assert_eq!(out["truncated"], false, "{out}");
    assert_eq!(out["optimal"], true, "{out}");
    assert_eq!(out["best"]["total_price"], 1000, "{out}");
    assert_eq!(out["message"], "search complete", "{out}");
}

#[test]
#[ignore = "slow: full exact pass over 8191 candidates"]
fn seed_with_a_build_reports_the_optimum() {
    let out = run(8191);
    assert_eq!(out["truncated"], false, "{out}");
    assert_eq!(out["optimal"], true, "{out}");
    assert_eq!(out["best"]["total_price"], 1000, "{out}");
}

#[test]
fn a_seed_that_finds_a_build_still_reports_the_optimum() {
    let out = run(2);
    assert_eq!(out["truncated"], false, "{out}");
    assert_eq!(out["optimal"], true, "{out}");
    assert_eq!(out["best"]["total_price"], 1000, "{out}");
    assert_eq!(out["message"], "search complete", "{out}");
}

#[test]
fn a_seed_that_finds_nothing_does_not_claim_a_best_build() {
    let mut payload = spec(2).0;
    payload["slotsByHost"]["w1"][0]["candidateIds"] = json!(["B00000", "B00001"]);
    let (_, request) = spec(2);
    let payload = serde_json::to_vec(&payload).unwrap();
    let request = serde_json::to_vec(&request).unwrap();
    let out = gunsmith_mincost::solve_slices(&payload, &request).unwrap();
    let out: Value = serde_json::from_slice(&out).unwrap();
    assert!(out["best"].is_null(), "{out}");
    assert_eq!(out["optimal"], false, "{out}");
    assert!(
        !out["message"].as_str().unwrap().contains("best found so far"),
        "{out}"
    );
}
