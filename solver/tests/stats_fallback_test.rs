use gunsmith_mincost::assembly::Assembler;
use gunsmith_mincost::metrics::{has_laser, BuildMetrics, Evaluator, TuningState};
use gunsmith_mincost::model::{Map, WeaponPayload};
use flate2::read::GzDecoder;
use rustc_hash::FxHashMap;
use serde_json::{json, Value};
use std::fs::File;

const REAL_WEAPON: &str = "18070000010";

fn real_payload() -> WeaponPayload {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/weapon-18070000010.json.gz"
    );
    let file = File::open(path).expect("fixture payload");
    serde_json::from_reader(GzDecoder::new(file)).expect("decode fixture payload")
}

fn real_prices() -> FxHashMap<String, i64> {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/prices-18070000010.json"
    );
    let raw: Value = serde_json::from_str(&std::fs::read_to_string(path).expect("fixture prices"))
        .expect("decode fixture prices");
    raw["prices"]
        .as_object()
        .expect("prices object")
        .iter()
        .map(|(id, price)| (id.clone(), price.as_f64().unwrap_or(0.0).round() as i64))
        .collect()
}

fn synthetic_payload(stats: Value) -> WeaponPayload {
    serde_json::from_value(json!({
        "parts": { "w1": { "id": "w1", "name": "W" } },
        "slotsByHost": { "w1": [] },
        "weaponStatsById": { "w1": stats },
    }))
    .expect("decode synthetic payload")
}

fn evaluate(
    payload: &WeaponPayload,
    prices: &FxHashMap<String, i64>,
    weapon_id: &str,
    mode: &str,
    laser_enabled: bool,
    selections: Map<String, Option<String>>,
) -> BuildMetrics {
    let assembler = Assembler::new(payload, weapon_id);
    let assembly = assembler.resolve(&selections);
    let evaluator = Evaluator {
        payload,
        weapon_id,
        mode,
        laser_enabled,
        bipod_deployed: false,
        prices,
    };
    evaluator
        .evaluate(&assembler, &assembly, &TuningState::default())
        .expect("evaluate assembly")
}

#[test]
fn real_weapon_blackhawk_falls_back_to_top_level_stats() {
    let payload = real_payload();
    let prices = real_prices();
    let stats = payload.weapon_stats.get(REAL_WEAPON).expect("weapon stats");
    assert!(
        !stats.by_mode.contains_key("blackhawk"),
        "fixture must exercise the missing-mode fallback"
    );

    for mode in ["operations", "blackhawk"] {
        let metrics = evaluate(&payload, &prices, REAL_WEAPON, mode, true, Map::default());
        // Site `st(stats, "blackhawk")` has no byMode entry and returns the
        // weapon's top-level stats object.
        assert_eq!(metrics.panel["10001"], 27.0, "{mode} 10001");
        assert_eq!(metrics.panel["10005"], 42.0, "{mode} 10005");
        assert_eq!(metrics.panel["10006"], 70.0, "{mode} 10006");
        assert_eq!(metrics.panel["10007"], 35.0, "{mode} 10007");
        assert_eq!(metrics.panel["10008"], 75.0, "{mode} 10008");
        assert_eq!(metrics.detail.aim_time, Some(230.0), "{mode} aim_time");
    }

    let blackhawk = evaluate(
        &payload,
        &prices,
        REAL_WEAPON,
        "blackhawk",
        true,
        Map::default(),
    );
    let detail = &blackhawk.detail;
    assert_eq!(detail.handling_score, 0.0);
    assert_eq!(detail.stability_score, 0.0);
    assert_eq!(detail.hipfire_score, 0.0);
    assert_eq!(detail.sprint_fire_delay, Some(79.0));
    assert_eq!(detail.aim_move_speed, None);
    assert_eq!(detail.reload_time, Some(2.1));
    assert_eq!(detail.empty_reload_time, Some(2.2));
    assert_eq!(detail.holster_time, Some(0.2));
    assert_eq!(detail.switch_time, Some(0.3));
    assert_eq!(detail.breath_duration, Some(20.0));
    assert!((detail.breath_range.unwrap() - 0.47).abs() < 1e-9);
    assert_eq!(detail.movement_camera, Some(1.0));
    assert_eq!(detail.fire_camera, Some(1.0));
    assert_eq!(detail.hit_sway, Some(0.8));
    assert_eq!(detail.hold_breath_recoil, Some(1.0));
    assert_eq!(detail.hipfire_standing, Some(2.18));
    assert_eq!(detail.hipfire_maximum, Some(5.0));
    assert_eq!(blackhawk.missing_price_count, 0);
}

