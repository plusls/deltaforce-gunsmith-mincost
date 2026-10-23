use crate::model::{CurveKey, Part, TuningOption};

pub fn tuning_available(slot_name: &str, part: &Part) -> bool {
    part.tuning_socket_availability
        .as_ref()
        .and_then(|m| m.get(slot_name))
        .copied()
        .unwrap_or(slot_name != "StoKit_point")
}

pub fn quantize(value: f64, min: f64, max: f64, step: f64) -> f64 {
    let decimals = decimals(min).max(decimals(max)).max(decimals(step));
    quantize_with_decimals(value, min, max, step, decimals)
}

fn quantize_with_decimals(value: f64, min: f64, max: f64, step: f64, decimals: u32) -> f64 {
    let clamped = value.clamp(min, max);
    if step <= 0.0 {
        return clamped;
    }
    let n = ((clamped - min) / step).round();
    let raw = min + n * step;
    let clamped = raw.clamp(min, max);
    round_to(clamped, decimals)
}

/// Maximum decimal places used when quantizing tuning values. Anything that
/// needs more cannot be represented exactly and is reported as truncated.
const MAX_DECIMALS: u32 = 15;

/// Decimal places needed to write `value` (up to float noise), or `None` when
/// it needs more than [`MAX_DECIMALS`].
fn decimals_opt(value: f64) -> Option<u32> {
    if !value.is_finite() {
        return Some(0);
    }
    let mut v = value.abs();
    if v == 0.0 {
        return Some(0);
    }
    for digits in 0..MAX_DECIMALS {
        // The tolerance is relative to `v`, so tiny values such as 1e-9 keep
        // their own scale instead of being treated as integers.
        if (v - v.round()).abs() <= 32.0 * f64::EPSILON * v {
            return Some(digits);
        }
        v *= 10.0;
    }
    if (v - v.round()).abs() <= 32.0 * f64::EPSILON * v {
        Some(MAX_DECIMALS)
    } else {
        None
    }
}

fn decimals(value: f64) -> u32 {
    decimals_opt(value).unwrap_or(MAX_DECIMALS)
}

fn round_to(value: f64, decimals: u32) -> f64 {
    let factor = 10f64.powi(decimals.min(15) as i32);
    (value * factor).round() / factor
}

fn points_are_sorted(points: &[CurveKey]) -> bool {
    points.windows(2).all(|pair| {
        pair[0].time < pair[1].time
            || (pair[0].time == pair[1].time && pair[0].key_index <= pair[1].key_index)
    })
}

fn eval_sorted_curve_points(points: &[CurveKey], x: f64) -> Option<f64> {
    let first = &points[0];
    if x <= first.time {
        return Some(first.value);
    }
    let last = &points[points.len() - 1];
    if x >= last.time {
        return Some(last.value);
    }
    for i in 0..points.len() - 1 {
        let a = &points[i];
        let b = &points[i + 1];
        if x > b.time {
            continue;
        }
        if b.time == a.time {
            return Some(b.value);
        }
        let t = (x - a.time) / (b.time - a.time);
        // The site's tuning evaluator (`mr`) ignores InterpMode entirely and
        // always interpolates linearly.
        return Some(a.value + (b.value - a.value) * t);
    }
    Some(last.value)
}

pub fn eval_curve_points(points: &[CurveKey], x: f64) -> Option<f64> {
    if points.is_empty() || !x.is_finite() {
        return None;
    }
    if points_are_sorted(points) {
        return eval_sorted_curve_points(points, x);
    }
    let mut keys: Vec<&CurveKey> = points.iter().collect();
    // Tuning curves use the site's `mr`, which does tie-break equal times by
    // key_index (unlike the curve normalization used by profile curves).
    keys.sort_by(|a, b| {
        a.time
            .partial_cmp(&b.time)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.key_index.cmp(&b.key_index))
    });
    let first = keys[0];
    if x <= first.time {
        return Some(first.value);
    }
    let last = keys[keys.len() - 1];
    if x >= last.time {
        return Some(last.value);
    }
    for i in 0..keys.len() - 1 {
        let a = keys[i];
        let b = keys[i + 1];
        if x > b.time {
            continue;
        }
        if b.time == a.time {
            return Some(b.value);
        }
        let t = (x - a.time) / (b.time - a.time);
        // The site's tuning evaluator (`mr`) ignores InterpMode entirely and
        // always interpolates linearly.
        return Some(a.value + (b.value - a.value) * t);
    }
    Some(last.value)
}

