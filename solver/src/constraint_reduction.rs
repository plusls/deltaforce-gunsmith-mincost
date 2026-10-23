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
use crate::model::{Curve, NumericProfile, Part, WeaponPayload};
use crate::search::Constraint;

pub const REDUCIBLE_STABILITY_METRICS: [&str; 5] = [
    "stability-fire-camera",
    "stability-movement-camera",
    "stability-hold-breath-recoil",
    "stability-breath-range",
    "stability-breath-duration",
];

const STABILITY_PANEL_ATTR: &str = "10007";
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
    let Some((internal_min, internal_max)) = stability_internal_offset(payload, weapon_id, mode)
    else {
        return constraints.to_vec();
    };
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
                        // `dS = panel[10007] - base + internal`, so a lower
                        // bound on `dS` only proves a lower bound on the panel
                        // once the largest possible internal offset is
                        // removed; a upper bound on `dS` removes the smallest
                        // one. Both keep the mapped constraint necessary for
                        // the original one.
                        let panel_min = min.map(|v| base + v - internal_max - BOUND_EPS);
                        let panel_max = max.map(|v| base + v - internal_min + BOUND_EPS);
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
    let curve = stability_curve_for_metric(profile, metric)?;
    let points = critical_points(curve, domain_lo, domain_hi);
    let dir = classify_direction(&f, &points)?;

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

fn stability_curve_for_metric<'a>(profile: &'a NumericProfile, metric: &str) -> Option<&'a Curve> {
    let curves = &profile.linked_tables.stability_curves;
    match metric {
        "stability-fire-camera" => curves.fire_camera_stability.as_ref(),
        "stability-movement-camera" => curves.movement_camera_stability.as_ref(),
        "stability-hold-breath-recoil" => curves.hold_breath_recoil.as_ref(),
        "stability-breath-range" => curves.breath_scale.as_ref(),
        "stability-breath-duration" => curves.breath_time.as_ref(),
        _ => None,
    }
}

/// Every point where the composed metric can change direction: curve keys,
/// interior extrema of cubic segments, segment midpoints and both sides of a
/// key. Between two consecutive points the metric is monotone, so checking
/// these points decides the direction exactly. Uniform sampling can step over
/// a non-monotone feature that is narrower than the sample spacing.
fn critical_points(curve: &Curve, lo: f64, hi: f64) -> Vec<f64> {
    let mut points = vec![lo, hi];
    let keys = &curve.keys;
    if keys.is_empty() {
        return points;
    }
    let span = (hi - lo).abs().max(1.0);
    let eps = span * 1e-9;
    for key in keys {
        points.push(key.time);
        points.push(key.time - eps);
        points.push(key.time + eps);
    }
    for pair in keys.windows(2) {
        let a = &pair[0];
        let b = &pair[1];
        let dt = b.time - a.time;
        if dt <= 0.0 {
            continue;
        }
        points.push(a.time + dt * 0.5);
        if a.interp_mode == "RCIM_Cubic" {
            // p'(t) = A t^2 + B t + C with
            // A = 6(v0-v1) + 3 dt (m0+m1)
            // B = 6(v1-v0) - 4 dt m0 - 2 dt m1
            // C = dt m0
            let v0 = a.value;
            let v1 = b.value;
            let m0 = a.leave_tangent;
            let m1 = b.arrive_tangent;
            let qa = 6.0 * (v0 - v1) + 3.0 * dt * (m0 + m1);
            let qb = 6.0 * (v1 - v0) - 4.0 * dt * m0 - 2.0 * dt * m1;
            let qc = dt * m0;
            let mut roots = [f64::NAN; 2];
            let mut count = 0;
            if qa.abs() <= f64::EPSILON {
                if qb.abs() > f64::EPSILON {
                    roots[0] = -qc / qb;
                    count = 1;
                }
            } else {
                let disc = qb * qb - 4.0 * qa * qc;
                if disc >= 0.0 {
                    let root = disc.sqrt();
                    roots[0] = (-qb - root) / (2.0 * qa);
                    roots[1] = (-qb + root) / (2.0 * qa);
                    count = 2;
                }
            }
            for root in roots.into_iter().take(count) {
                if root > 0.0 && root < 1.0 {
                    points.push(a.time + dt * root);
                }
            }
        }
    }
    points.retain(|x| x.is_finite() && *x >= lo && *x <= hi);
    points.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    points.dedup_by(|a, b| *a == *b);
    points
}

