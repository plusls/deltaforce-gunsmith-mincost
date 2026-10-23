use flate2::read::GzDecoder;
use serde_json::{json, Value};
use std::fs::File;
use std::io::Read;

fn solve(payload: &Value, request: Value) -> Value {
    let payload = serde_json::to_vec(payload).unwrap();
    let request = serde_json::to_vec(&request).unwrap();
    let out = gunsmith_mincost::solve_slices(&payload, &request).unwrap();
    serde_json::from_slice(&out).unwrap()
}

fn synthetic_payload() -> Value {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/lock_extra_occupy.payload.json"
    );
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

fn real_payload() -> Value {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/weapon-18010000006.json.gz"
    );
    let mut text = String::new();
    GzDecoder::new(File::open(path).unwrap())
        .read_to_string(&mut text)
        .unwrap();
    serde_json::from_str(&text).unwrap()
}

fn lock_installed(out: &Value, path: &str, part: &str) -> bool {
    out["installed_part_keys"]
        .as_array()
        .unwrap()
        .iter()
        .any(|key| key.as_str() == Some(&format!("{}\0{}", path, part)))
}

#[test]
fn a_conflicting_lock_makes_the_search_pick_another_slot() {
    let request = json!({
        "weaponId": "w1",
        "mode": "blackhawk",
        "laserEnabled": false,
        "prices": { "w1": 1334, "s1c0": 0, "s2c0": 210 },
        "constraints": [],
        "maxEvaluations": 4000000,
        "maxLeaves": 2000000,
        "maxResults": 3,
        "deadlineMs": 30000,
        "lockedPathKeys": { "weapon:w1/slot:3:Mag_point": "s2c0" }
    });
    let out = solve(&synthetic_payload(), request);
    assert_eq!(out["optimal"], true);
    assert!(out["truncated"] == false);
    let best = &out["best"];
    assert_eq!(best["total_price"], 1544);
    assert!(lock_installed(best, "weapon:w1/slot:3:Mag_point", "s2c0"));
}

#[test]
fn contradictory_locks_report_no_build_instead_of_a_stale_selection() {
    let request = json!({
        "weaponId": "w1",
        "mode": "blackhawk",
        "laserEnabled": false,
        "prices": { "w1": 1334, "s1c0": 0, "s2c0": 210 },
        "constraints": [],
        "maxEvaluations": 4000000,
        "maxLeaves": 2000000,
        "maxResults": 3,
        "deadlineMs": 30000,
        "lockedPathKeys": {
            "weapon:w1/slot:2:StoKit_point": "s1c0",
            "weapon:w1/slot:3:Mag_point": "s2c0"
        }
    });
    let out = solve(&synthetic_payload(), request);
    assert!(out["best"].is_null());
    assert_eq!(out["optimal"], false);
    assert_eq!(out["truncated"], false);
}

#[test]
fn real_payload_contradictory_sight_locks_report_no_build() {
    let request = json!({
        "weaponId": "18010000006",
        "mode": "operations",
        "laserEnabled": false,
        "prices": {},
        "constraints": [],
        "maxEvaluations": 2000000,
        "maxLeaves": 1000000,
        "maxResults": 3,
        "deadlineMs": 30000,
        "lockedPathKeys": {
            "weapon:18010000006/slot:11:Sco_point": "13110000049",
            "weapon:18010000006/slot:31:sco_secondary_point": "13110000076"
        }
    });
    let out = solve(&real_payload(), request);
    assert_eq!(out["truncated"], false);
    assert!(out["best"].is_null(), "unexpected build: {}", out["best"]);
    assert_eq!(out["optimal"], false);
}

#[test]
fn real_payload_without_locks_is_unchanged() {
    let request = json!({
        "weaponId": "18010000006",
        "mode": "operations",
        "laserEnabled": false,
        "prices": {},
        "constraints": [],
        "maxEvaluations": 2000000,
        "maxLeaves": 1000000,
        "maxResults": 3,
        "deadlineMs": 30000
    });
    let out = solve(&real_payload(), request);
    assert_eq!(out["optimal"], true);
    assert!(out["best"].is_object());
}