#[derive(Debug, Clone, Copy, Default)]
pub struct TuneChannels {
    pub h: f64,
    pub v: f64,
    pub k: f64,
    pub open: f64,
    pub move_speed: f64,
    pub move_runtime: f64,
    pub breath: f64,
    pub move_stability: f64,
}

pub fn tune_channels(tune: &TuningOption, value: f64) -> TuneChannels {
    let mut out = TuneChannels::default();
    for effect in &tune.display_effects {
        let Some(v) = eval_curve_points(&effect.curve_points, value) else {
            continue;
        };
        match effect.attribute_name_snapshot.as_str() {
            "%额外后坐力控制" => {
                if runtime_target_allowed(tune, "GRecoil_H") {
                    out.h += v;
                }
                if runtime_target_allowed(tune, "GRecoil_V") {
                    out.v += v;
                }
            }
            "%开火时武器稳定性" => {
                if runtime_target_allowed(tune, "GGunkickSpring") {
                    out.k += v;
                }
            }
            name if name.contains("移动速度") => {
                out.move_speed += v;
                if runtime_target_allowed(tune, "GMovement_ADSSpeed") {
                    out.move_runtime += v / 100.0;
                }
            }
            name if name.contains("开镜速度")
                || name.contains("举镜瞄准速度")
                || name.contains("举枪瞄准速度") =>
            {
                out.open += v;
            }
            name if name.contains("瞄准时呼吸稳定性") => out.breath += v,
            name if name.contains("移动时据枪稳定性") => out.move_stability += v,
            _ => {}
        }
    }
    out
}

pub fn tune_offer(tune: &TuningOption, value: f64) -> (f64, f64, f64) {
    let channels = tune_channels(tune, value);
    (channels.h, channels.v, channels.k)
}

/// Upper bound on the number of evenly spaced tuning values. A grid above
/// this only happens for payloads far outside the site's range; it is sampled
/// and reported as truncated so the search cannot claim completeness.
const TUNE_GRID_LIMIT: usize = 65_536;

/// Returns the tuning values worth evaluating and whether the step grid had to
/// be sampled. Every grid value matters: a constraint can have a feasible band
/// narrower than one step, so dropping interior values silently turns a
/// feasible build into "no build".
///
/// The returned set defines the scope of `optimal = true`: it only means best
/// within these discrete tuning candidates, never over a continuous range.
/// `step > 0` enumerates the full `[min, max]` grid; `step <= 0` (free tuning)
/// uses `{min, max, default} ∪ {curve key Time}`, clamped to `[min, max]` and
/// deduped. The site optimizer only tries `default` for free tuning, so this
/// set is a superset of the site's. `truncated` covers every case where the
/// grid cannot be enumerated completely: an oversized grid, an unrepresentable
/// precision, or a step absorbed by floating point.
///
/// This bool is internal to the tuning enumeration. Search maps it to
/// `heuristic_incomplete`/`tune_incomplete` so the result cannot claim
/// optimality; it does not set the public `SolveResult.truncated`, which only
/// reports deadline or node/leaf budget exhaustion.
pub fn tune_offer_values(tune: &TuningOption) -> (Vec<f64>, bool) {
    let mut values = vec![tune.min_value, tune.max_value, tune.default_value];
    for effect in &tune.display_effects {
        for key in &effect.curve_points {
            values.push(key.time);
        }
    }
    let step = tune.step_value;
    let mut truncated = false;
    if step > 0.0 {
        let count = ((tune.max_value - tune.min_value) / step).floor() + 1.0;
        if count.is_finite() && count > 0.0 {
            if count <= TUNE_GRID_LIMIT as f64 {
                let mut v = tune.min_value;
                while v <= tune.max_value + step * 0.5 {
                    values.push(v);
                    let next = v + step;
                    if !next.is_finite() || next == v {
                        // The step is absorbed by floating point at this
                        // magnitude, so the grid cannot be enumerated and must
                        // not be reported as complete.
                        truncated = true;
                        break;
                    }
                    v = next;
                }
            } else {
                truncated = true;
                let count = count.min(usize::MAX as f64) as usize;
                let stride = count.div_ceil(TUNE_GRID_LIMIT);
                for i in (0..count).step_by(stride) {
                    let v = tune.min_value + i as f64 * step;
                    if v <= tune.max_value {
                        values.push(v);
                    }
                }
                values.push(tune.max_value);
            }
        } else if !count.is_finite() {
            truncated = true;
        }
    }
    let precision_representable = decimals_opt(tune.min_value).is_some()
        && decimals_opt(tune.max_value).is_some()
        && decimals_opt(tune.step_value).is_some();
    let decimals = decimals(tune.min_value)
        .max(decimals(tune.max_value))
        .max(decimals(tune.step_value));
    if !precision_representable {
        // The grid cannot be represented at 15 decimal places, so its
        // quantization would silently merge or move candidates.
        truncated = true;
    }
    let mut out: Vec<f64> = values
        .into_iter()
        .map(|v| {
            quantize_with_decimals(v, tune.min_value, tune.max_value, tune.step_value, decimals)
        })
        .collect();
    out.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    // Keep the dedup tolerance strictly below the effective step, otherwise
    // adjacent grid values are merged away.
    let dedup_tolerance = if step > 0.0 && step.is_finite() {
        step * 1e-6
    } else {
        1e-12
    };
    out.dedup_by(|a, b| (*a - *b).abs() < dedup_tolerance);
    (out, truncated)
}