fn classify_direction(f: &impl Fn(f64) -> Option<f64>, points: &[f64]) -> Option<Direction> {
    let mut prev = f(*points.first()?)?;
    if !prev.is_finite() {
        return None;
    }
    let mut increasing = true;
    let mut decreasing = true;
    for &x in &points[1..] {
        let value = f(x)?;
        if !value.is_finite() {
            return None;
        }
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

/// `dS` sums display stability addends over the visible parts and the
/// internal parts, while the panel only counts visible ones. The relation is
/// `dS = panel[10007] - base + internal`, so an exact reduction needs a
/// bracket on `internal`.
///
/// Every stability addend of a part that can never be internal must be an
/// unconditional `10007` addend, otherwise it would move `dS` without moving
/// the panel by the same amount (returns `None`). Parts the assembler can
/// install behind the panel keep their addends in the bracket instead.
///
/// A part can be installed internally more than once: every default preset
/// node with its own `pathKey` is a separate placement, and virtual rules add
/// more. Each internal placement contributes the addend to `dS` without
/// touching the panel. A visible placement of a `10007` addend cancels against
/// the panel and must not be counted; a visible placement of any other
/// stability addend moves `dS` without the panel and adds one more copy.
/// Conditional addends are treated as always active on the high side and as
/// active when negative on the low side, which can only widen the bracket.
/// Only the display channel matters; `dS` never reads the main attribute list.
fn stability_internal_offset(
    payload: &WeaponPayload,
    weapon_id: &str,
    mode: &str,
) -> Option<(f64, f64)> {
    let placements = placement_counts(payload, weapon_id, mode)?;
    let mut lo = 0.0;
    let mut hi = 0.0;
    for part in payload.parts.values() {
        let Some(&internal) = placements.internal.get(part.id.as_str()) else {
            for modifier in part.display_mods_for(weapon_id, mode) {
                if modifier.modifier_type != "Addend" {
                    continue;
                }
                let attr = modifier.attribute_id.as_deref().unwrap_or("");
                if !STABILITY_ATTRS.contains(&attr) {
                    continue;
                }
                if attr != STABILITY_PANEL_ATTR || modifier.condition_id.is_some() {
                    return None;
                }
            }
            continue;
        };
        let visible = placements
            .visible
            .get(part.id.as_str())
            .copied()
            .unwrap_or(0);
        for modifier in part.display_mods_for(weapon_id, mode) {
            if modifier.modifier_type != "Addend" {
                continue;
            }
            let attr = modifier.attribute_id.as_deref().unwrap_or("");
            if !STABILITY_ATTRS.contains(&attr) {
                continue;
            }
            let copies = internal
                + if attr == STABILITY_PANEL_ATTR {
                    0
                } else {
                    visible
                };
            let copies = copies as f64;
            let delta = modifier.delta.map(|v| v.0).unwrap_or(0.0);
            lo += copies * delta.min(0.0);
            hi += copies * delta.max(0.0);
        }
    }
    Some((lo, hi))
}

#[derive(Default)]
struct PlacementCounts<'a> {
    /// Internal placements per part: default preset path keys plus virtual
    /// rule socket groups.
    internal: rustc_hash::FxHashMap<&'a str, usize>,
    /// Visible slots whose candidate list contains the part.
    visible: rustc_hash::FxHashMap<&'a str, usize>,
}

/// Placements through which the assembler can install each part.
///
/// Default preset nodes are keyed by `pathKey` in the assembler's defaults map
/// and each tree position is visited at most once, so distinct path keys are
/// distinct placements. Virtual rules are grouped by socket id: at most one
/// rule per socket can fire. Their part still cannot be bounded by the rule
/// count alone because the assembler applies the selected rule at every
/// visited slot with that id, including slots under a host it installs more
/// than once, so virtual parts carrying stability addends decline the
/// reduction. The visible count uses every static slot candidate list, which
/// overcounts unreachable hosts and therefore widens the bracket.
fn placement_counts<'a>(
    payload: &'a WeaponPayload,
    weapon_id: &str,
    mode: &str,
) -> Option<PlacementCounts<'a>> {
    let mut out = PlacementCounts::default();
    if let Some(preset) = payload.default_presets.get(weapon_id) {
        let mut paths: rustc_hash::FxHashMap<&'a str, rustc_hash::FxHashSet<&'a str>> =
            rustc_hash::FxHashMap::default();
        for node in &preset.nodes {
            if node.is_root || node.is_virtual {
                continue;
            }
            // The assembler only installs defaults that carry a path key.
            let Some(path) = node.path_key.as_deref() else {
                continue;
            };
            let Some(part) = payload.parts.get(&node.part_id) else {
                continue;
            };
            paths.entry(part.id.as_str()).or_default().insert(path);
        }
        for (id, paths) in paths {
            out.internal.insert(id, paths.len());
        }
    }
    if let Some(rules) = payload.virtual_rules.get(weapon_id) {
        let mut groups: rustc_hash::FxHashMap<&'a str, rustc_hash::FxHashSet<&'a str>> =
            rustc_hash::FxHashMap::default();
        for rule in rules {
            let Some(part) = payload.parts.get(&rule.virtual_part_id) else {
                continue;
            };
            if has_stability_addend(part, weapon_id, mode) {
                return None;
            }
            groups
                .entry(part.id.as_str())
                .or_default()
                .insert(rule.socket_id.as_str());
        }
        for (id, sockets) in groups {
            *out.internal.entry(id).or_default() += sockets.len();
        }
    }
    for slots in payload.slots_by_host.values() {
        for slot in slots {
            for id in &slot.candidate_ids {
                let Some(part) = payload.parts.get(id) else {
                    continue;
                };
                *out.visible.entry(part.id.as_str()).or_default() += 1;
            }
        }
    }
    Some(out)
}

/// Whether a part can move `dS` at all. Conditional addends never reach the
/// stability detail sum, so they do not count.
fn has_stability_addend(part: &Part, weapon_id: &str, mode: &str) -> bool {
    part.display_mods_for(weapon_id, mode).any(|modifier| {
        modifier.modifier_type == "Addend"
            && modifier.condition_id.is_none()
            && modifier.delta.map(|v| v.0 != 0.0).unwrap_or(false)
            && STABILITY_ATTRS.contains(&modifier.attribute_id.as_deref().unwrap_or(""))
    })
}

fn stability_base(payload: &WeaponPayload, weapon_id: &str, mode: &str) -> Option<f64> {
    payload
        .weapon_stats
        .get(weapon_id)?
        .display_stat(mode, STABILITY_PANEL_ATTR)
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
