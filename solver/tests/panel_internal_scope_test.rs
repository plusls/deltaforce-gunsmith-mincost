use gunsmith_mincost::assembly::Assembler;
use gunsmith_mincost::constraint_reduction::reduce_stability_constraints;
use gunsmith_mincost::metrics::{BuildMetrics, Evaluator, TuningState};
use gunsmith_mincost::model::{Num, WeaponPayload};
use gunsmith_mincost::search::{Constraint, SolveOptions, SolveResult, Solver};
use flate2::read::GzDecoder;
use rustc_hash::FxHashMap;
use std::fs::File;

const WEAPON: &str = "18060000009";
const INTERNAL_SIGHT: &str = "13110000057";

fn payload() -> WeaponPayload {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/weapon-18060000009.json.gz"
    );
    let file = File::open(path).expect("fixture payload");
    serde_json::from_reader(GzDecoder::new(file)).expect("decode fixture payload")
}

fn prices() -> FxHashMap<String, i64> {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/prices-18060000009.json"
    );
    let raw: FxHashMap<String, f64> =
        serde_json::from_str(&std::fs::read_to_string(path).expect("fixture prices"))
            .expect("decode fixture prices");
    raw.into_iter()
        .map(|(id, price)| (id, price.round() as i64))
        .collect()
}

fn evaluate(payload: &WeaponPayload, prices: &FxHashMap<String, i64>, mode: &str) -> BuildMetrics {
    let assembler = Assembler::new(payload, WEAPON);
    let assembly = assembler.resolve(&FxHashMap::default());
    let evaluator = Evaluator {
        payload,
        weapon_id: WEAPON,
        mode,
        laser_enabled: true,
        bipod_deployed: false,
        prices,
    };
    evaluator
        .evaluate(&assembler, &assembly, &TuningState::default())
        .expect("evaluate default assembly")
}

fn solve(
    payload: &WeaponPayload,
    prices: &FxHashMap<String, i64>,
    constraints: Vec<Constraint>,
) -> SolveResult {
    let options = SolveOptions {
        mode: "operations".to_string(),
        laser_enabled: true,
        constraints,
        max_nodes: 2_000_000,
        max_leaves: 1_000_000,
        max_results: 3,
        ..SolveOptions::default()
    };
    Solver::new(payload, WEAPON, prices, &options).solve()
}

#[test]
fn default_panel_excludes_internal_parts_but_detail_keeps_them() {
    let payload = payload();
    let prices = prices();
    let metrics = evaluate(&payload, &prices, "operations");

    assert_eq!(metrics.panel["10006"], 48.0);
    assert_eq!(metrics.panel["10007"], 70.0);
    assert_eq!(metrics.panel["10005"], 24.0);
    assert_eq!(metrics.panel["10008"], 27.0);
    assert_eq!(metrics.attachment_price, 0);
    assert_eq!(metrics.total_price, 102000);

    // The default sight is internal: the site detail path still adds its
    // +6 stability and -8 handling.
    assert_eq!(metrics.detail.stability_score, 6.0);
    assert_eq!(metrics.detail.handling_score, -8.0);
}

#[test]
fn explicitly_selecting_the_default_part_matches_leaving_it_unset() {
    let payload = payload();
    let prices = prices();
    let assembler = Assembler::new(&payload, WEAPON);
    let implicit = assembler.resolve(&FxHashMap::default());

    let mut selections = FxHashMap::default();
    selections.insert(
        "weapon:18060000009/slot:11:Sco_point".to_string(),
        Some(INTERNAL_SIGHT.to_string()),
    );
    let explicit = assembler.resolve(&selections);

    let mut implicit_ids = implicit.installed_part_ids();
    let mut explicit_ids = explicit.installed_part_ids();
    implicit_ids.sort();
    explicit_ids.sort();
    assert_eq!(implicit_ids, explicit_ids);
    assert!(explicit
        .internal_parts
        .iter()
        .any(|part| part.part_id == INTERNAL_SIGHT));
    assert!(explicit
        .visible_slots
        .iter()
        .all(|slot| slot.selected_part_id.as_deref() != Some(INTERNAL_SIGHT)));

    let evaluate_with = |assembly: &gunsmith_mincost::assembly::Assembly| {
        let evaluator = Evaluator {
            payload: &payload,
            weapon_id: WEAPON,
            mode: "operations",
            laser_enabled: true,
            bipod_deployed: false,
            prices: &prices,
        };
        evaluator
            .evaluate(&assembler, assembly, &TuningState::default())
            .expect("evaluate assembly")
    };
    let implicit_metrics = evaluate_with(&implicit);
    let explicit_metrics = evaluate_with(&explicit);
    assert_eq!(implicit_metrics.panel, explicit_metrics.panel);
    assert_eq!(
        implicit_metrics.detail.stability_score,
        explicit_metrics.detail.stability_score
    );
    assert_eq!(implicit_metrics.total_price, explicit_metrics.total_price);
}