#[test]
fn flat_mode_stats_are_used_directly() {
    let payload = synthetic_payload(json!({
        "byMode": {
            "operations": {
                "10001": 11, "10005": 22, "10006": 33, "10007": 44, "10008": 55
            }
        }
    }));
    let stats = payload.weapon_stats.get("w1").unwrap();
    for (id, expected) in [
        ("10001", 11.0),
        ("10005", 22.0),
        ("10006", 33.0),
        ("10007", 44.0),
        ("10008", 55.0),
    ] {
        assert_eq!(stats.display_stat("operations", id), Some(expected));
        assert_eq!(stats.base_stat("operations", id), Some(expected));
    }

    let metrics = evaluate(
        &payload,
        &FxHashMap::default(),
        "w1",
        "operations",
        true,
        Map::default(),
    );
    assert_eq!(metrics.panel["10001"], 11.0);
    assert_eq!(metrics.panel["10005"], 22.0);
    assert_eq!(metrics.panel["10006"], 33.0);
    assert_eq!(metrics.panel["10007"], 44.0);
    assert_eq!(metrics.panel["10008"], 55.0);
}

#[test]
fn missing_weapon_price_counts_as_missing() {
    let payload = synthetic_payload(json!({
        "byMode": { "operations": { "base": { "10006": 50 }, "display": { "10006": 50 } } }
    }));

    let without_weapon = evaluate(
        &payload,
        &FxHashMap::default(),
        "w1",
        "operations",
        true,
        Map::default(),
    );
    assert_eq!(without_weapon.missing_price_count, 1);
    assert_eq!(without_weapon.total_price, 0);

    let mut prices = FxHashMap::default();
    prices.insert("w1".to_string(), 100);
    let with_weapon = evaluate(&payload, &prices, "w1", "operations", true, Map::default());
    assert_eq!(with_weapon.missing_price_count, 0);
    assert_eq!(with_weapon.total_price, 100);
}

fn laser_payload() -> Value {
    json!({
        "parts": {
            "w1": { "id": "w1", "name": "W" },
            "LB": {
                "id": "LB",
                "name": "LB",
                "uiEffectsByMode": {
                    "warfare": [{ "conditionId": "10038" }],
                    "operations": []
                },
                "modifiersByMode": {
                    "operations": [
                        {
                            "attributeId": "10006",
                            "modifierType": "Addend",
                            "delta": 20,
                            "polarity": "pro",
                            "conditionId": null
                        }
                    ]
                }
            }
        },
        "slotsByHost": {
            "w1": [{ "id": "900", "name": "Laser", "type": "Laser_point", "candidateIds": ["LB"] }]
        },
        "weaponStatsById": {
            "w1": {
                "byMode": {
                    "operations": { "base": { "10006": 50 }, "display": { "10006": 50 } }
                }
            }
        }
    })
}

#[test]
fn laser_detection_scans_every_mode() {
    let payload: WeaponPayload = serde_json::from_value(laser_payload()).expect("payload");
    assert!(has_laser(&payload.parts["LB"]));

    let mut selections = Map::default();
    selections.insert(
        "weapon:w1/slot:900:Laser".to_string(),
        Some("LB".to_string()),
    );
    let mut prices = FxHashMap::default();
    prices.insert("w1".to_string(), 100);
    prices.insert("LB".to_string(), 10);

    let enabled = evaluate(
        &payload,
        &prices,
        "w1",
        "operations",
        true,
        selections.clone(),
    );
    assert_eq!(enabled.panel["10006"], 70.0);

    // A mode-scoped laser check would not classify LB as a laser in
    // operations and would keep the pro addend even with lasers disabled.
    let disabled = evaluate(&payload, &prices, "w1", "operations", false, selections);
    assert_eq!(disabled.panel["10006"], 50.0);
}
