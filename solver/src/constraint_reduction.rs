//! Reduces monotone stability detail metrics to bounds on the stability score.
//!
//! Several stability metrics are produced by evaluating a curve at `dS`, the
//! sum of the stability attribute addends (`STABILITY_ATTRS`). When every such
//! addend for a weapon comes from the panel attribute `10007`, `dS` is exactly
//! `panel[10007] - base[10007]`, which the leaf DP already tracks. A single
//! sided bound on such a monotone curve is therefore equivalent to a bound on
//! `10007` and no longer forces the solver off the Pareto DP path.
//!
//! The reduction is deliberately conservative: anything that cannot be proven
//! equivalent at the payload level is left untouched and follows the original
//! full enumeration path. The original constraints are always kept for the
//! final exact evaluation, so a wrong reduction could only cost performance,
//! never correctness of the reported build.

use crate::metrics::{stability_metric_at, STABILITY_ATTRS};
use crate::model::WeaponPayload;
use crate::search::Constraint;

pub const REDUCIBLE_STABILITY_METRICS: [&str; 5] = [
    "stability-fire-camera",
    "stability-movement-camera",
    "stability-hold-breath-recoil",
    "stability-breath-range",
    "stability-breath-duration",
];

const STABILITY_PANEL_ATTR: &str = "10007";
const SAMPLES: usize = 8192;
const BISECT_STEPS: usize = 128;
const BOUND_EPS: f64 = 1e-7;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Direction {
    Increasing,
    Decreasing,
    Constant,
}

#[derive(Debug, Clone, Copy)]
enum Reduction {
    /// The bound holds for every possible stability score.
    Always,
    /// The bound cannot hold for any stability score.
    Never,
    /// Equivalent single sided bound on the raw stability score `dS`.
    Bound { min: Option<f64>, max: Option<f64> },
}

/// Rewrites constraints used by the search. Constraints that cannot be reduced
/// are returned unchanged.
pub fn reduce_stability_constraints(
    payload: &WeaponPayload,
    weapon_id: &str,
    mode: &str,
    constraints: &[Constraint],
) -> Vec<Constraint> {
    let has_candidate = constraints.iter().any(|c| {
        REDUCIBLE_STABILITY_METRICS.contains(&c.id.as_str()) && (c.min.is_some() != c.max.is_some())
    });
    if !has_candidate {
        return constraints.to_vec();
    }
    let Some(profile) = payload.numeric_profiles.get(weapon_id) else {
        return constraints.to_vec();
    };
    if !stability_delta_is_panel_only(payload, mode) {
        return constraints.to_vec();
    }
    let Some(base) = stability_base(payload, weapon_id, mode) else {
        return constraints.to_vec();
    };
    let Some((domain_lo, domain_hi)) = stability_curve_domain(profile) else {
        return constraints.to_vec();
    };

    let mut out: Vec<Constraint> = Vec::with_capacity(constraints.len());
    for c in constraints {
        let reducible = REDUCIBLE_STABILITY_METRICS.contains(&c.id.as_str())
            && (c.min.is_some() != c.max.is_some())
            && metric_extra_inputs_neutral(payload, mode, &c.id);
        if reducible {
            if let Some(reduction) = reduce_one(profile, c, domain_lo, domain_hi) {
                match reduction {
                    Reduction::Always => continue,
                    Reduction::Never => {}
                    Reduction::Bound { min, max } => {
                        let panel_min = min.map(|v| base + v - BOUND_EPS);
                        let panel_max = max.map(|v| base + v + BOUND_EPS);
                        merge_stability_bound(&mut out, panel_min, panel_max);
                        continue;
                    }
                }
            }
        }
        out.push(c.clone());
    }
    out
}

fn merge_stability_bound(out: &mut Vec<Constraint>, min: Option<f64>, max: Option<f64>) {
    if let Some(existing) = out.iter_mut().find(|c| c.id == STABILITY_PANEL_ATTR) {
        if let Some(value) = min {
            existing.min = Some(existing.min.map_or(value, |current| current.max(value)));
        }
        if let Some(value) = max {
            existing.max = Some(existing.max.map_or(value, |current| current.min(value)));
        }
        return;
    }
    let mapped = Constraint::new(STABILITY_PANEL_ATTR, min, max);
    out.push(mapped);
}

fn reduce_one(
    profile: &crate::model::NumericProfile,
    c: &Constraint,
    domain_lo: f64,
    domain_hi: f64,
) -> Option<Reduction> {
    let metric = c.id.as_str();
    let f = |x: f64| stability_metric_at(profile, metric, x);
    let dir = classify_direction(&f, domain_lo, domain_hi)?;

    if let Some(max) = c.max {
        let sat = |v: f64| v <= max + 1e-9;
        return Some(match dir {
            Direction::Constant => {
                if sat(f(domain_lo)?) {
                    Reduction::Always
                } else {
                    Reduction::Never
                }
            }
            Direction::Decreasing => {
                if sat(f(domain_lo)?) {
                    Reduction::Always
                } else if !sat(f(domain_hi)?) {
                    Reduction::Never
                } else {
                    Reduction::Bound {
                        min: Some(bisect(&f, domain_lo, domain_hi, true, &sat)?),
                        max: None,
                    }
                }
            }
            Direction::Increasing => {
                if sat(f(domain_hi)?) {
                    Reduction::Always
                } else if !sat(f(domain_lo)?) {
                    Reduction::Never
                } else {
                    Reduction::Bound {
                        min: None,
                        max: Some(bisect(&f, domain_lo, domain_hi, false, &sat)?),
                    }
                }
            }
        });
    }

    if let Some(min) = c.min {
        let sat = |v: f64| v >= min - 1e-9;
        return Some(match dir {
            Direction::Constant => {
                if sat(f(domain_lo)?) {
                    Reduction::Always
                } else {
                    Reduction::Never
                }
            }
            Direction::Decreasing => {
                if sat(f(domain_hi)?) {
                    Reduction::Always
                } else if !sat(f(domain_lo)?) {
                    Reduction::Never
                } else {
                    Reduction::Bound {
                        min: None,
                        max: Some(bisect(&f, domain_lo, domain_hi, false, &sat)?),
                    }
                }
            }
            Direction::Increasing => {
                if sat(f(domain_lo)?) {
                    Reduction::Always
                } else if !sat(f(domain_hi)?) {
                    Reduction::Never
                } else {
                    Reduction::Bound {
                        min: Some(bisect(&f, domain_lo, domain_hi, true, &sat)?),
                        max: None,
                    }
                }
            }
        });
    }

    None
}