#[test]
fn normal_price_skips_internal_parts_while_blackhawk_keeps_them() {
    let payload = payload();
    // A synthetic single price source keeps the mode comparison independent of
    // the upstream blackhawk/market price split.
    let mut prices = FxHashMap::default();
    prices.insert("18060000009".to_string(), 100);
    prices.insert(INTERNAL_SIGHT.to_string(), 40);

    let operations = evaluate(&payload, &prices, "operations");
    assert_eq!(operations.attachment_price, 0);
    assert_eq!(operations.total_price, 100);
    assert_eq!(operations.panel["10006"], 48.0);
    assert_eq!(operations.panel["10007"], 70.0);

    let blackhawk = evaluate(&payload, &prices, "blackhawk");
    // `sn()` on the site walks non-virtual internal parts as well.
    assert_eq!(blackhawk.attachment_price, 40);
    assert_eq!(blackhawk.total_price, 140);
    // The panel still only counts visible selected parts.
    assert_eq!(blackhawk.panel["10006"], 48.0);
    assert_eq!(blackhawk.panel["10007"], 70.0);
}

fn panel_fixture() -> (WeaponPayload, FxHashMap<String, i64>) {
    let payload: WeaponPayload = serde_json::from_value(serde_json::json!({
        "parts": {
            "w1": { "id": "w1", "name": "W" },
            "p1": {
                "id": "p1",
                "name": "P1",
                "modifiersByMode": {
                    "operations": [
                        { "attributeId": "10001", "modifierType": "Initial", "param1": 40 },
                        { "attributeId": "10001", "modifierType": "Mult_A", "param1": 0.5 },
                        { "attributeId": "10001", "modifierType": "Addend", "delta": 20 }
                    ]
                },
                "baselineRecoveryModifiersByWeaponMode": {
                    "w1": {
                        "operations": [
                            { "attributeId": "10001", "modifierType": "Initial", "param1": 100 },
                            { "attributeId": "10001", "modifierType": "Mult_A", "param1": 0.25 },
                            { "attributeId": "10001", "modifierType": "Addend", "delta": 5 }
                        ]
                    }
                }
            }
        },
        "weaponStatsById": {
            "w1": {
                "byMode": {
                    "operations": {
                        "base": { "10001": 50 },
                        "display": { "10001": 50 }
                    }
                }
            }
        }
    }))
    .unwrap();
    (payload, FxHashMap::default())
}

#[test]
fn attribute_10001_initial_and_mult_a_ignore_baseline_lists() {
    let (payload, prices) = panel_fixture();
    let part = payload.parts.get("p1").unwrap();
    let evaluator = Evaluator {
        payload: &payload,
        weapon_id: "w1",
        mode: "operations",
        laser_enabled: true,
        bipod_deployed: false,
        prices: &prices,
    };
    let panel = evaluator.panel(&[part]);
    // Addends come from the normal-then-baseline list: 20 + 5.
    // `Initial` and `Mult_A` read only the part's own list: (40 + 25) * 1.5.
    assert_eq!(panel["10001"], 98.0);
}