fn runtime_target_allowed(tune: &TuningOption, target: &str) -> bool {
    tune.runtime_targets.is_empty()
        || tune
            .runtime_targets
            .iter()
            .any(|t| t.runtime_target == target)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::TuningEffect;

    #[test]
    fn tuning_curves_are_always_linear() {
        // The site's tuning evaluator (`mr`) never reads InterpMode.
        for mode in ["RCIM_Cubic", "RCIM_Constant", "RCIM_Bogus", ""] {
            let mut first = key(0.0, 0.0);
            first.interp_mode = mode.to_string();
            first.leave_tangent = 10.0;
            let points = vec![first, key(10.0, 10.0)];
            assert_eq!(
                eval_curve_points(&points, 5.0),
                Some(5.0),
                "mode {mode:?}"
            );
            let tune = TuningOption {
                min_value: 0.0,
                max_value: 10.0,
                step_value: 1.0,
                default_value: 0.0,
                display_effects: vec![TuningEffect {
                    attribute_name_snapshot: "开镜速度".to_string(),
                    curve_points: points,
                    ..Default::default()
                }],
                ..Default::default()
            };
            assert_eq!(tune_channels(&tune, 5.0).open, 5.0, "mode {mode:?}");
        }
    }

    #[test]
    fn absorbed_step_does_not_loop_forever() {
        // At 1e16 the ulp is 2, so v + 1 == v. The enumeration must stop and
        // report an incomplete grid instead of pushing the same value forever.
        let tune = TuningOption {
            min_value: 1e16,
            max_value: 1e16 + 100.0,
            default_value: 1e16,
            step_value: 1.0,
            ..Default::default()
        };
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(tune_offer_values(&tune));
        });
        let (values, truncated) = rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("tune_offer_values must terminate when v + step == v");
        assert!(truncated, "an absorbed step cannot claim a complete grid");
        assert!(values.len() < 10, "unexpected values: {values:?}");
    }

    #[test]
    fn tiny_step_grid_keeps_every_candidate() {
        // decimals(1e-9) used to be 0, which quantized the whole grid to
        // integers, and the 1e-9 dedup tolerance merged adjacent steps.
        let tune = TuningOption {
            min_value: 0.0,
            max_value: 6e-5,
            default_value: 0.0,
            step_value: 1e-9,
            ..Default::default()
        };
        let (values, truncated) = tune_offer_values(&tune);
        assert!(!truncated);
        assert_eq!(values.len(), 60_001);
        assert!(values.windows(2).all(|w| w[1] - w[0] >= 0.999e-9));
    }

    #[test]
    fn unrepresentable_precision_marks_the_grid_truncated() {
        // 1e-20 needs 20 decimal places, beyond the 15 supported.
        let tune = TuningOption {
            min_value: 0.0,
            max_value: 1e-19,
            default_value: 0.0,
            step_value: 1e-20,
            ..Default::default()
        };
        let (_, truncated) = tune_offer_values(&tune);
        assert!(truncated);
    }

    #[test]
    fn move_speed_channel_matches_any_move_speed_name() {
        // The site matches the effect name by substring, and the evaluator
        // already sums "移动速度"; only this channel used an exact name.
        for name in ["%瞄准时移动速度", "%瞄准移动速度"] {
            let tune = TuningOption {
                display_effects: vec![TuningEffect {
                    attribute_name_snapshot: name.to_string(),
                    curve_points: vec![key(0.0, 0.0), key(20.0, 20.0)],
                    ..Default::default()
                }],
                ..Default::default()
            };
            let channels = tune_channels(&tune, 7.0);
            assert_eq!(channels.move_speed, 7.0, "name {name}");
            assert!((channels.move_runtime - 0.07).abs() < 1e-12, "name {name}");
        }
    }

    #[test]
    fn quantize_clamps_and_steps() {
        assert!((quantize(12.0, -10.0, 10.0, 1.0) - 10.0).abs() < 1e-9);
        assert!((quantize(-12.0, -10.0, 10.0, 1.0) + 10.0).abs() < 1e-9);
        assert!((quantize(2.4, 0.0, 10.0, 1.0) - 2.0).abs() < 1e-9);
        assert!((quantize(2.6, 0.0, 10.0, 1.0) - 3.0).abs() < 1e-9);
    }

    fn key(time: f64, value: f64) -> CurveKey {
        CurveKey {
            time,
            value,
            interp_mode: "RCIM_Linear".to_string(),
            leave_tangent: 0.0,
            arrive_tangent: 0.0,
            key_index: 0,
        }
    }

    #[test]
    fn curve_points_fast_path_matches_sorted_fallback() {
        let sorted = vec![key(0.0, 1.0), key(10.0, 0.5), key(20.0, 0.75)];
        let mut unsorted = sorted.clone();
        unsorted.swap(0, 2);
        assert!(points_are_sorted(&sorted));
        assert!(!points_are_sorted(&unsorted));
        for x in [-1.0, 0.0, 3.5, 10.0, 17.25, 20.0, 40.0] {
            let fast = eval_sorted_curve_points(&sorted, x);
            assert_eq!(fast, eval_curve_points(&sorted, x));
            assert_eq!(fast, eval_curve_points(&unsorted, x));
        }
    }

    #[test]
    fn quantize_with_hoisted_decimals_matches_quantize() {
        for (min, max, step, value) in [
            (0.0, 10.0, 1.0, 2.6),
            (-10.0, 10.0, 0.5, 2.4),
            (0.25, 9.75, 0.25, 3.3),
            (0.0, 100.0, 0.0, 42.0),
        ] {
            let d = decimals(min).max(decimals(max)).max(decimals(step));
            assert_eq!(
                quantize(value, min, max, step),
                quantize_with_decimals(value, min, max, step, d)
            );
        }
    }

    #[test]
    fn tuning_offer_sums_channels() {
        let tune = TuningOption {
            min_value: -10.0,
            max_value: 10.0,
            step_value: 1.0,
            default_value: 0.0,
            display_effects: vec![TuningEffect {
                attribute_name_snapshot: "%额外后坐力控制".to_string(),
                curve_points: vec![key(-10.0, -20.0), key(0.0, 0.0), key(10.0, 5.0)],
                ..Default::default()
            }],
            ..Default::default()
        };
        let (h, v, k) = tune_offer(&tune, -10.0);
        assert!((h + 20.0).abs() < 1e-9);
        assert!((v + 20.0).abs() < 1e-9);
        assert!((k - 0.0).abs() < 1e-9);
        let (h, v, _) = tune_offer(&tune, 10.0);
        assert!((h - 5.0).abs() < 1e-9);
        assert!((v - 5.0).abs() < 1e-9);
    }
}