/// Finds the boundary between the unsatisfied and satisfied halves of a
/// monotone predicate. `lowest` selects the smallest satisfying point.
fn bisect(
    f: &impl Fn(f64) -> Option<f64>,
    lo: f64,
    hi: f64,
    lowest: bool,
    sat: &impl Fn(f64) -> bool,
) -> Option<f64> {
    let mut a = lo;
    let mut b = hi;
    for _ in 0..BISECT_STEPS {
        let mid = (a + b) * 0.5;
        let value = f(mid)?;
        if sat(value) {
            if lowest {
                b = mid;
            } else {
                a = mid;
            }
        } else if lowest {
            a = mid;
        } else {
            b = mid;
        }
    }
    Some(if lowest { b } else { a })
}

fn classify_direction(f: &impl Fn(f64) -> Option<f64>, lo: f64, hi: f64) -> Option<Direction> {
    if !(hi > lo) {
        return None;
    }
    let mut prev = f(lo)?;
    let mut increasing = true;
    let mut decreasing = true;
    for index in 1..=SAMPLES {
        let x = lo + (hi - lo) * (index as f64) / (SAMPLES as f64);
        let value = f(x)?;
        if value < prev - 1e-12 {
            increasing = false;
        }
        if value > prev + 1e-12 {
            decreasing = false;
        }
        if !increasing && !decreasing {
            return None;
        }
        prev = value;
    }
    Some(if increasing && decreasing {
        Direction::Constant
    } else if increasing {
        Direction::Increasing
    } else {
        Direction::Decreasing
    })
}

/// `dS` must come exclusively from unconditional `10007` addends, otherwise
/// the panel score cannot stand in for it.
fn stability_delta_is_panel_only(payload: &WeaponPayload, mode: &str) -> bool {
    for part in payload.parts.values() {
        let Some(list) = part.display_mods.get(mode) else {
            continue;
        };
        for modifier in list {
            if modifier.modifier_type != "Addend" {
                continue;
            }
            let attr = modifier.attribute_id.as_deref().unwrap_or("");
            if !STABILITY_ATTRS.contains(&attr) {
                continue;
            }
            if attr != STABILITY_PANEL_ATTR || modifier.condition_id.is_some() {
                return false;
            }
        }
    }
    true
}

fn stability_base(payload: &WeaponPayload, weapon_id: &str, mode: &str) -> Option<f64> {
    let stats = payload.weapon_stats.get(weapon_id)?.by_mode.get(mode)?;
    stats
        .display
        .get(STABILITY_PANEL_ATTR)
        .copied()
        .or_else(|| stats.base.get(STABILITY_PANEL_ATTR).copied())
}

fn stability_curve_domain(profile: &crate::model::NumericProfile) -> Option<(f64, f64)> {
    let curves = &profile.linked_tables.stability_curves;
    let mut lo = f64::INFINITY;
    let mut hi = f64::NEG_INFINITY;
    for curve in [
        &curves.breath_scale,
        &curves.breath_time,
        &curves.fire_camera_stability,
        &curves.movement_camera_stability,
        &curves.hold_breath_recoil,
    ]
    .into_iter()
    .flatten()
    {
        let first = curve.keys.first()?;
        let last = curve.keys.last()?;
        lo = lo.min(first.time);
        hi = hi.max(last.time);
    }
    if lo.is_finite() && hi.is_finite() && hi > lo {
        Some((lo, hi))
    } else {
        None
    }
}

/// Metrics that divide or multiply by a value changed by tuning or by parts
/// cannot be reduced to `dS` alone unless that value is provably constant.
fn metric_extra_inputs_neutral(payload: &WeaponPayload, mode: &str, id: &str) -> bool {
    match id {
        "stability-fire-camera" | "stability-hold-breath-recoil" => true,
        "stability-movement-camera" => !tuning_effects_contain(payload, "移动时据枪稳定性"),
        "stability-breath-range" => {
            !tuning_effects_contain(payload, "瞄准时呼吸稳定性")
                && !has_breath_scale_multiplier(payload, mode)
        }
        "stability-breath-duration" => !tuning_effects_contain(payload, "瞄准时呼吸稳定性"),
        _ => false,
    }
}

fn tuning_effects_contain(payload: &WeaponPayload, needle: &str) -> bool {
    payload.parts.values().any(|part| {
        part.tuning_options.iter().any(|tune| {
            tune.display_effects
                .iter()
                .any(|effect| effect.attribute_name_snapshot.contains(needle))
        })
    })
}

fn has_breath_scale_multiplier(payload: &WeaponPayload, mode: &str) -> bool {
    payload.parts.values().any(|part| {
        part.raw_mods
            .get(mode)
            .map(|list| {
                list.iter().any(|modifier| {
                    modifier.modifier_type == "Mult_C"
                        && modifier.target_path.ends_with(".StandBreathFactor")
                })
            })
            .unwrap_or(false)
    })
}