#[test]
fn laser_initial_and_mult_a_follow_the_laser_rule() {
    let payload: WeaponPayload = serde_json::from_value(serde_json::json!({
        "parts": {
            "w1": { "id": "w1", "name": "W" },
            "L": {
                "id": "L",
                "name": "L",
                "uiEffectsByMode": { "operations": [{ "conditionId": "10038" }] },
                "modifiersByMode": {
                    "operations": [
                        { "attributeId": "10001", "modifierType": "Initial", "param1": 10, "polarity": "pro" },
                        { "attributeId": "10001", "modifierType": "Mult_A", "param1": 0.2, "polarity": "pro" },
                        { "attributeId": "10001", "modifierType": "Addend", "delta": 5, "polarity": "pro" }
                    ]
                }
            },
            "P": {
                "id": "P",
                "name": "P",
                "modifiersByMode": {
                    "operations": [
                        { "attributeId": "10001", "modifierType": "Initial", "param1": 40 },
                        { "attributeId": "10001", "modifierType": "Mult_A", "param1": 0.5 },
                        { "attributeId": "10001", "modifierType": "Addend", "delta": 20 }
                    ]
                }
            }
        },
        "weaponStatsById": {
            "w1": {
                "byMode": {
                    "operations": {
                        "base": { "10001": 50 },
                        "display": { "10001": 50 }
                    }
                }
            }
        }
    }))
    .unwrap();
    let prices = FxHashMap::default();
    let panel = |order: [&str; 2], laser: bool| {
        let parts: Vec<&gunsmith_mincost::model::Part> = order
            .iter()
            .map(|id| payload.parts.get(*id).unwrap())
            .collect();
        let evaluator = Evaluator {
            payload: &payload,
            weapon_id: "w1",
            mode: "operations",
            laser_enabled: laser,
            bipod_deployed: false,
            prices: &prices,
        };
        evaluator.panel(&parts)["10001"]
    };

    // The site ranks the single best laser over the panel list. With the laser
    // first it wins the pro slot: (last Initial 40 + addends 25) * (1 + 0.7).
    assert_eq!(panel(["L", "P"], true), 111.0);
    // Lasers off keep only the con side, so the pro Initial/Mult_A drop out.
    assert_eq!(panel(["L", "P"], false), 90.0);
    // Reversing the list moves the laser index: the last Initial is now 10.
    assert_eq!(panel(["P", "L"], true), 60.0);
    assert_eq!(panel(["P", "L"], false), 90.0);
}

#[test]
fn handling_max_constraint_returns_a_build_that_really_satisfies_it() {
    let payload = payload();
    let prices = prices();
    let out = solve(
        &payload,
        &prices,
        vec![Constraint::new("10006", None, Some(44.0))],
    );

    assert!(out.optimal);
    let best = out.best.expect("feasible 10006 max 44 build");
    assert!(best.panel["10006"] <= 44.0 + 1e-9);
    // The internal default sight is installed in this build. Its +6 must not
    // leak into the panel, otherwise the solver reports an impossible 76.
    assert_eq!(best.panel["10007"], 70.0);
    assert!(best
        .installed_part_keys
        .iter()
        .any(|key| key.ends_with(INTERNAL_SIGHT)));
}

#[test]
fn stability_min_constraint_counts_only_visible_parts() {
    let payload = payload();
    let prices = prices();
    let out = solve(
        &payload,
        &prices,
        vec![Constraint::new("10007", Some(74.0), None)],
    );

    assert!(out.optimal);
    let best = out.best.expect("feasible 10007 min 74 build");
    assert!(best.panel["10007"] >= 74.0 - 1e-9);
    // 70 base + 5 from an explicitly selected muzzle. The internal default
    // sight has +6 in the detail channel but must not be what satisfies the
    // panel constraint.
    assert_eq!(best.panel["10007"], 75.0);
    assert!(best.selections.values().any(|part| part.is_some()));
}

fn stability_constraint() -> Vec<Constraint> {
    vec![Constraint::new("stability-fire-camera", None, Some(0.5))]
}

#[test]
fn stability_reduction_keeps_the_internal_stability_offset() {
    let payload = payload();
    let constraints = stability_constraint();
    let reduced = reduce_stability_constraints(&payload, WEAPON, "operations", &constraints);
    assert_eq!(reduced.len(), 1);
    assert_eq!(reduced[0].id, "10007");
    let with_internal = reduced[0].min.expect("lower-bound reduction");
    assert!(reduced[0].max.is_none());

    // Reducing the same payload without the internal addend gives the exact
    // panel bound. The internal +6 can only be installed behind the panel, so
    // it must widen the mapped bound by the internal bracket (counted twice,
    // once for the internal placement and once for a possible visible one).
    let mut visible_only = payload.clone();
    let part = visible_only.parts.get_mut(INTERNAL_SIGHT).unwrap();
    for list in part.display_mods.values_mut() {
        list.retain(|m| m.attribute_id.as_deref() != Some("10007"));
    }
    let exact = reduce_stability_constraints(&visible_only, WEAPON, "operations", &constraints);
    let exact_min = exact[0].min.expect("exact reduction");
    assert!((exact_min - with_internal - 12.0).abs() < 1e-6);
    assert!(with_internal < exact_min);
}

#[test]
fn stability_reduction_widens_the_panel_upper_bound_for_a_negative_internal_delta() {
    let mut payload = payload();
    let part = payload.parts.get_mut(INTERNAL_SIGHT).unwrap();
    for list in part.display_mods.values_mut() {
        for modifier in list.iter_mut() {
            if modifier.attribute_id.as_deref() == Some("10007")
                && modifier.modifier_type == "Addend"
            {
                modifier.delta = Some(Num(-6.0));
            }
        }
    }
    let constraints = vec![Constraint::new("stability-fire-camera", Some(0.5), None)];
    let reduced = reduce_stability_constraints(&payload, WEAPON, "operations", &constraints);
    assert_eq!(reduced.len(), 1);
    assert_eq!(reduced[0].id, "10007");
    let with_internal = reduced[0].max.expect("upper-bound reduction");

    let mut visible_only = payload.clone();
    let part = visible_only.parts.get_mut(INTERNAL_SIGHT).unwrap();
    for list in part.display_mods.values_mut() {
        list.retain(|m| m.attribute_id.as_deref() != Some("10007"));
    }
    let exact = reduce_stability_constraints(&visible_only, WEAPON, "operations", &constraints);
    let exact_max = exact[0].max.expect("exact reduction");
    // A negative internal addend can only pull the panel bound down, so the
    // mapped upper bound must sit 2 * 6 above the exact one.
    assert!((with_internal - exact_max - 12.0).abs() < 1e-6);
}

#[test]
fn stability_max_constraint_keeps_the_internal_part_solution() {
    let payload = payload();
    let prices = prices();
    let out = solve(
        &payload,
        &prices,
        vec![Constraint::new("stability-fire-camera", None, Some(0.5))],
    );

    assert!(out.optimal);
    let best = out.best.expect("feasible fire-camera build");
    assert!(best.detail.fire_camera.unwrap() <= 0.5 + 1e-9);
    // The stock build only clears the bound because the default sight adds
    // stability behind the panel; mapping the constraint to the panel without
    // an offset would prune it.
    assert_eq!(best.total_price, 102000);
    assert!(best
        .installed_part_keys
        .iter()
        .any(|key| key.ends_with(INTERNAL_SIGHT)));
}

#[test]
fn stability_min_constraint_keeps_the_reduced_search_path() {
    let payload = payload();
    let prices = prices();
    // `stability-fire-camera` falls as stability rises, so a min bound maps to
    // a panel upper bound. With only positive internal stability the mapped
    // bound stays valid and must keep the leaf DP instead of falling back to
    // full enumeration.
    let out = solve(
        &payload,
        &prices,
        vec![Constraint::new("stability-fire-camera", Some(0.5), None)],
    );

    assert!(out.optimal);
    let best = out.best.expect("feasible fire-camera build");
    assert!(best.detail.fire_camera.unwrap() >= 0.5 - 1e-9);
}

#[test]
fn stability_reduction_still_applies_without_internal_stability_modifiers() {
    let mut payload = payload();
    let part = payload.parts.get_mut(INTERNAL_SIGHT).unwrap();
    for list in part.display_mods.values_mut() {
        list.retain(|m| m.attribute_id.as_deref() != Some("10007"));
    }

    let constraints = stability_constraint();
    let reduced = reduce_stability_constraints(&payload, WEAPON, "operations", &constraints);
    assert_eq!(reduced.len(), 1);
    assert_eq!(reduced[0].id, "10007");
    assert!(reduced[0].min.is_some());
}
