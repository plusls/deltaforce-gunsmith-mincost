use crate::assembly::{Assembler, VisibleSlot};
use crate::constraint_reduction::{reduce_stability_constraints, REDUCIBLE_STABILITY_METRICS};
use crate::metrics::{
    eval_curve_points, quantize, scale_factor, scale_ratio, tune_channels, tune_offer,
    tune_offer_values, tuning_available, DetailMetrics, DetailSums, Evaluator, TuningInstance,
    TuningState, DETAIL_METRICS, HANDLING_ATTRS, HIPFIRE_ATTRS, STABILITY_ATTRS,
};
use crate::model::{Curve, CurveKey, Part, WeaponPayload};
use rustc_hash::{FxHashMap, FxHashSet};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::rc::Rc;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Constraint {
    pub id: String,
    #[serde(default)]
    pub min: Option<f64>,
    #[serde(default)]
    pub max: Option<f64>,
}

#[derive(Debug, Clone)]
pub struct SolveOptions {
    pub mode: String,
    pub laser_enabled: bool,
    pub bipod_deployed: bool,
    pub locked_paths: FxHashMap<String, Option<String>>,
    pub locked_tuning: FxHashMap<String, f64>,
    pub excluded_part_ids: FxHashSet<String>,
    pub constraints: Vec<Constraint>,
    pub max_nodes: u64,
    pub max_leaves: u64,
    pub max_results: usize,
    pub fixed_cost_cap: Option<i64>,
    pub first_solution: bool,
    pub deadline_ms: Option<u64>,
}

impl Default for SolveOptions {
    fn default() -> Self {
        Self {
            mode: "operations".to_string(),
            laser_enabled: true,
            bipod_deployed: false,
            locked_paths: FxHashMap::default(),
            locked_tuning: FxHashMap::default(),
            excluded_part_ids: FxHashSet::default(),
            constraints: Vec::new(),
            max_nodes: 5_000_000,
            max_leaves: 2_000_000,
            max_results: 10,
            fixed_cost_cap: None,
            first_solution: false,
            deadline_ms: None,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ConstraintResult {
    pub id: String,
    pub value: f64,
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub ok: bool,
}

/// Abort a speculative exact pass once its Pareto frontier exceeds this many
/// states. The frontier is kept cost-ordered, so aborting only affects how
/// quickly a tighter incumbent is found; the final pass has no such limit.
const FRONTIER_ABORT_STATES: usize = 8_192;
/// Do not abort a DP level before it has run this long. Fast passes with a
/// wide-but-short-lived frontier must still complete exactly.
const FRONTIER_ABORT_LEVEL_MS: u64 = 1_000;
/// Work units (DFS nodes plus evaluated leaves) the heuristic improvement
/// pass may spend without improving the incumbent before it gives up.
const SEED_IMPROVE_WORK: u64 = 10_000;
/// Hard wall-clock cap on the heuristic improvement pass.
const SEED_IMPROVE_MS: u64 = 8_000;

pub const PANEL_ATTRS: [&str; 5] = ["10001", "10005", "10006", "10007", "10008"];
pub const RECOIL_METRICS: [&str; 5] = [
    "recoil-continuous-horizontal",
    "recoil-continuous-vertical",
    "recoil-single-horizontal",
    "recoil-single-vertical",
    "recoil-fire-stability",
];

pub fn is_supported_constraint(id: &str) -> bool {
    RECOIL_METRICS.contains(&id) || PANEL_ATTRS.contains(&id) || DETAIL_METRICS.contains(&id)
}

fn is_recoil_metric(id: &str) -> bool {
    RECOIL_METRICS.contains(&id)
}

fn is_panel_attr(id: &str) -> bool {
    PANEL_ATTRS.contains(&id)
}

fn panel_attr_index(id: &str) -> Option<usize> {
    PANEL_ATTRS.iter().position(|attr| *attr == id)
}

/// Whether a detail metric consumes one of the tuning sums. Every other
/// supported detail metric is a function of the installed parts alone, so a
/// single evaluation can rule out the entire tuning frontier.
fn detail_constraint_needs_tuning(id: &str) -> bool {
    matches!(
        id,
        "handling-aim-time"
            | "handling-sprint-fire-delay"
            | "handling-aim-move-speed"
            | "stability-breath-range"
            | "stability-breath-duration"
            | "stability-movement-camera"
    )
}

/// A tuning sum that a single detail metric depends on monotonically. The
/// metric's value is otherwise fixed by the installed parts, so evaluating it
/// at the extreme sum values present in the frontier proves whether any offer
/// can satisfy the constraint.
#[derive(Clone, Copy)]
enum MonotoneChannel {
    Open,
    MoveStability,
    BreathStability,
}

impl MonotoneChannel {
    /// Returns the channel and whether the metric increases with it.
    fn for_constraint(id: &str) -> Option<(Self, bool)> {
        match id {
            "handling-aim-time" | "handling-sprint-fire-delay" => Some((Self::Open, false)),
            "stability-movement-camera" => Some((Self::MoveStability, false)),
            "stability-breath-range" => Some((Self::BreathStability, false)),
            "stability-breath-duration" => Some((Self::BreathStability, true)),
            _ => None,
        }
    }

    fn index(self) -> usize {
        match self {
            Self::Open => 0,
            Self::MoveStability => 1,
            Self::BreathStability => 2,
        }
    }

    fn value(self, offer: &Offer) -> f64 {
        match self {
            Self::Open => offer.e_open,
            Self::MoveStability => offer.e_move_stability,
            Self::BreathStability => offer.e_breath,
        }
    }

    fn sums(self, value: f64) -> DetailSums {
        let mut sums = DetailSums::default();
        match self {
            Self::Open => sums.open_speed = value,
            Self::MoveStability => sums.move_stability = value,
            Self::BreathStability => sums.breath_stability = value,
        }
        sums
    }
}

/// Proves whether some offer can satisfy every monotone detail constraint.
/// Returns false only when an extreme evaluation shows that no offer can.
fn frontier_reachable(
    frontier: &[Offer],
    parts: &[&Part],
    constraints: &[&Constraint],
    evaluator: &Evaluator<'_>,
) -> bool {
    if frontier.is_empty() {
        return false;
    }
    let mut ranges = [(f64::INFINITY, f64::NEG_INFINITY); 3];
    for offer in frontier {
        for channel in [
            MonotoneChannel::Open,
            MonotoneChannel::MoveStability,
            MonotoneChannel::BreathStability,
        ] {
            let value = channel.value(offer);
            let range = &mut ranges[channel.index()];
            range.0 = range.0.min(value);
            range.1 = range.1.max(value);
        }
    }
    for constraint in constraints {
        let Some((channel, increasing)) = MonotoneChannel::for_constraint(&constraint.id) else {
            continue;
        };
        let (lo, hi) = ranges[channel.index()];
        if !lo.is_finite() || !hi.is_finite() {
            continue;
        }
        // `tuning_scale` is `clamp_factor(1 / (1 + open / 100))`. Below
        // `open = -100` the ratio turns negative and clamps to the low bound,
        // so a range that straddles -100 is not monotone and the extreme test
        // would be invalid.
        if matches!(channel, MonotoneChannel::Open) && lo <= -100.0 && hi > -100.0 {
            continue;
        }
        let evaluate = |value: f64| {
            evaluator
                .detail_with(parts, &channel.sums(value))
                .value(&constraint.id)
        };
        let (lo_value, hi_value) = (evaluate(lo), evaluate(hi));
        // Every mapped metric depends on the channel through a positive
        // constant times a monotone shape, so the direction follows the
        // assumed sign. A payload with a negative breath curve or breath
        // factor would reverse it; skip instead of rejecting when the
        // endpoints contradict the assumed direction.
        let direction_ok = match (lo_value, hi_value) {
            (Some(low), Some(high)) if increasing => high >= low - 1e-9,
            (Some(low), Some(high)) => high <= low + 1e-9,
            _ => false,
        };
        if !direction_ok {
            continue;
        }
        if let Some(max) = constraint.max {
            let smallest = if increasing { lo_value } else { hi_value };
            if smallest.is_some_and(|value| value > max + 1e-9) {
                return false;
            }
        }
        if let Some(min) = constraint.min {
            let largest = if increasing { hi_value } else { lo_value };
            if largest.is_some_and(|value| value < min - 1e-9) {
                return false;
            }
        }
    }
    true
}

#[derive(Debug, Clone, Serialize)]
pub struct InstalledPart {
    pub slot_path: String,
    pub slot_name: String,
    pub part_id: String,
    pub part_name: String,
    pub price: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Solution {
    pub attachment_price: i64,
    pub total_price: i64,
    pub recoil: RecoilSummary,
    pub panel: FxHashMap<String, f64>,
    pub detail: DetailMetrics,
    pub tuning: FxHashMap<String, f64>,
    pub constraints: Vec<ConstraintResult>,
    pub installed_parts: Vec<InstalledPart>,
    pub installed_part_keys: Vec<String>,
    pub selections: FxHashMap<String, Option<String>>,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct RecoilSummary {
    pub score: f64,
    pub continuous_h: f64,
    pub continuous_v: f64,
    pub single_h: f64,
    pub single_v: f64,
    pub fire_stability: f64,
    pub d: f64,
    pub f_h: f64,
    pub f_v: f64,
    pub s: f64,
    pub a_h: f64,
    pub a_v: f64,
    pub c_h: f64,
    pub c_v: f64,
    pub c_k: f64,
    pub t_h: f64,
    pub t_v: f64,
    pub t_k: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct SolveResult {
    pub optimal: bool,
    pub truncated: bool,
    pub nodes: u64,
    pub leaves: u64,
    pub best: Option<Solution>,
    pub alternatives: Vec<Solution>,
    pub message: String,
}

pub struct Solver<'a> {
    payload: &'a WeaponPayload,
    assembler: Assembler<'a>,
    evaluator: Evaluator<'a>,
    options: &'a SolveOptions,
    search_constraints: Vec<Constraint>,
    relevance: Relevance,
    offer_cache: FxHashMap<(String, String), Vec<Offer>>,
    part_bounds: FxHashMap<String, MetricBound>,
    part_prices: FxHashMap<String, i64>,
    slot_bounds: FxHashMap<String, FxHashMap<String, MetricBound>>,
    structural_parts: FxHashSet<String>,
    dp_ok: bool,
    negative_recoil_max: bool,
    dp_dims: DpDims,
    tune_dims: TuneDims,
    aim_dp: AimDir,
    panel_attrs: Vec<String>,
    panel_dirs: Vec<PanelDir>,
    seed_panel_attrs: Vec<usize>,
    seed_aim: f64,
    dp_metric_cache: FxHashMap<(String, String, bool), ExactPart>,
    /// Parts whose display stability addend is hidden from the panel, so the
    /// DP cannot tell their contribution apart from not installing them.
    hidden_stab_parts: FxHashSet<String>,
    /// Whether a single-sided stability constraint was rewritten to a panel
    /// bound. The reduction stays valid, but the DP abstraction loses the
    /// internal-part offset the original metric still sees.
    has_reduced_stability: bool,
    tune_frontier_cache: FxHashMap<String, CachedFrontier>,
    tune_cap: usize,
    tune_next: usize,
    tune_incomplete: bool,
    control_curve: Option<Curve>,
    gunkick_curve: Option<Curve>,
    control_min: Option<f64>,
    gunkick_min: Option<f64>,
    gunkick0: f64,
    aim_scale: AimScaleFloor,
    curves_nonincreasing: bool,
    weapon_price: i64,
    seed: bool,
    seed_continue: bool,
    seed_continue_until: u64,
    seed_continue_deadline: Option<u64>,
    frontier_abort: Option<usize>,
    frontier_abort_not_before: Option<u64>,
    aborted_pass: bool,
    cost_cap: i64,
    run_found: bool,
    deadline_at: Option<u64>,
    best: Option<Solution>,
    alternatives: Vec<Solution>,
    nodes: u64,
    leaves: u64,
    truncated: bool,
    heuristic_incomplete: bool,
    seed_beam_tried: bool,
    stop: bool,
    best_cost: i64,
    panel_base: [f64; 5],
}

#[derive(Debug, Clone, Default)]
struct Relevance {
    slots: FxHashMap<String, FxHashSet<String>>,
}

impl Relevance {
    fn contains(&self, host: &str, slot: &str) -> bool {
        self.slots
            .get(host)
            .map(|slots| slots.contains(slot))
            .unwrap_or(false)
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct DpDims {
    d: bool,
    ah: bool,
    av: bool,
    ch: bool,
    cv: bool,
    ck: bool,
    eh: bool,
    ev: bool,
    ek: bool,
    hs: bool,
    adsm: bool,
    eopen: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AimDir {
    MoreHandling,
    LessHandling,
    Equal,
    Ignore,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PanelDir {
    Higher,
    Lower,
    Equal,
    Ignore,
}

impl DpDims {
    fn from_constraints(constraints: &[Constraint]) -> Self {
        let mut dims = Self::default();
        for c in constraints {
            match c.id.as_str() {
                "recoil-continuous-horizontal" | "recoil-single-horizontal" => {
                    dims.d = true;
                    dims.ah = true;
                    dims.ch = true;
                    dims.eh = true;
                }
                "recoil-continuous-vertical" | "recoil-single-vertical" => {
                    dims.d = true;
                    dims.av = true;
                    dims.cv = true;
                    dims.ev = true;
                }
                "recoil-fire-stability" => {
                    dims.d = true;
                    dims.ck = true;
                    dims.ek = true;
                }
                "handling-aim-time" => {
                    dims.hs = true;
                    dims.adsm = true;
                    dims.eopen = true;
                }
                _ => {}
            }
        }
        dims
    }
}

#[derive(Debug, Clone)]
struct Offer {
    e_h: f64,
    e_v: f64,
    e_k: f64,
    e_open: f64,
    e_move: f64,
    e_move_runtime: f64,
    e_breath: f64,
    e_move_stability: f64,
    values: Vec<f64>,
    deviation: f64,
}

/// True when the offer sets every tuning in `assignment`. Key order is not
/// assumed: the exact pass can report a different key order than the frontier.
fn offer_matches_assignment(offer: &Offer, keys: &[String], assignment: &[(String, f64)]) -> bool {
    !assignment.is_empty()
        && assignment.iter().all(|(key, value)| {
            keys.iter()
                .position(|candidate| candidate == key)
                .and_then(|idx| offer.values.get(idx))
                .is_some_and(|current| (current - value).abs() <= 1e-12)
        })
}

#[derive(Clone)]
struct CachedFrontier {
    offers: Rc<Vec<Offer>>,
    /// Tuning keys in the same order as every offer's `values`. The keys are
    /// identical for all offers in a frontier, so they are stored once instead
    /// of once per offer.
    keys: Rc<Vec<String>>,
    capped: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TuneDir {
    Higher,
    Lower,
    Equal,
    Ignore,
}

#[derive(Debug, Clone, Copy)]
struct TuneDims {
    open: TuneDir,
    move_effect: TuneDir,
    breath: TuneDir,
    move_stability: TuneDir,
    recoil_h: TuneDir,
    recoil_v: TuneDir,
    recoil_k: TuneDir,
}

impl TuneDims {
    fn from_constraints(constraints: &[Constraint]) -> Self {
        let mut open_high = false;
        let mut open_low = false;
        let mut move_high = false;
        let mut move_low = false;
        let mut breath_high = false;
        let mut breath_low = false;
        let mut stability_high = false;
        let mut stability_low = false;
        let mut h_high = false;
        let mut h_low = false;
        let mut v_high = false;
        let mut v_low = false;
        let mut k_high = false;
        let mut k_low = false;
        for c in constraints {
            let max = c.max.is_some();
            let min = c.min.is_some();
            match c.id.as_str() {
                "recoil-continuous-horizontal" | "recoil-single-horizontal" => {
                    h_high |= max;
                    h_low |= min;
                }
                "recoil-continuous-vertical" | "recoil-single-vertical" => {
                    v_high |= max;
                    v_low |= min;
                }
                "recoil-fire-stability" => {
                    k_high |= max;
                    k_low |= min;
                }
                "handling-aim-time" | "handling-sprint-fire-delay" => {
                    open_high |= max;
                    open_low |= min;
                }
                "handling-aim-move-speed" => {
                    move_low |= max;
                    move_high |= min;
                }
                "stability-breath-range" => {
                    breath_high |= max;
                    breath_low |= min;
                }
                "stability-breath-duration" => {
                    breath_high |= min;
                    breath_low |= max;
                }
                "stability-movement-camera" => {
                    stability_high |= max;
                    stability_low |= min;
                }
                _ => {}
            }
        }
        if !h_high && !h_low {
            h_high = true;
        }
        if !v_high && !v_low {
            v_high = true;
        }
        if !k_high && !k_low {
            k_high = true;
        }
        let dir = |high: bool, low: bool| {
            if high && !low {
                TuneDir::Higher
            } else if low && !high {
                TuneDir::Lower
            } else if high && low {
                TuneDir::Equal
            } else {
                TuneDir::Ignore
            }
        };
        Self {
            open: dir(open_high, open_low),
            move_effect: dir(move_high, move_low),
            breath: dir(breath_high, breath_low),
            move_stability: dir(stability_high, stability_low),
            recoil_h: dir(h_high, h_low),
            recoil_v: dir(v_high, v_low),
            recoil_k: dir(k_high, k_low),
        }
    }
}

fn channel_ok(a: f64, b: f64, dir: TuneDir) -> bool {
    match dir {
        TuneDir::Higher => a >= b - 1e-9,
        TuneDir::Lower => a <= b + 1e-9,
        TuneDir::Equal => (a - b).abs() <= 1e-9,
        TuneDir::Ignore => true,
    }
}

fn aim_direction(constraints: &[Constraint]) -> AimDir {
    let mut has_min = false;
    let mut has_max = false;
    for c in constraints {
        if c.id != "handling-aim-time" {
            continue;
        }
        has_min |= c.min.is_some();
        has_max |= c.max.is_some();
    }
    match (has_min, has_max) {
        (false, true) => AimDir::MoreHandling,
        (true, false) => AimDir::LessHandling,
        (true, true) => AimDir::Equal,
        _ => AimDir::Ignore,
    }
}

fn seed_aim_weight(constraints: &[Constraint]) -> f64 {
    let mut has_min = false;
    let mut has_max = false;
    for c in constraints {
        if c.id != "handling-aim-time" {
            continue;
        }
        has_min |= c.min.is_some();
        has_max |= c.max.is_some();
    }
    match (has_min, has_max) {
        (false, true) => 1.0,
        (true, false) => -1.0,
        _ => 0.0,
    }
}

fn rank_seed_candidates(candidates: Vec<(String, f64, i64)>) -> Vec<String> {
    let mut candidates = candidates;
    candidates.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(Ordering::Equal)
            .then_with(|| a.2.cmp(&b.2))
            .then_with(|| a.0.cmp(&b.0))
    });
    candidates.into_iter().map(|(id, _, _)| id).collect()
}

fn seed_metric_rank(metric: &ExactPart, panel_attrs: &[usize], aim: f64) -> f64 {
    let mut score = metric.score();
    for idx in panel_attrs {
        score += metric.panel.get(*idx).copied().unwrap_or(0.0) * 100.0;
    }
    score += aim * (-metric.adsm * 100.0 + metric.hs * 0.02 + metric.eopen * 2.0);
    score
}

#[derive(Debug, PartialEq, Eq)]
enum ReachResult {
    Found(Vec<usize>),
    Infeasible,
    Budget,
}

enum ExactTune {
    Found(TuningState, Vec<(String, f64)>),
    Infeasible,
    Unsupported,
}

fn reach_targets(
    increments: &[Vec<(i64, i64, i64, i64)>],
    targets: [Option<i64>; 4],
    budget: usize,
) -> ReachResult {
    let n = increments.len();
    let get = |x: &(i64, i64, i64, i64), c: usize| match c {
        0 => x.0,
        1 => x.1,
        2 => x.2,
        _ => x.3,
    };
    let max_sum = |c: usize| -> i64 {
        increments
            .iter()
            .map(|offers| offers.iter().map(|x| get(x, c)).max().unwrap_or(0))
            .sum()
    };
    for c in 0..4 {
        if let Some(t) = targets[c] {
            if t > max_sum(c) {
                return ReachResult::Infeasible;
            }
        }
    }
    let cap = |v: i64, t: Option<i64>| match t {
        Some(t) => v.min(t),
        None => v,
    };
    let mut visited: FxHashSet<(usize, i64, i64, i64, i64)> = FxHashSet::default();
    let mut chosen = vec![0usize; n];
    let mut used = 0usize;
    fn rec(
        i: usize,
        state: [i64; 4],
        increments: &[Vec<(i64, i64, i64, i64)>],
        targets: [Option<i64>; 4],
        cap: &dyn Fn(i64, Option<i64>) -> i64,
        visited: &mut FxHashSet<(usize, i64, i64, i64, i64)>,
        chosen: &mut [usize],
        used: &mut usize,
        budget: usize,
    ) -> Result<bool, ()> {
        *used += 1;
        if *used > budget {
            return Err(());
        }
        if i == increments.len() {
            return Ok((0..4).all(|c| targets[c].map(|t| state[c] >= t).unwrap_or(true)));
        }
        let key = (i, state[0], state[1], state[2], state[3]);
        if !visited.insert(key) {
            return Ok(false);
        }
        let mut order: Vec<usize> = (0..increments[i].len()).collect();
        let mut score: Vec<i64> = increments[i]
            .iter()
            .map(|x| {
                let xs = [x.0, x.1, x.2, x.3];
                (0..4)
                    .map(|c| match targets[c] {
                        Some(t) => xs[c].min((t - state[c]).max(0)),
                        None => 0,
                    })
                    .sum()
            })
            .collect();
        order.sort_by(|&a, &b| score[b].cmp(&score[a]));
        score.clear();
        for &oi in &order {
            let x = increments[i][oi];
            chosen[i] = oi;
            let next = [
                cap(state[0].saturating_add(x.0), targets[0]),
                cap(state[1].saturating_add(x.1), targets[1]),
                cap(state[2].saturating_add(x.2), targets[2]),
                cap(state[3].saturating_add(x.3), targets[3]),
            ];
            if rec(
                i + 1,
                next,
                increments,
                targets,
                cap,
                visited,
                chosen,
                used,
                budget,
            )? {
                return Ok(true);
            }
        }
        Ok(false)
    }
    let zeros = [0i64; 4];
    match rec(
        0,
        zeros,
        increments,
        targets,
        &cap,
        &mut visited,
        &mut chosen,
        &mut used,
        budget,
    ) {
        Ok(true) => ReachResult::Found(chosen),
        Ok(false) => ReachResult::Infeasible,
        Err(()) => ReachResult::Budget,
    }
}

#[derive(Debug, Clone, Copy)]
struct TuneReq {
    lower_h: f64,
    upper_h: f64,
    lower_v: f64,
    upper_v: f64,
    lower_k: f64,
    upper_k: f64,
    impossible: bool,
}

impl TuneReq {
    fn unconstrained() -> Self {
        Self {
            lower_h: f64::NEG_INFINITY,
            upper_h: f64::INFINITY,
            lower_v: f64::NEG_INFINITY,
            upper_v: f64::INFINITY,
            lower_k: f64::NEG_INFINITY,
            upper_k: f64::INFINITY,
            impossible: false,
        }
    }
}

impl<'a> Solver<'a> {
    pub fn new(
        payload: &'a WeaponPayload,
        weapon_id: &'a str,
        prices: &'a FxHashMap<String, i64>,
        options: &'a SolveOptions,
    ) -> Self {
        let mut assembler = Assembler::new(payload, weapon_id);
        assembler.set_excluded(options.excluded_part_ids.clone());
        let evaluator = Evaluator {
            payload,
            weapon_id,
            mode: &options.mode,
            laser_enabled: options.laser_enabled,
            bipod_deployed: options.bipod_deployed,
            prices,
        };
        let search_constraints =
            reduce_stability_constraints(payload, weapon_id, &options.mode, &options.constraints);
        let relevance = compute_relevance(payload, weapon_id, &search_constraints);
        let flat_bounds = compute_bounds(payload, weapon_id, &options.mode, options.laser_enabled);
        let mut slot_bounds: FxHashMap<String, FxHashMap<String, MetricBound>> =
            FxHashMap::default();
        for ((host, slot), bound) in flat_bounds {
            slot_bounds.entry(host).or_default().insert(slot, bound);
        }
        let part_bounds: FxHashMap<String, MetricBound> = payload
            .parts
            .iter()
            .map(|(id, part)| {
                (
                    id.clone(),
                    part_own_bound(part, weapon_id, &options.mode, options.laser_enabled),
                )
            })
            .collect();
        let part_prices: FxHashMap<String, i64> = payload
            .parts
            .iter()
            .map(|(id, part)| {
                let price = prices.get(part.price_id()).copied().unwrap_or(0);
                (id.clone(), price)
            })
            .collect();
        let (control_curve, gunkick_curve, gunkick0) = {
            let profile = payload.numeric_profiles.get(weapon_id);
            let curves = profile.map(|p| &p.linked_tables.recoil_curves);
            let control = curves
                .and_then(|c| {
                    if options.mode == "warfare" {
                        c.recoil_control_mp.as_ref().or(c.recoil_control.as_ref())
                    } else {
                        c.recoil_control.as_ref()
                    }
                })
                .cloned();
            let gunkick = curves
                .and_then(|c| {
                    if options.mode == "warfare" {
                        c.gunkick_mp.as_ref().or(c.gunkick.as_ref())
                    } else {
                        c.gunkick.as_ref()
                    }
                })
                .cloned();
            let gk0 = gunkick.as_ref().and_then(|c| c.eval(0.0)).unwrap_or(0.0);
            (control, gunkick, gk0)
        };
        let control_min = control_curve.as_ref().and_then(curve_min_value);
        let gunkick_min = gunkick_curve.as_ref().and_then(curve_min_value);
        let curves_nonincreasing = control_curve
            .as_ref()
            .map(curve_is_nonincreasing)
            .unwrap_or(true)
            && gunkick_curve
                .as_ref()
                .map(curve_is_nonincreasing)
                .unwrap_or(true);
        let mut structural_parts: FxHashSet<String> = FxHashSet::default();
        let rule_part_ids: FxHashSet<String> = payload
            .virtual_rules
            .get(weapon_id)
            .map(|rules| {
                rules
                    .iter()
                    .flat_map(|r| r.condition_item_ids.iter().cloned())
                    .collect()
            })
            .unwrap_or_default();
        for (id, part) in &payload.parts {
            let structural = !part.extra_occupy_socket_ids.is_empty()
                || payload
                    .slots_by_host
                    .get(id)
                    .map(|slots| !slots.is_empty())
                    .unwrap_or(false)
                || rule_part_ids.contains(id)
                || crate::metrics::has_laser(part)
                || !part.aim_recoil_profiles.is_empty();
            if structural {
                structural_parts.insert(id.clone());
            }
        }
        // The reduced 10007 bound is necessary but not equivalent: internal
        // parts contribute their stability addend to the original metric
        // without entering the panel, and the DP neither dedups nor dominates
        // on that hidden offset. Force the explicit Unset/Remove actions for
        // slots whose internal part carries such an addend so the DP never
        // folds two different offsets into one state.
        let has_reduced_stability = options.constraints.iter().any(|c| {
            REDUCIBLE_STABILITY_METRICS.contains(&c.id.as_str())
                && (c.min.is_some() != c.max.is_some())
                && !search_constraints
                    .iter()
                    .any(|s| s.id == c.id && s.min == c.min && s.max == c.max)
        });
        let mut hidden_stab_parts: FxHashSet<String> = FxHashSet::default();
        for (id, part) in &payload.parts {
            for modifier in part.display_mods_for(weapon_id, &options.mode) {
                if modifier.modifier_type != "Addend" {
                    continue;
                }
                let attr = normalize_attr(modifier.attribute_id.as_deref());
                if STABILITY_ATTRS.contains(&attr) {
                    hidden_stab_parts.insert(id.clone());
                    break;
                }
            }
        }
        let mut panel_attrs: Vec<String> = search_constraints
            .iter()
            .filter(|c| is_panel_attr(&c.id))
            .map(|c| c.id.clone())
            .collect();
        panel_attrs.sort();
        panel_attrs.dedup();
        let panel_dirs: Vec<PanelDir> = panel_attrs
            .iter()
            .map(|attr| {
                let mut has_min = false;
                let mut has_max = false;
                for c in &search_constraints {
                    if &c.id == attr {
                        has_min |= c.min.is_some();
                        has_max |= c.max.is_some();
                    }
                }
                match (has_min, has_max) {
                    (true, false) => PanelDir::Higher,
                    (false, true) => PanelDir::Lower,
                    (true, true) => PanelDir::Equal,
                    (false, false) => PanelDir::Ignore,
                }
            })
            .collect();
        let seed_panel_attrs: Vec<usize> = panel_attrs
            .iter()
            .enumerate()
            .filter(|(_, attr)| {
                search_constraints
                    .iter()
                    .any(|c| &c.id == *attr && c.min.is_some())
            })
            .map(|(idx, _)| idx)
            .collect();
        let mut dp_ok = search_constraints.iter().all(|c| {
            if c.id == "10001" {
                false
            } else if is_panel_attr(&c.id) {
                !(c.min.is_some() && c.max.is_some())
            } else if is_recoil_metric(&c.id) {
                c.min.is_none()
            } else if c.id == "handling-aim-time" {
                !(c.min.is_some() && c.max.is_some())
            } else {
                false
            }
        });
        let dp_dims = DpDims::from_constraints(&search_constraints);
        if dp_dims.d && !curves_nonincreasing {
            dp_ok = false;
        }
        // A negative recoil maximum inverts the usual "smaller is better"
        // order: with a negative control value a larger multiplier yields the
        // smaller metric. The Pareto frontier cannot express that, so use the
        // exhaustive DFS and keep every tuning offer.
        let negative_recoil_max = search_constraints
            .iter()
            .any(|c| is_recoil_metric(&c.id) && c.max.map(|max| max < 0.0).unwrap_or(false));
        if negative_recoil_max {
            dp_ok = false;
        }
        let mut panel_base = [0.0f64; 5];
        if let Some(stats) = payload.weapon_stats.get(weapon_id) {
            for (idx, attr) in PANEL_ATTRS.iter().enumerate() {
                panel_base[idx] = stats.display_stat(&options.mode, attr).unwrap_or(0.0);
            }
        }
        let weapon_price = payload
            .parts
            .get(weapon_id)
            .and_then(|p| prices.get(p.price_id()))
            .copied()
            .unwrap_or(0);
        Self {
            payload,
            assembler,
            evaluator,
            options,
            relevance,
            offer_cache: FxHashMap::default(),
            part_bounds,
            part_prices,
            slot_bounds,
            structural_parts,
            dp_ok,
            negative_recoil_max,
            dp_dims,
            tune_dims: TuneDims::from_constraints(&search_constraints),
            aim_dp: aim_direction(&search_constraints),
            panel_attrs,
            panel_dirs,
            seed_panel_attrs,
            seed_aim: seed_aim_weight(&search_constraints),
            search_constraints,
            dp_metric_cache: FxHashMap::default(),
            hidden_stab_parts,
            has_reduced_stability,
            tune_frontier_cache: FxHashMap::default(),
            tune_cap: TUNE_CAP,
            tune_next: TUNE_NEXT_LIMIT,
            tune_incomplete: false,
            control_curve,
            gunkick_curve,
            control_min,
            gunkick_min,
            gunkick0,
            aim_scale: aim_scale_floor(payload, weapon_id, &options.mode),
            curves_nonincreasing,
            weapon_price,
            seed: false,
            seed_continue: false,
            seed_continue_until: 0,
            seed_continue_deadline: None,
            frontier_abort: None,
            frontier_abort_not_before: None,
            aborted_pass: false,
            cost_cap: i64::MAX,
            run_found: false,
            deadline_at: None,
            best: None,
            alternatives: Vec::new(),
            nodes: 0,
            leaves: 0,
            truncated: false,
            heuristic_incomplete: false,
            seed_beam_tried: false,
            stop: false,
            best_cost: i64::MAX,
            panel_base,
        }
    }

    pub fn solve(mut self) -> SolveResult {
        if locked_paths_conflict(self.payload, &self.options.locked_paths) {
            return SolveResult {
                optimal: false,
                truncated: false,
                nodes: 0,
                leaves: 0,
                best: None,
                alternatives: Vec::new(),
                message: "no build satisfies the constraints".to_string(),
            };
        }
        let has_constraints = self
            .options
            .constraints
            .iter()
            .any(|c| c.min.is_some() || c.max.is_some());
        let fixed_cap = self.options.fixed_cost_cap;
        self.cost_cap = i64::MAX;
        let mut selections: FxHashMap<String, Option<String>> = FxHashMap::default();
        let mut decided: FxHashSet<String> = FxHashSet::default();
        let mut deferred: FxHashSet<String> = FxHashSet::default();
        let reset = |solver: &mut Self,
                     selections: &mut FxHashMap<String, Option<String>>,
                     decided: &mut FxHashSet<String>| {
            selections.clear();
            decided.clear();
            for (path, part) in &solver.options.locked_paths {
                selections.insert(path.clone(), part.clone());
                decided.insert(path.clone());
            }
            solver.stop = false;
            solver.truncated = false;
            solver.heuristic_incomplete = false;
            solver.tune_incomplete = false;
            solver.seed_beam_tried = false;
            solver.nodes = 0;
            solver.leaves = 0;
            solver.run_found = false;
        };
        for (path, part) in &self.options.locked_paths {
            selections.insert(path.clone(), part.clone());
            decided.insert(path.clone());
        }

        self.deadline_at = self
            .options
            .deadline_ms
            .filter(|ms| *ms > 0)
            .map(|ms| now_ms().saturating_add(ms));
        let mut truncated = false;
        let mut incomplete: bool;
        let mut total_nodes = 0u64;
        let mut total_leaves = 0u64;
        let mut exact_pass_ran = false;
        if self.options.first_solution {
            self.cost_cap = fixed_cap.unwrap_or(i64::MAX);
            self.seed = true;
            self.dfs(&mut selections, &mut decided, &mut deferred);
            self.seed = false;
            let found = self.best.is_some();
            let message = if found {
                "first feasible build found"
            } else if self.truncated {
                "budget exhausted; no build found"
            } else if self.heuristic_incomplete {
                "search stopped early by internal limits; no build found"
            } else {
                "no build satisfies the constraints"
            };
            return SolveResult {
                optimal: false,
                truncated: self.truncated,
                nodes: self.nodes,
                leaves: self.leaves,
                best: self.best,
                alternatives: self.alternatives,
                message: message.to_string(),
            };
        }
        if has_constraints {
            self.seed = true;
            self.dfs(&mut selections, &mut decided, &mut deferred);
            self.seed = false;
            total_nodes += self.nodes;
            total_leaves += self.leaves;
            truncated |= self.truncated;
            incomplete = self.heuristic_incomplete;
            let seed_found = self.run_found;
            if let Some(cap) = fixed_cap {
                if self
                    .best
                    .as_ref()
                    .map(|b| b.total_price > cap)
                    .unwrap_or(false)
                {
                    self.best = None;
                    self.best_cost = i64::MAX;
                    self.alternatives.clear();
                }
                self.cost_cap = cap;
            }
            // A truncated seed may still have left time on the clock; the
            // optimal pass has its own budget checks and resets the flag.
            let deadline_hit = self
                .deadline_at
                .is_some_and(|deadline| now_ms() >= deadline);
            if (self.best.is_some() || (fixed_cap.is_some() && seed_found))
                && !(self.truncated && deadline_hit)
            {
                reset(&mut self, &mut selections, &mut decided);
                self.frontier_abort = Some(FRONTIER_ABORT_STATES);
                self.dfs(&mut selections, &mut decided, &mut deferred);
                exact_pass_ran = true;
                self.frontier_abort = None;
                self.frontier_abort_not_before = None;
                total_nodes += self.nodes;
                total_leaves += self.leaves;
                if self.aborted_pass {
                    // The exact frontier grew past the point where the DP can
                    // finish in reasonable time. Spend a bounded slice of the
                    // budget on the cheap heuristic search to tighten the
                    // incumbent, then rerun the exact pass with that bound.
                    self.aborted_pass = false;
                    let deadline_hit = self
                        .deadline_at
                        .is_some_and(|deadline| now_ms() >= deadline);
                    if deadline_hit {
                        truncated = true;
                    } else {
                        self.nodes = 0;
                        self.leaves = 0;
                        self.run_found = false;
                        self.stop = false;
                        self.truncated = false;
                        self.heuristic_incomplete = false;
                        self.tune_incomplete = false;
                        self.seed = true;
                        self.seed_continue = true;
                        self.seed_continue_until = self.work() + SEED_IMPROVE_WORK;
                        self.seed_continue_deadline = Some(now_ms() + SEED_IMPROVE_MS);
                        self.dfs(&mut selections, &mut decided, &mut deferred);
                        self.seed = false;
                        self.seed_continue = false;
                        self.seed_continue_deadline = None;
                        total_nodes += self.nodes;
                        total_leaves += self.leaves;
                        let improve_stopped = self.truncated
                            || self
                                .deadline_at
                                .is_some_and(|deadline| now_ms() >= deadline);
                        if improve_stopped {
                            truncated = true;
                            incomplete = self.heuristic_incomplete;
                        } else {
                            reset(&mut self, &mut selections, &mut decided);
                            self.dfs(&mut selections, &mut decided, &mut deferred);
                            total_nodes += self.nodes;
                            total_leaves += self.leaves;
                            truncated |= self.truncated;
                            incomplete = self.heuristic_incomplete;
                        }
                    }
                } else {
                    truncated |= self.truncated;
                    incomplete = self.heuristic_incomplete;
                }
            }
        } else {
            self.cost_cap = fixed_cap.unwrap_or(i64::MAX);
            self.dfs(&mut selections, &mut decided, &mut deferred);
            exact_pass_ran = true;
            total_nodes += self.nodes;
            total_leaves += self.leaves;
            truncated = self.truncated;
            incomplete = self.heuristic_incomplete;
        }
        if exact_pass_ran && self.tune_incomplete && !truncated && self.stage2_time_ok() {
            self.tune_cap = TUNE_STAGE2_CAP;
            self.tune_next = TUNE_STAGE2_NEXT;
            // Cached frontiers were built under the first pass's caps.
            self.tune_frontier_cache.clear();
            reset(&mut self, &mut selections, &mut decided);
            deferred.clear();
            self.aborted_pass = false;
            self.dfs(&mut selections, &mut decided, &mut deferred);
            total_nodes += self.nodes;
            total_leaves += self.leaves;
            truncated = self.truncated;
            incomplete = self.heuristic_incomplete || self.aborted_pass;
        }
        let optimal = !truncated && !incomplete && self.best.is_some();
        let message = if truncated {
            "budget exhausted; result is the best found so far".to_string()
        } else if incomplete {
            "search stopped early by internal limits; result is the best found so far".to_string()
        } else if self.best.is_some() {
            "search complete".to_string()
        } else {
            "no build satisfies the constraints".to_string()
        };
        SolveResult {
            optimal,
            truncated,
            nodes: total_nodes,
            leaves: total_leaves,
            best: self.best,
            alternatives: self.alternatives,
            message,
        }
    }

    fn work(&self) -> u64 {
        self.nodes.saturating_add(self.leaves)
    }

    fn seed_continue_exhausted(&self) -> bool {
        self.seed
            && self.seed_continue
            && (self.work() >= self.seed_continue_until
                || self.seed_continue_deadline.is_some_and(|d| now_ms() >= d))
    }

    fn budget_exceeded(&mut self) -> bool {
        if self.nodes >= self.options.max_nodes || self.leaves >= self.options.max_leaves {
            self.truncated = true;
            self.stop = true;
            return true;
        }
        if let Some(deadline) = self.deadline_at {
            if now_ms() >= deadline {
                self.truncated = true;
                self.stop = true;
                return true;
            }
        }
        false
    }

    fn dfs(
        &mut self,
        selections: &mut FxHashMap<String, Option<String>>,
        decided: &mut FxHashSet<String>,
        deferred: &mut FxHashSet<String>,
    ) {
        if self.stop || self.budget_exceeded() {
            return;
        }
        if self.seed_continue_exhausted() {
            self.stop = true;
            return;
        }
        self.nodes += 1;

        let assembly = self.assembler.resolve(selections);

        let mut next: Option<usize> = None;
        for (idx, slot) in assembly.visible_slots.iter().enumerate() {
            if decided.contains(&slot.path_key) || deferred.contains(&slot.path_key) {
                continue;
            }
            if !self.relevance.contains(&slot.host_part_id, &slot.slot.id) {
                continue;
            }
            next = Some(idx);
            break;
        }

        let Some(idx) = next else {
            self.dp_leaf(selections, decided, &assembly);
            return;
        };

        if self.cannot_satisfy(selections, &assembly, decided) {
            return;
        }
        if !self.seed {
            let lower = self.cost_lower_bound(selections, &assembly, decided) + self.weapon_price;
            if self.cost_cap < i64::MAX && lower > self.cost_cap {
                return;
            }
            if self.best_cost < i64::MAX && lower >= self.best_cost {
                return;
            }
        }

        let actions = self.slot_actions(&assembly.visible_slots[idx], selections);
        let path = assembly.visible_slots[idx].path_key.clone();
        for action in actions {
            if self.stop {
                return;
            }
            let old = selections.get(&path).cloned();
            let mut was_deferred = false;
            match &action {
                Action::Unset => {
                    selections.remove(&path);
                }
                Action::Remove => {
                    selections.insert(path.clone(), None);
                }
                Action::Defer => {
                    was_deferred = true;
                }
                Action::Part(id) => {
                    selections.insert(path.clone(), Some(id.clone()));
                }
            }
            if was_deferred {
                deferred.insert(path.clone());
            } else {
                decided.insert(path.clone());
            }
            self.dfs(selections, decided, deferred);
            if was_deferred {
                deferred.remove(&path);
            } else {
                decided.remove(&path);
            }
            if let Some(old) = old {
                selections.insert(path.clone(), old);
            } else {
                selections.remove(&path);
            }
        }
    }

    fn slot_actions(
        &mut self,
        slot: &VisibleSlot,
        selections: &FxHashMap<String, Option<String>>,
    ) -> Vec<Action> {
        let path = &slot.path_key;
        let mut parts: Vec<String> = slot
            .candidate_ids
            .iter()
            .chain(slot.slot.virtual_candidate_ids.iter())
            .filter(|id| !self.options.excluded_part_ids.contains(*id))
            .cloned()
            .collect();
        parts.sort();
        parts.dedup();
        let unset = slot.internal_part_id.clone();
        let mut forced: Vec<Action> = Vec::new();
        let mut can_defer = false;
        if self.dp_ok {
            let unset_structural = unset
                .as_ref()
                .map(|id| self.structural_parts.contains(id))
                .unwrap_or(false);
            // A structural default suppresses Defer, so the DP can never pick a
            // non-structural alternative for this slot. Keep those alternatives
            // in the explicit action list; otherwise they are unreachable.
            // Same for an internal part carrying a hidden stability addend:
            // the DP would fold Unset and Remove into one state.
            let hidden_unset = self.has_reduced_stability
                && unset
                    .as_ref()
                    .map(|id| self.hidden_stab_parts.contains(id))
                    .unwrap_or(false);
            if !unset_structural && !hidden_unset {
                parts.retain(|id| self.structural_parts.contains(id));
            }
            let remove = if slot.internal_is_default {
                self.effective_none_part(selections, path)
            } else {
                unset.clone()
            };
            let remove_structural = remove
                .as_ref()
                .map(|id| self.structural_parts.contains(id))
                .unwrap_or(false);
            if unset_structural || hidden_unset {
                forced.push(Action::Unset);
            }
            if (remove_structural || unset_structural || hidden_unset)
                && remove.as_deref() != unset.as_deref()
            {
                forced.push(Action::Remove);
            }
            can_defer = !unset_structural && !hidden_unset;
        } else if unset.is_some() {
            forced.push(Action::Unset);
            if slot.internal_is_default {
                forced.push(Action::Remove);
            }
        } else {
            forced.push(Action::Remove);
        }
        if self.seed {
            let mut ranked: Vec<(String, f64, i64)> = Vec::with_capacity(parts.len());
            for id in parts {
                let score = self.seed_part_score(&id, &slot.slot.name);
                let price = self.part_price(&id);
                ranked.push((id, score, price));
            }
            parts = rank_seed_candidates(ranked);
        } else {
            parts.sort_by(|a, b| {
                let pa = self.part_price(a);
                let pb = self.part_price(b);
                pa.cmp(&pb).then(a.cmp(b))
            });
        }
        let mut part_actions: Vec<Action> = Vec::new();
        for id in parts {
            if Some(id.as_str()) == unset.as_deref() {
                continue;
            }
            part_actions.push(Action::Part(id));
        }
        if !self.dp_ok {
            if let Some(id) = unset.clone() {
                part_actions.push(Action::Part(id));
            }
        }
        let mut actions: Vec<Action> = Vec::new();
        actions.extend(forced);
        if can_defer {
            actions.push(Action::Defer);
        }
        actions.extend(part_actions);
        actions
    }

    /// Whether an internal part currently hides a slot whose candidates an
    /// explicit removal could install.
    fn hidden_slot_release_possible(&self, assembly: &crate::assembly::Assembly) -> bool {
        let mut claimed: FxHashSet<&str> = FxHashSet::default();
        for item in &assembly.internal_parts {
            if let Some(part) = self.payload.parts.get(&item.part_id) {
                for extra in &part.extra_occupy_socket_ids {
                    claimed.insert(extra.as_str());
                }
            }
        }
        if claimed.is_empty() {
            return false;
        }
        for host in assembly
            .active_part_ids
            .iter()
            .map(String::as_str)
            .chain(std::iter::once(self.evaluator.weapon_id))
        {
            let Some(slots) = self.payload.slots_by_host.get(host) else {
                continue;
            };
            for slot in slots {
                if !claimed.contains(slot.id.as_str()) {
                    continue;
                }
                let visible = assembly
                    .visible_slots
                    .iter()
                    .any(|v| &v.host_part_id == host && v.slot.id == slot.id);
                if !visible {
                    return true;
                }
            }
        }
        false
    }

    fn cannot_satisfy(
        &self,
        _selections: &FxHashMap<String, Option<String>>,
        assembly: &crate::assembly::Assembly,
        decided: &FxHashSet<String>,
    ) -> bool {
        let has_panel = self
            .search_constraints
            .iter()
            .any(|c| is_panel_attr(&c.id) && c.id != "10001");
        let has_recoil_max = self
            .search_constraints
            .iter()
            .any(|c| c.max.is_some() && is_recoil_metric(&c.id));
        if !has_panel && !has_recoil_max {
            return false;
        }
        // The bound below only sees the current assembly. A default/internal
        // part may claim a socket through `extraOccupySocketIds` and hide the
        // slot behind it, yet removing that part releases the slot and its
        // candidates. Assume the best when such a release is possible.
        if self.hidden_slot_release_possible(assembly) {
            return false;
        }
        let has_visible_bound = |slot: &crate::assembly::VisibleSlot| {
            self.relevance.contains(&slot.host_part_id, &slot.slot.id)
                && self
                    .slot_bounds
                    .get(&slot.host_part_id)
                    .and_then(|slots| slots.get(&slot.slot.id))
                    .is_some()
        };
        let roots = bound_roots(
            &assembly.visible_slots,
            &assembly.internal_parts,
            decided,
            has_visible_bound,
        );
        let mut decided_bound = MetricBound::neutral();
        let mut assumed_bound = MetricBound::neutral();
        for root in roots {
            match root {
                BoundRoot::DecidedVisible(idx) => {
                    let slot = &assembly.visible_slots[idx];
                    let Some(id) = slot
                        .selected_part_id
                        .as_ref()
                        .or(slot.internal_part_id.as_ref())
                    else {
                        continue;
                    };
                    if let Some(b) = self.part_bounds.get(id) {
                        if slot.selected_part_id.is_some() {
                            decided_bound.add_a(b);
                        } else {
                            // A default/virtual part on a visible slot is
                            // internal: it never contributes to the panel.
                            decided_bound.add_non_panel(b);
                        }
                    }
                }
                BoundRoot::DecidedInternal(idx) => {
                    let item = &assembly.internal_parts[idx];
                    if let Some(b) = self.part_bounds.get(&item.part_id) {
                        decided_bound.add_non_panel(b);
                    }
                }
                BoundRoot::AssumedVisible(idx) => {
                    let slot = &assembly.visible_slots[idx];
                    if let Some(b) = self
                        .slot_bounds
                        .get(&slot.host_part_id)
                        .and_then(|slots| slots.get(&slot.slot.id))
                    {
                        assumed_bound.add_a(b);
                    }
                }
                BoundRoot::OptionalInternal(idx) => {
                    let item = &assembly.internal_parts[idx];
                    if !self.payload.parts.contains_key(&item.part_id) {
                        continue;
                    }
                    if let Some(b) = self.part_bounds.get(&item.part_id) {
                        assumed_bound.add_optional_non_panel(b);
                    }
                }
            }
        }
        let mut bound = decided_bound;
        bound.add_a(&assumed_bound);
        for c in &self.search_constraints {
            if let Some(idx) = panel_attr_index(&c.id) {
                if idx == 0 {
                    continue;
                }
                if let Some(min) = c.min {
                    if self.panel_base[idx] + bound.panel_max[idx] < min - 1e-9 {
                        return true;
                    }
                }
                if let Some(max) = c.max {
                    if self.panel_base[idx] + bound.panel_min[idx] > max + 1e-9 {
                        return true;
                    }
                }
            }
        }
        if !has_recoil_max {
            return false;
        }
        if self.control_curve.is_none() || self.gunkick_curve.is_none() || self.gunkick0 == 0.0 {
            return false;
        }
        let d_lo = decided_bound.max_d;
        let d_hi = bound.max_d;
        let cr = self
            .control_curve
            .as_ref()
            .and_then(|c| c.eval(d_lo))
            .into_iter()
            .chain(self.control_curve.as_ref().and_then(|c| c.eval(d_hi)))
            .chain(self.control_min)
            .fold(f64::INFINITY, f64::min);
        if !cr.is_finite() {
            return false;
        }
        let gk = self
            .gunkick_curve
            .as_ref()
            .and_then(|c| c.eval(d_lo))
            .into_iter()
            .chain(self.gunkick_curve.as_ref().and_then(|c| c.eval(d_hi)))
            .chain(self.gunkick_min)
            .fold(f64::INFINITY, f64::min);
        if !gk.is_finite() {
            return false;
        }
        // The product bound is only optimistic when every factor is
        // non-negative: with a negative control value a larger addend or
        // multiplier can make the result smaller, so multiplying the smallest
        // ones would overestimate it.
        let positive = |x: f64, c: f64| x > 0.0 && c > 0.0;
        let xh = 1.0 + bound.min_a_h + bound.min_t_h;
        let xv = 1.0 + bound.min_a_v + bound.min_t_v;
        let xk = 1.0 + bound.min_t_k;
        let mut f_h = (cr >= 0.0 && positive(xh, bound.min_c_h)).then(|| cr * xh * bound.min_c_h);
        let mut f_v = (cr >= 0.0 && positive(xv, bound.min_c_v)).then(|| cr * xv * bound.min_c_v);
        // The bound covers every installable part, so the optimistic recoil is
        // the raw value times the smallest aim scale any part can apply.
        if self.aim_scale.enabled {
            if let Some(v) = f_h {
                if self.aim_scale.h_factor >= 0.0 {
                    f_h = Some(v * self.aim_scale.h_factor - self.aim_scale.h_margin);
                }
            }
            if let Some(v) = f_v {
                if self.aim_scale.v_factor >= 0.0 {
                    f_v = Some(v * self.aim_scale.v_factor - self.aim_scale.v_margin);
                }
            }
        }
        let mut s = (gk >= 0.0 && positive(xk, bound.min_c_k))
            .then(|| gk / self.gunkick0 * xk * bound.min_c_k);
        // `fire-stability` is `round2(round2(p0 * s) / round2(p0))`; the
        // optimistic bound has to allow the rounding to move the value up.
        if let Some(v) = s {
            let rounded0 = ((self.gunkick0 + f64::EPSILON) * 100.0).round() / 100.0;
            if rounded0 != 0.0 {
                s = Some(v - 0.005 / rounded0.abs() - 0.005);
            }
        }
        for c in &self.search_constraints {
            let Some(max) = c.max else { continue };
            let value = match c.id.as_str() {
                "recoil-continuous-horizontal" => f_h,
                "recoil-continuous-vertical" => f_v,
                "recoil-fire-stability" => s,
                _ => continue,
            };
            if let Some(value) = value {
                if value > max + 1e-9 {
                    return true;
                }
            }
        }
        false
    }

    fn cost_lower_bound(
        &self,
        _selections: &FxHashMap<String, Option<String>>,
        assembly: &crate::assembly::Assembly,
        decided: &FxHashSet<String>,
    ) -> i64 {
        let mut cost = 0i64;
        let mut seen: FxHashSet<(&str, &str)> = FxHashSet::default();
        for slot in &assembly.visible_slots {
            if !decided.contains(&slot.path_key) {
                continue;
            }
            let Some(id) = slot.selected_part_id.as_ref() else {
                continue;
            };
            if !seen.insert((slot.path_key.as_str(), id.as_str())) {
                continue;
            }
            if let Some(part) = self.payload.parts.get(id) {
                cost += self.dp_part_price(part);
            }
        }
        if self.options.mode == "blackhawk" {
            for item in &assembly.internal_parts {
                if !decided.contains(&item.slot_path_key) {
                    continue;
                }
                if !seen.insert((item.slot_path_key.as_str(), item.part_id.as_str())) {
                    continue;
                }
                if let Some(part) = self.payload.parts.get(&item.part_id) {
                    cost += self.dp_part_price(part);
                }
            }
        }
        cost
    }

    fn part_price(&self, id: &str) -> i64 {
        self.part_prices.get(id).copied().unwrap_or(0)
    }

    fn seed_metric_score(&self, metric: &ExactPart) -> f64 {
        seed_metric_rank(metric, &self.seed_panel_attrs, self.seed_aim)
    }

    fn seed_part_score(&mut self, id: &str, slot_name: &str) -> f64 {
        let payload = self.payload;
        let Some(part) = payload.parts.get(id) else {
            return f64::NEG_INFINITY;
        };
        let metric = self
            .dp_option_for_part(part, slot_name, SlotChoice::Part(id.to_string()), &[], true)
            .metric;
        self.seed_metric_score(&metric)
    }

    fn dp_level_may_satisfy(&self, st: &DpState, sb: &DpSuffixBound) -> bool {
        let mut has_recoil = false;
        let mut aim_max: Option<f64> = None;
        for c in &self.search_constraints {
            let Some(max) = c.max else { continue };
            match c.id.as_str() {
                "recoil-continuous-horizontal"
                | "recoil-single-horizontal"
                | "recoil-continuous-vertical"
                | "recoil-single-vertical"
                | "recoil-fire-stability" => has_recoil = true,
                "handling-aim-time" if c.min.is_none() => aim_max = Some(max),
                _ => {}
            }
        }
        if has_recoil {
            if let (Some(control), Some(gunkick)) = (&self.control_curve, &self.gunkick_curve) {
                if self.gunkick0 > 0.0 {
                    let d_hi = st.metric.d + sb.d_max;
                    let (cr, gk) = if self.curves_nonincreasing {
                        (control.eval(d_hi), gunkick.eval(d_hi))
                    } else {
                        (self.control_min, self.gunkick_min)
                    };
                    if let (Some(cr), Some(gk)) = (cr, gk) {
                        let c_h = if st.metric.ch < 0.0 || sb.ch_negative {
                            0.0
                        } else {
                            (st.metric.ch * sb.ch_min).max(0.0)
                        };
                        let c_v = if st.metric.cv < 0.0 || sb.cv_negative {
                            0.0
                        } else {
                            (st.metric.cv * sb.cv_min).max(0.0)
                        };
                        let c_k = if st.metric.ck < 0.0 || sb.ck_negative {
                            0.0
                        } else {
                            (st.metric.ck * sb.ck_min).max(0.0)
                        };
                        let mut f_h = cr.max(0.0)
                            * (1.0 + st.metric.ah + sb.ah_min - 0.01 * (st.metric.eh + sb.eh_max))
                                .max(0.0)
                            * c_h;
                        let mut f_v = cr.max(0.0)
                            * (1.0 + st.metric.av + sb.av_min - 0.01 * (st.metric.ev + sb.ev_max))
                                .max(0.0)
                            * c_v;
                        if self.aim_scale.enabled {
                            f_h = f_h * self.aim_scale.h_factor - self.aim_scale.h_margin;
                            f_v = f_v * self.aim_scale.v_factor - self.aim_scale.v_margin;
                        }
                        let mut s = (gk / self.gunkick0).max(0.0)
                            * (1.0 - 0.01 * (st.metric.ek + sb.ek_max)).max(0.0)
                            * c_k;
                        let rounded0 = ((self.gunkick0 + f64::EPSILON) * 100.0).round() / 100.0;
                        if rounded0 != 0.0 {
                            s -= 0.005 / rounded0.abs() + 0.005;
                        }
                        for c in &self.search_constraints {
                            let Some(max) = c.max else { continue };
                            if max < 0.0 {
                                continue;
                            }
                            let value = match c.id.as_str() {
                                "recoil-continuous-horizontal" | "recoil-single-horizontal" => f_h,
                                "recoil-continuous-vertical" | "recoil-single-vertical" => f_v,
                                "recoil-fire-stability" => s,
                                _ => continue,
                            };
                            if value > max + 1e-9 {
                                return false;
                            }
                        }
                    }
                }
            }
        }
        if let Some(max) = aim_max {
            let open = (st.metric.eopen + sb.eopen_max).max(0.0);
            if let Some(v) = self.evaluator.aim_time(
                st.metric.hs + sb.hs_max,
                st.metric.adsm + sb.adsm_min,
                open,
            ) {
                if v > max + 1e-9 {
                    return false;
                }
            }
        }
        true
    }

    fn dp_leaf(
        &mut self,
        selections: &FxHashMap<String, Option<String>>,
        decided: &FxHashSet<String>,
        assembly: &crate::assembly::Assembly,
    ) {
        let mut leaf_slots: Vec<&VisibleSlot> = Vec::new();
        for slot in &assembly.visible_slots {
            if decided.contains(&slot.path_key) {
                continue;
            }
            if !self.relevance.contains(&slot.host_part_id, &slot.slot.id) {
                continue;
            }
            leaf_slots.push(slot);
        }
        if leaf_slots.is_empty() || !self.dp_ok {
            let mut collapsed = selections.clone();
            for slot in &assembly.visible_slots {
                if !decided.contains(&slot.path_key) {
                    collapsed.insert(slot.path_key.clone(), None);
                }
            }
            self.leaf(&collapsed);
            return;
        }
        let leaf_paths: FxHashSet<String> = leaf_slots.iter().map(|s| s.path_key.clone()).collect();
        let is_under_leaf = |path: &str| {
            leaf_paths.iter().any(|leaf| {
                path.len() > leaf.len()
                    && path.starts_with(leaf.as_str())
                    && path[leaf.len()..].starts_with("/part:")
            })
        };
        let mut base_trial = selections.clone();
        for slot in &assembly.visible_slots {
            if !decided.contains(&slot.path_key) && !leaf_paths.contains(&slot.path_key) {
                base_trial.entry(slot.path_key.clone()).or_insert(None);
            }
        }

        let mut base = ExactPart::neutral_panel(self.panel_attrs.len());
        let mut base_cost = 0i64;
        let mut base_parts: Vec<(&Part, &str, bool)> = Vec::new();
        let mut seen_paths: FxHashSet<(&str, &str)> = FxHashSet::default();
        let base_assembly = self.assembler.resolve(&base_trial);
        for slot in &base_assembly.visible_slots {
            if leaf_paths.contains(&slot.path_key) || is_under_leaf(&slot.path_key) {
                continue;
            }
            let Some(id) = slot.selected_part_id.as_ref() else {
                continue;
            };
            if !seen_paths.insert((slot.path_key.as_str(), id.as_str())) {
                continue;
            }
            if let Some(part) = self.payload.parts.get(id) {
                base_parts.push((part, &slot.slot.name, true));
                base_cost += self.dp_part_price(part);
            }
        }
        for item in &base_assembly.internal_parts {
            if leaf_paths.contains(&item.slot_path_key) || is_under_leaf(&item.slot_path_key) {
                continue;
            }
            if !seen_paths.insert((item.slot_path_key.as_str(), item.part_id.as_str())) {
                continue;
            }
            if let Some(part) = self.payload.parts.get(&item.part_id) {
                base_parts.push((part, &item.slot.name, false));
                if self.options.mode == "blackhawk" {
                    base_cost += self.dp_part_price(part);
                }
            }
        }
        let mode = &self.options.mode;
        let mut base_detail_pro: Option<Rc<LaserPro>> = None;
        let mut base_panel_pro: Option<Rc<LaserPro>> = None;
        for (part, slot_name, visible) in &base_parts {
            let is_laser = crate::metrics::has_laser(part);
            let m = exact_part_metric(
                part,
                self.evaluator.weapon_id,
                slot_name,
                mode,
                self.options.bipod_deployed,
                is_laser,
                false,
                *visible,
                &self.panel_attrs,
            );
            base.add(&m);
            if is_laser && self.options.laser_enabled {
                let score = crate::metrics::laser_pro_score(part, mode);
                let pro = Rc::new(LaserPro {
                    score,
                    metric: exact_laser_pro(
                        part,
                        self.evaluator.weapon_id,
                        mode,
                        &self.panel_attrs,
                    ),
                });
                // The detail path ranks the laser over every installed part,
                // while the panel only sees visible selected parts.
                if base_detail_pro
                    .as_ref()
                    .map(|cur| score > cur.score)
                    .unwrap_or(true)
                {
                    base_detail_pro = Some(pro.clone());
                }
                if *visible
                    && base_panel_pro
                        .as_ref()
                        .map(|cur| score > cur.score)
                        .unwrap_or(true)
                {
                    base_panel_pro = Some(pro);
                }
            }
        }
        if let Some(pro) = &base_detail_pro {
            base.add_laser_pro_detail(&pro.metric);
        }
        if let Some(pro) = &base_panel_pro {
            base.add_laser_pro_panel(&pro.metric);
        }

        let mut options_per_slot: Vec<Vec<DpOption>> = Vec::new();
        let mut slot_paths: Vec<String> = Vec::new();
        for slot in &leaf_slots {
            let path = slot.path_key.clone();
            let mut opts: Vec<DpOption> = Vec::new();
            let unset_part = slot.internal_part_id.clone();
            if let Some(part) = unset_part
                .as_ref()
                .and_then(|id| self.payload.parts.get(id))
            {
                opts.push(self.dp_option_for_part(
                    part,
                    &slot.slot.name,
                    SlotChoice::Unset,
                    &opts,
                    false,
                ));
            } else {
                opts.push(DpOption {
                    cost: 0,
                    metric: ExactPart::neutral_panel(self.panel_attrs.len()),
                    laser_pro: None,
                    visible: false,
                    choice: SlotChoice::Unset,
                });
            }
            let remove_part = if slot.internal_is_default {
                self.effective_none_part(selections, &path)
            } else {
                unset_part.clone()
            };
            let remove_structural = remove_part
                .as_ref()
                .map(|id| self.structural_parts.contains(id))
                .unwrap_or(false);
            if remove_part.as_deref() != unset_part.as_deref() && !remove_structural {
                if let Some(part) = remove_part
                    .as_ref()
                    .and_then(|id| self.payload.parts.get(id))
                {
                    opts.push(self.dp_option_for_part(
                        part,
                        &slot.slot.name,
                        SlotChoice::Remove,
                        &opts,
                        false,
                    ));
                } else {
                    opts.push(DpOption {
                        cost: 0,
                        metric: ExactPart::neutral_panel(self.panel_attrs.len()),
                        laser_pro: None,
                        visible: false,
                        choice: SlotChoice::Remove,
                    });
                }
            }
            for id in &slot.candidate_ids {
                if self.options.excluded_part_ids.contains(id) {
                    continue;
                }
                if self.structural_parts.contains(id) {
                    continue;
                }
                if Some(id.as_str()) == unset_part.as_deref() {
                    continue;
                }
                if let Some(part) = self.payload.parts.get(id) {
                    let choice = SlotChoice::Part(id.clone());
                    if !opts.iter().any(|o| o.choice == choice) {
                        opts.push(self.dp_option_for_part(
                            part,
                            &slot.slot.name,
                            choice,
                            &opts,
                            true,
                        ));
                    }
                }
            }
            opts.sort_by(|a, b| {
                a.cost.cmp(&b.cost).then(
                    b.metric
                        .score()
                        .partial_cmp(&a.metric.score())
                        .unwrap_or(Ordering::Equal),
                )
            });
            opts.dedup_by(|a, b| a.choice == b.choice);
            options_per_slot.push(opts);
            slot_paths.push(path);
        }

        let mut suffix_min = vec![0i64; options_per_slot.len() + 1];
        for (idx, opts) in options_per_slot.iter().enumerate().rev() {
            let min_cost = opts.iter().map(|o| o.cost).min().unwrap_or(0);
            suffix_min[idx] = suffix_min[idx + 1] + min_cost;
        }
        let suffix_bound = dp_suffix_bounds(&options_per_slot);

        // Admissible aim-time bounds for the slots that are not placed yet.
        // Pruning on them early only drops states that no completion can make
        // feasible, and it keeps the frontier small when tuning couples recoil
        // control with aim speed.
        let aim_max = self
            .options
            .constraints
            .iter()
            .find(|c| c.id == "handling-aim-time" && c.min.is_none())
            .and_then(|c| c.max);
        let mut suffix_hs_max = vec![0.0; options_per_slot.len() + 1];
        let mut suffix_adsm_min = vec![0.0; options_per_slot.len() + 1];
        let mut suffix_open_max = vec![0.0; options_per_slot.len() + 1];
        if aim_max.is_some() {
            for (idx, opts) in options_per_slot.iter().enumerate().rev() {
                let mut hs_max = f64::NEG_INFINITY;
                let mut adsm_min = f64::INFINITY;
                let mut open_max = f64::NEG_INFINITY;
                for opt in opts {
                    hs_max = hs_max.max(opt.metric.hs);
                    adsm_min = adsm_min.min(opt.metric.adsm);
                    open_max = open_max.max(opt.metric.eopen);
                }
                if !hs_max.is_finite() || !adsm_min.is_finite() || !open_max.is_finite() {
                    continue;
                }
                suffix_hs_max[idx] = suffix_hs_max[idx + 1] + hs_max;
                suffix_adsm_min[idx] = suffix_adsm_min[idx + 1] + adsm_min;
                suffix_open_max[idx] = suffix_open_max[idx + 1] + open_max;
            }
        }

        if self.seed
            && self.options.first_solution
            && self.best_cost == i64::MAX
            && !self.seed_beam_tried
        {
            self.seed_beam_tried = true;
            if self.seed_beam_incumbent(
                &base,
                base_cost,
                &options_per_slot,
                &slot_paths,
                &base_trial,
            ) {
                return;
            }
        }

        let mut states: Vec<DpState> = vec![DpState {
            cost: base_cost,
            metric: base,
            detail_pro: base_detail_pro,
            panel_pro: base_panel_pro,
            choices: Vec::new(),
        }];
        for (slot_idx, opts) in options_per_slot.iter().enumerate() {
            let rest_cost = suffix_min[slot_idx + 1];
            let mut next: Vec<DpState> =
                Vec::with_capacity(states.len().saturating_mul(opts.len()).min(200_000));
            for st in &states {
                for opt in opts {
                    let cost = st.cost + opt.cost;
                    let total = cost + self.weapon_price;
                    if self.cost_cap < i64::MAX && total > self.cost_cap {
                        continue;
                    }
                    if self.best_cost < i64::MAX && total >= self.best_cost {
                        continue;
                    }
                    if total + rest_cost > self.cost_cap && self.cost_cap < i64::MAX {
                        continue;
                    }
                    if self.best_cost < i64::MAX && total + rest_cost >= self.best_cost {
                        continue;
                    }
                    let (metric, detail_pro, panel_pro) = dp_metric_advance(st, opt);
                    if let Some(max) = aim_max {
                        if let Some(best) = self.evaluator.aim_time(
                            metric.hs + suffix_hs_max[slot_idx + 1],
                            metric.adsm + suffix_adsm_min[slot_idx + 1],
                            (metric.eopen + suffix_open_max[slot_idx + 1]).max(0.0),
                        ) {
                            if best > max + 1e-9 {
                                continue;
                            }
                        }
                    }
                    let mut choices = st.choices.clone();
                    choices.push(opt.choice.clone());
                    next.push(DpState {
                        cost,
                        metric,
                        detail_pro,
                        panel_pro,
                        choices,
                    });
                }
            }
            next.retain(|st| self.dp_level_may_satisfy(st, &suffix_bound[slot_idx + 1]));
            // The seed pass only has to find one feasible build, so it keeps a
            // small capability-ranked frontier. The optimal pass needs its
            // cost-ranked frontier intact.
            let dp_limit = if self.seed { 8192 } else { 200_000 };
            if !self.seed && self.frontier_abort.is_some() {
                self.frontier_abort_not_before = Some(now_ms() + FRONTIER_ABORT_LEVEL_MS);
            }
            let (pruned, cut) = dp_pareto_capped(
                next,
                &self.dp_dims,
                &self.panel_dirs,
                self.aim_dp,
                dp_limit,
                self.deadline_at,
                self.seed,
                if self.seed { None } else { self.frontier_abort },
                self.frontier_abort_not_before,
            );
            if cut.aborted {
                self.aborted_pass = true;
                self.stop = true;
                return;
            }
            self.heuristic_incomplete |= cut.capped;
            self.truncated |= cut.deadline;
            next = pruned;
            if self.truncated && self.deadline_at.is_some_and(|d| now_ms() >= d) {
                self.stop = true;
                return;
            }
            states = next;
            if states.is_empty() {
                return;
            }
        }

        if self.seed {
            let mut ranked: Vec<(f64, i64, DpState)> = states
                .into_iter()
                .map(|st| {
                    let rank = self.seed_metric_score(&st.metric);
                    (rank, st.cost, st)
                })
                .collect();
            ranked.sort_by(|a, b| {
                b.0.partial_cmp(&a.0)
                    .unwrap_or(Ordering::Equal)
                    .then_with(|| a.1.cmp(&b.1))
            });
            states = ranked.into_iter().map(|(_, _, st)| st).collect();
        } else {
            states.sort_by_key(|s| s.cost);
        }
        let (h_max, v_max, k_max) = recoil_maxima(&self.search_constraints);
        let aim_max = aim_time_max(&self.search_constraints);
        for st in states {
            if self.budget_exceeded() {
                return;
            }
            if optimistic_recoil_exceeds(
                &st.metric,
                h_max,
                v_max,
                k_max,
                self.control_curve.as_ref(),
                self.gunkick_curve.as_ref(),
                self.gunkick0,
                &self.aim_scale,
            ) {
                continue;
            }
            if let Some(max) = aim_max {
                if let Some(best) =
                    self.evaluator
                        .aim_time(st.metric.hs, st.metric.adsm, st.metric.eopen.max(0.0))
                {
                    if best > max + 1e-9 {
                        continue;
                    }
                }
            }
            let total = st.cost + self.weapon_price;
            if self.best_cost < i64::MAX && total >= self.best_cost {
                continue;
            }
            let mut trial = base_trial.clone();
            for (path, choice) in slot_paths.iter().zip(st.choices.iter()) {
                match choice {
                    SlotChoice::Unset => {
                        trial.remove(path);
                    }
                    SlotChoice::Remove => {
                        trial.insert(path.clone(), None);
                    }
                    SlotChoice::Part(id) => {
                        trial.insert(path.clone(), Some(id.clone()));
                    }
                }
            }
            self.leaf(&trial);
            if self.stop {
                return;
            }
        }
    }

    /// Cheap constraint-directed incumbent for first-solution mode.
    ///
    /// A narrow beam keeps only the states with the best optimistic bound
    /// slack at every level, then evaluates the cheapest optimistically
    /// feasible members of the final beam. The beam never prunes the exact
    /// search: it only calls `leaf`, which validates every constraint, and
    /// the regular Pareto DP runs whenever no candidate is feasible.
    fn seed_beam_incumbent(
        &mut self,
        base: &ExactPart,
        base_cost: i64,
        options_per_slot: &[Vec<DpOption>],
        slot_paths: &[String],
        base_trial: &FxHashMap<String, Option<String>>,
    ) -> bool {
        const BEAM_WIDTH: usize = 1024;
        const EVAL_LIMIT: usize = 16;
        if options_per_slot.is_empty() {
            return false;
        }
        let (h_max, v_max, k_max) = recoil_maxima(&self.search_constraints);
        let aim_max = aim_time_max(&self.search_constraints);
        if h_max.is_none() && v_max.is_none() && k_max.is_none() && aim_max.is_none() {
            return false;
        }
        let mut beam: Vec<DpState> = vec![DpState {
            cost: base_cost,
            metric: base.clone(),
            detail_pro: None,
            panel_pro: None,
            choices: Vec::new(),
        }];
        for opts in options_per_slot {
            let mut scored: Vec<(f64, f64, i64, DpState)> =
                Vec::with_capacity(beam.len().saturating_mul(opts.len()).min(BEAM_WIDTH * 8));
            for st in &beam {
                for opt in opts {
                    let cost = st.cost + opt.cost;
                    let total = cost + self.weapon_price;
                    if self.cost_cap < i64::MAX && total > self.cost_cap {
                        continue;
                    }
                    let mut metric = st.metric.clone();
                    metric.add(&opt.metric);
                    let mut choices = st.choices.clone();
                    choices.push(opt.choice.clone());
                    let margin = seed_candidate_margin(
                        &metric,
                        h_max,
                        v_max,
                        k_max,
                        aim_max,
                        self.control_curve.as_ref(),
                        self.gunkick_curve.as_ref(),
                        self.gunkick0,
                        &self.aim_scale,
                        &self.evaluator,
                    );
                    let rank = self.seed_metric_score(&metric);
                    scored.push((
                        margin,
                        rank,
                        cost,
                        DpState {
                            cost,
                            metric,
                            detail_pro: None,
                            panel_pro: None,
                            choices,
                        },
                    ));
                }
            }
            scored.sort_by(|a, b| match (a.0 >= 0.0, b.0 >= 0.0) {
                (true, true) => {
                    a.2.cmp(&b.2)
                        .then_with(|| b.0.partial_cmp(&a.0).unwrap_or(Ordering::Equal))
                        .then_with(|| b.1.partial_cmp(&a.1).unwrap_or(Ordering::Equal))
                }
                (false, false) => {
                    b.0.partial_cmp(&a.0)
                        .unwrap_or(Ordering::Equal)
                        .then_with(|| b.1.partial_cmp(&a.1).unwrap_or(Ordering::Equal))
                        .then_with(|| a.2.cmp(&b.2))
                }
                (true, false) => Ordering::Less,
                (false, true) => Ordering::Greater,
            });
            scored.truncate(BEAM_WIDTH);
            beam = scored.into_iter().map(|(_, _, _, st)| st).collect();
            if beam.is_empty() {
                return false;
            }
        }
        let margins: Vec<f64> = beam
            .iter()
            .map(|st| {
                seed_candidate_margin(
                    &st.metric,
                    h_max,
                    v_max,
                    k_max,
                    aim_max,
                    self.control_curve.as_ref(),
                    self.gunkick_curve.as_ref(),
                    self.gunkick0,
                    &self.aim_scale,
                    &self.evaluator,
                )
            })
            .collect();
        for idx in incumbent_eval_order(&beam, &margins)
            .into_iter()
            .take(EVAL_LIMIT)
        {
            if self.stop {
                return self.best.is_some();
            }
            let st = &beam[idx];
            if optimistic_recoil_exceeds(
                &st.metric,
                h_max,
                v_max,
                k_max,
                self.control_curve.as_ref(),
                self.gunkick_curve.as_ref(),
                self.gunkick0,
                &self.aim_scale,
            ) {
                continue;
            }
            if let Some(max) = aim_max {
                if let Some(best) =
                    self.evaluator
                        .aim_time(st.metric.hs, st.metric.adsm, st.metric.eopen.max(0.0))
                {
                    if best > max + 1e-9 {
                        continue;
                    }
                }
            }
            let mut trial = base_trial.clone();
            for (path, choice) in slot_paths.iter().zip(st.choices.iter()) {
                match choice {
                    SlotChoice::Unset => {
                        trial.remove(path);
                    }
                    SlotChoice::Remove => {
                        trial.insert(path.clone(), None);
                    }
                    SlotChoice::Part(id) => {
                        trial.insert(path.clone(), Some(id.clone()));
                    }
                }
            }
            self.leaf(&trial);
            if self.stop || self.best.is_some() {
                return true;
            }
        }
        false
    }

    fn dp_part_price(&self, part: &Part) -> i64 {
        if part.is_virtual {
            0
        } else {
            self.part_prices.get(&part.id).copied().unwrap_or(0)
        }
    }

    fn dp_option_for_part(
        &mut self,
        part: &Part,
        slot_name: &str,
        choice: SlotChoice,
        existing: &[DpOption],
        visible_choice: bool,
    ) -> DpOption {
        let mode = &self.options.mode;
        let is_laser = crate::metrics::has_laser(part);
        let key = (part.id.clone(), slot_name.to_string(), visible_choice);
        let metric = if let Some(m) = self.dp_metric_cache.get(&key) {
            m.clone()
        } else {
            let m = exact_part_metric(
                part,
                self.evaluator.weapon_id,
                slot_name,
                mode,
                self.options.bipod_deployed,
                is_laser,
                false,
                visible_choice,
                &self.panel_attrs,
            );
            self.dp_metric_cache.insert(key, m.clone());
            m
        };
        // A laser part only contributes its `pro` modifiers when the evaluator
        // selects it as the single best laser of the assembly, so the option
        // carries the pro delta separately and the DP state applies it once.
        // Internal parts compete for the detail slot as well; `visible` below
        // keeps them out of the panel slot.
        let laser_pro = if is_laser && self.options.laser_enabled {
            Some(Rc::new(LaserPro {
                score: crate::metrics::laser_pro_score(part, mode),
                metric: exact_laser_pro(part, self.evaluator.weapon_id, mode, &self.panel_attrs),
            }))
        } else {
            None
        };
        let cost = if visible_choice || self.options.mode == "blackhawk" {
            self.dp_part_price(part)
        } else {
            0
        };
        let _ = existing;
        DpOption {
            cost,
            metric,
            laser_pro,
            visible: visible_choice,
            choice,
        }
    }

    /// Whether the resolved assembly still honours every locked path. The
    /// resolver silently drops a locked part when an earlier slot claims its
    /// socket through `extraOccupySocketIds`, so the raw selection map alone is
    /// no evidence that the lock was installed.
    ///
    /// All-virtual slots never produce a visible slot even when their virtual
    /// part is installed; those show up as internal parts instead.
    fn locks_honoured(&self, assembly: &crate::assembly::Assembly) -> bool {
        self.options.locked_paths.iter().all(|(path, expected)| {
            let visible = assembly
                .visible_slots
                .iter()
                .find(|slot| &slot.path_key == path);
            let internal = assembly
                .internal_parts
                .iter()
                .find(|part| &part.slot_path_key == path);
            let installed = visible
                .and_then(|slot| {
                    slot.selected_part_id
                        .as_ref()
                        .or(slot.internal_part_id.as_ref())
                })
                .or_else(|| internal.map(|part| &part.part_id));
            match expected {
                Some(expected) => installed == Some(expected),
                None => visible.is_some() && installed.is_none(),
            }
        })
    }

    fn effective_none_part(
        &self,
        selections: &FxHashMap<String, Option<String>>,
        path: &str,
    ) -> Option<String> {
        let mut trial = selections.clone();
        trial.insert(path.to_string(), None);
        let assembly = self.assembler.resolve(&trial);
        for slot in &assembly.visible_slots {
            if slot.path_key == path {
                return slot
                    .selected_part_id
                    .clone()
                    .or_else(|| slot.internal_part_id.clone());
            }
        }
        None
    }

    fn leaf(&mut self, selections: &FxHashMap<String, Option<String>>) {
        if self.stop {
            return;
        }
        if self.seed_continue_exhausted() {
            self.stop = true;
            return;
        }
        self.leaves += 1;
        if self.budget_exceeded() {
            return;
        }
        let assembly = self.assembler.resolve(selections);
        if !self.locks_honoured(&assembly) {
            return;
        }
        let installed: Vec<&Part> = self.evaluator.installed_parts(&assembly);
        let rec = self.recoil_base(&installed);
        if rec.is_none()
            && self
                .options
                .constraints
                .iter()
                .any(|c| is_recoil_metric(&c.id))
        {
            return;
        }
        let (attachment_price, total_price, _) = self.evaluator.price(&assembly);
        if total_price >= self.best_cost || total_price > self.cost_cap {
            return;
        }

        let instances = self.evaluator.tunable_instances(&assembly);
        let req = match &rec {
            Some(rec) => match self.tune_requirements(rec) {
                Some(r) => r,
                None => return,
            },
            None => TuneReq::unconstrained(),
        };
        let mut excluded: Vec<Vec<(String, f64)>> = Vec::new();
        let (chosen, metrics, constraint_results) = loop {
            let Some((tuning_state, chosen)) =
                self.pick_tuning(&instances, &req, &installed, &excluded)
            else {
                return;
            };

            let mut constraint_results = Vec::new();
            let mut ok = true;
            let metrics = self
                .evaluator
                .evaluate(&self.assembler, &assembly, &tuning_state);
            if let Some(metrics) = metrics {
                for c in &self.options.constraints {
                    let Some(value) = metric_value(c.id.as_str(), &metrics) else {
                        return;
                    };
                    let min_ok = c.min.map(|m| value >= m - 1e-9).unwrap_or(true);
                    let max_ok = c.max.map(|m| value <= m + 1e-9).unwrap_or(true);
                    let ok_this = min_ok && max_ok;
                    ok &= ok_this;
                    constraint_results.push(ConstraintResult {
                        id: c.id.clone(),
                        value,
                        min: c.min,
                        max: c.max,
                        ok: ok_this,
                    });
                }
                if ok {
                    break (chosen, metrics, constraint_results);
                }
            }
            // The frontier can rank a grid value that sits exactly on the open
            // edge of a rounded requirement band. The exact evaluation above is
            // authoritative, so try the next candidate before discarding the
            // whole assembly.
            if chosen.is_empty() {
                return;
            }
            excluded.push(chosen);
            if excluded.len() >= TUNE_PICK_ATTEMPTS {
                return;
            }
        };

        let tuning = chosen
            .iter()
            .map(|(k, v)| (k.clone(), *v))
            .collect::<FxHashMap<_, _>>();
        let installed_parts = self.installed_list(&assembly);
        let mut installed_part_keys: Vec<String> = installed_parts
            .iter()
            .map(|p| format!("{}\0{}", p.slot_path, p.part_id))
            .collect();
        installed_part_keys.sort();
        let solution = Solution {
            attachment_price,
            total_price,
            recoil: RecoilSummary {
                score: metrics.recoil.d,
                continuous_h: metrics.recoil.continuous_h,
                continuous_v: metrics.recoil.continuous_v,
                single_h: metrics.recoil.single_h,
                single_v: metrics.recoil.single_v,
                fire_stability: metrics.recoil.fire_stability,
                d: metrics.recoil.d,
                f_h: metrics.recoil.f_h,
                f_v: metrics.recoil.f_v,
                s: metrics.recoil.s,
                a_h: rec.as_ref().map(|r| r.a_h).unwrap_or(0.0),
                a_v: rec.as_ref().map(|r| r.a_v).unwrap_or(0.0),
                c_h: rec.as_ref().map(|r| r.c_h).unwrap_or(1.0),
                c_v: rec.as_ref().map(|r| r.c_v).unwrap_or(1.0),
                c_k: rec.as_ref().map(|r| r.c_k).unwrap_or(1.0),
                t_h: metrics
                    .recoil
                    .tuning_runtime
                    .get("GRecoil_H")
                    .copied()
                    .unwrap_or(0.0),
                t_v: metrics
                    .recoil
                    .tuning_runtime
                    .get("GRecoil_V")
                    .copied()
                    .unwrap_or(0.0),
                t_k: metrics
                    .recoil
                    .tuning_runtime
                    .get("GGunkickSpring")
                    .copied()
                    .unwrap_or(0.0),
            },
            panel: metrics.panel,
            detail: metrics.detail,
            tuning,
            constraints: constraint_results,
            installed_parts,
            installed_part_keys,
            selections: selections.clone(),
        };
        self.best_cost = total_price;
        if self.best.is_some() {
            let prev = self.best.replace(solution);
            if let Some(prev) = prev {
                self.push_alternative(prev);
            }
        } else {
            self.best = Some(solution);
        }
        self.run_found = true;
        if self.seed {
            if self.seed_continue {
                self.seed_continue_until = self.work() + SEED_IMPROVE_WORK;
            } else {
                self.stop = true;
            }
        }
    }

    fn push_alternative(&mut self, solution: Solution) {
        if self.options.max_results == 0 {
            return;
        }
        if self.alternatives.iter().any(|existing| {
            existing.total_price == solution.total_price
                && existing.installed_part_keys == solution.installed_part_keys
                && existing.tuning == solution.tuning
        }) {
            return;
        }
        let limit = self.options.max_results;
        if self.alternatives.len() >= limit
            && self
                .alternatives
                .last()
                .map(|s| s.total_price <= solution.total_price)
                .unwrap_or(false)
        {
            return;
        }
        let pos = self
            .alternatives
            .binary_search_by(|s| {
                s.total_price
                    .cmp(&solution.total_price)
                    .then_with(|| s.installed_parts.len().cmp(&solution.installed_parts.len()))
            })
            .unwrap_or_else(|p| p);
        self.alternatives.insert(pos, solution);
        if self.alternatives.len() > limit {
            self.alternatives.pop();
        }
    }

    fn recoil_base(&self, parts: &[&Part]) -> Option<BaseRecoil> {
        let profile = self
            .payload
            .numeric_profiles
            .get(self.evaluator.weapon_id)?;
        let mode = &self.options.mode;
        let mut d = 0.0;
        let mut a_h = 0.0;
        let mut a_v = 0.0;
        let mut c_h = 1.0;
        let mut c_v = 1.0;
        let mut c_k = 1.0;
        let mut laser_parts: Vec<(usize, f64)> = Vec::new();
        for (idx, part) in parts.iter().enumerate() {
            if crate::metrics::has_laser(part) {
                // Primary-laser ranking mirrors site `ct`: raw `modifiersByMode`
                // only, never the baseline recovery list.
                let score: f64 = part
                    .display_mods
                    .get(mode)
                    .map(|list| {
                        list.iter()
                            .filter(|m| {
                                m.condition_id.is_none()
                                    && m.modifier_type == "Addend"
                                    && m.polarity.as_deref() == Some("pro")
                            })
                            .map(|m| m.delta.map(|v| v.0.abs()).unwrap_or(0.0))
                            .sum()
                    })
                    .unwrap_or(0.0);
                laser_parts.push((idx, score));
            }
        }
        let laser_idx = laser_parts
            .iter()
            .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(Ordering::Equal))
            .map(|(i, _)| *i);

        for (idx, part) in parts.iter().enumerate() {
            let laser_part = crate::metrics::has_laser(part);
            let selected_laser = laser_idx == Some(idx);
            for m in part.main_mods_for(self.evaluator.weapon_id, mode) {
                if m.condition_id.is_some() || m.modifier_type != "Addend" {
                    continue;
                }
                if laser_part {
                    if m.polarity.as_deref() != Some("con")
                        && !(self.options.laser_enabled && selected_laser)
                    {
                        continue;
                    }
                }
                if normalize_attr(m.attribute_id.as_deref()) == "10005" {
                    d += m.delta.map(|v| v.0).unwrap_or(0.0);
                }
            }
            if let Some(rr) = part.raw_recoil.get(mode) {
                if let Some(v) = rr.horizontal {
                    if v.is_finite() {
                        a_h += v;
                    }
                }
                if let Some(v) = rr.vertical {
                    if v.is_finite() {
                        a_v += v;
                    }
                }
            }
            let bipod = is_bipod(part);
            let mut explicit_spring = false;
            let mut explicit_c_h = false;
            let mut explicit_c_v = false;
            for m in part.raw_mods.get(mode).map(|v| v.as_slice()).unwrap_or(&[]) {
                if m.modifier_type != "Mult_C" {
                    continue;
                }
                let Some(p) = m.param1.map(|v| v.0) else {
                    continue;
                };
                if !p.is_finite() {
                    continue;
                }
                if bipod && !self.options.bipod_deployed && is_recoil_target(&m.target_path) {
                    continue;
                }
                match m.target_path.as_str() {
                    "GRecoil_H" => {
                        c_h *= p;
                        explicit_c_h = true;
                    }
                    "GRecoil_V" => {
                        c_v *= p;
                        explicit_c_v = true;
                    }
                    "GGunkickSpring" => {
                        c_k *= p;
                        explicit_spring = true;
                    }
                    _ => {}
                }
            }
            if (!bipod || self.options.bipod_deployed) && !explicit_spring {
                if let Some(k) = part
                    .raw_recoil
                    .get(mode)
                    .and_then(|r| r.gunkick_spring_mult_c)
                {
                    if k.is_finite() {
                        c_k *= 1.0 + k;
                    }
                }
            }
            let _ = (explicit_c_h, explicit_c_v);
        }

        let curves = &profile.linked_tables.recoil_curves;
        let control = if mode == "warfare" {
            curves
                .recoil_control_mp
                .as_ref()
                .or(curves.recoil_control.as_ref())
        } else {
            curves.recoil_control.as_ref()
        }?;
        let gunkick = if mode == "warfare" {
            curves.gunkick_mp.as_ref().or(curves.gunkick.as_ref())
        } else {
            curves.gunkick.as_ref()
        }?;
        let cr = control.eval(d)?;
        let gk = gunkick.eval(d)?;
        let gk0 = gunkick.eval(0.0)?;
        if gk0 == 0.0 {
            return None;
        }
        // The evaluator replaces the aim profile with the last installed part
        // that provides one, then scales raw recoil by override / base.
        let base_aim = &profile.linked_tables.aim_recoil;
        let mut aim = base_aim;
        for part in parts {
            if let Some(over) = part.aim_recoil_profiles.get(mode) {
                aim = over;
            }
        }
        Some(BaseRecoil {
            a_h,
            a_v,
            c_h,
            c_v,
            c_k,
            cr,
            gk_ratio: gk / gk0,
            cont_h_scale: scale_factor(
                base_aim.continue_shoot.horizontal_scale,
                aim.continue_shoot.horizontal_scale,
            ),
            cont_v_scale: scale_factor(
                base_aim.continue_shoot.vertical_scale,
                aim.continue_shoot.vertical_scale,
            ),
            single_h_scale: scale_factor(
                base_aim.single_or_burst.horizontal_scale,
                aim.single_or_burst.horizontal_scale,
            ),
            single_v_scale: scale_factor(
                base_aim.single_or_burst.vertical_scale,
                aim.single_or_burst.vertical_scale,
            ),
        })
    }

    fn tune_requirements(&self, b: &BaseRecoil) -> Option<TuneReq> {
        let base_aim = &self
            .payload
            .numeric_profiles
            .get(self.evaluator.weapon_id)?
            .linked_tables
            .aim_recoil;
        let mut req = TuneReq {
            lower_h: f64::NEG_INFINITY,
            upper_h: f64::INFINITY,
            lower_v: f64::NEG_INFINITY,
            upper_v: f64::INFINITY,
            lower_k: f64::NEG_INFINITY,
            upper_k: f64::INFINITY,
            impossible: false,
        };
        for c in &self.search_constraints {
            let (lower, upper) = match c.id.as_str() {
                "recoil-continuous-horizontal" | "recoil-single-horizontal" => {
                    (&mut req.lower_h, &mut req.upper_h)
                }
                "recoil-continuous-vertical" | "recoil-single-vertical" => {
                    (&mut req.lower_v, &mut req.upper_v)
                }
                "recoil-fire-stability" => (&mut req.lower_k, &mut req.upper_k),
                _ => continue,
            };
            let (k, base, metric): (f64, f64, &dyn Fn(f64) -> f64) = match c.id.as_str() {
                "recoil-continuous-horizontal" => {
                    (b.cr * b.c_h * b.cont_h_scale, 1.0 + b.a_h, &|t| {
                        scale_ratio(
                            base_aim.continue_shoot.horizontal_scale,
                            b.cr * (1.0 + b.a_h - t / 100.0) * b.c_h * b.cont_h_scale,
                        )
                    })
                }
                "recoil-single-horizontal" => {
                    (b.cr * b.c_h * b.single_h_scale, 1.0 + b.a_h, &|t| {
                        scale_ratio(
                            base_aim.single_or_burst.horizontal_scale,
                            b.cr * (1.0 + b.a_h - t / 100.0) * b.c_h * b.single_h_scale,
                        )
                    })
                }
                "recoil-continuous-vertical" => {
                    (b.cr * b.c_v * b.cont_v_scale, 1.0 + b.a_v, &|t| {
                        scale_ratio(
                            base_aim.continue_shoot.vertical_scale,
                            b.cr * (1.0 + b.a_v - t / 100.0) * b.c_v * b.cont_v_scale,
                        )
                    })
                }
                "recoil-single-vertical" => (b.cr * b.c_v * b.single_v_scale, 1.0 + b.a_v, &|t| {
                    scale_ratio(
                        base_aim.single_or_burst.vertical_scale,
                        b.cr * (1.0 + b.a_v - t / 100.0) * b.c_v * b.single_v_scale,
                    )
                }),
                _ => (b.gk_ratio * b.c_k, 1.0, &|t| {
                    scale_ratio(Some(self.gunkick0), b.gk_ratio * (1.0 - t / 100.0) * b.c_k)
                }),
            };
            if let Some(thr) = c.max {
                apply_exact_req(
                    lower,
                    upper,
                    &mut req.impossible,
                    k,
                    base,
                    thr,
                    true,
                    metric,
                );
            }
            if let Some(thr) = c.min {
                apply_exact_req(
                    lower,
                    upper,
                    &mut req.impossible,
                    k,
                    base,
                    thr,
                    false,
                    metric,
                );
            }
        }
        if req.impossible {
            None
        } else {
            Some(req)
        }
    }

    fn pick_tuning(
        &mut self,
        instances: &[TuningInstance<'a>],
        req: &TuneReq,
        parts: &[&Part],
        excluded: &[Vec<(String, f64)>],
    ) -> Option<(TuningState, Vec<(String, f64)>)> {
        if !excluded.is_empty() {
            // The exact pass only produces its single best combination. Once
            // that one fails the caller needs the full frontier to offer
            // alternatives.
            return self.pick_tuning_frontier(instances, req, parts, excluded);
        }
        match self.pick_tuning_exact(instances, req, parts) {
            ExactTune::Found(state, values) => Some((state, values)),
            ExactTune::Infeasible => None,
            ExactTune::Unsupported => self.pick_tuning_frontier(instances, req, parts, excluded),
        }
    }

    fn pick_tuning_exact(
        &mut self,
        instances: &[TuningInstance<'a>],
        req: &TuneReq,
        parts: &[&Part],
    ) -> ExactTune {
        if req.upper_h.is_finite() || req.upper_v.is_finite() || req.upper_k.is_finite() {
            return ExactTune::Unsupported;
        }
        let mut aim_max: Option<f64> = None;
        for c in &self.search_constraints {
            if c.id == "handling-aim-time" {
                if c.min.is_some() {
                    return ExactTune::Unsupported;
                }
                aim_max = c.max;
            } else if !is_panel_attr(&c.id) && !is_recoil_metric(&c.id) {
                return ExactTune::Unsupported;
            }
        }
        if self.budget_exceeded() {
            return ExactTune::Unsupported;
        }
        let mut groups: Vec<(String, Vec<LocalOffer>)> = Vec::new();
        for inst in instances {
            if !tuning_available(&inst.slot_name, inst.part) {
                continue;
            }
            for tune in &inst.part.tuning_options {
                let key_base = Evaluator::tuning_key(&inst.slot_path, &inst.part.id, &tune.tune_id);
                let offers = if let Some(v) = self.options.locked_tuning.get(&key_base).copied() {
                    let v = quantize(v, tune.min_value, tune.max_value, tune.step_value);
                    let c = tune_channels(tune, v);
                    vec![LocalOffer {
                        e_h: c.h,
                        e_v: c.v,
                        e_k: c.k,
                        e_open: c.open,
                        e_move: c.move_speed,
                        e_move_runtime: c.move_runtime,
                        e_breath: c.breath,
                        e_move_stability: c.move_stability,
                        value: v,
                    }]
                } else {
                    self.tune_offers(&inst.part.id, tune)
                };
                if !offers.is_empty() {
                    groups.push((key_base, offers));
                }
            }
        }
        let n = groups.len();
        const SCALE: f64 = 1_000_000.0;
        let mut base = [0.0f64; 4];
        let increments: Vec<Vec<(i64, i64, i64, i64)>> = groups
            .iter()
            .map(|(_, offers)| {
                let mut min = [f64::INFINITY; 4];
                for o in offers {
                    let vals = [o.e_h, o.e_v, o.e_k, o.e_open];
                    for c in 0..4 {
                        min[c] = min[c].min(vals[c]);
                    }
                }
                for c in 0..4 {
                    base[c] += min[c];
                }
                offers
                    .iter()
                    .map(|o| {
                        let vals = [o.e_h, o.e_v, o.e_k, o.e_open];
                        let floor =
                            |c: usize| (((vals[c] - min[c]).max(0.0)) * SCALE).floor() as i64;
                        (floor(0), floor(1), floor(2), floor(3))
                    })
                    .collect()
            })
            .collect();
        let lower_target = |t: f64, n: usize| -> (i64, i64) {
            if !t.is_finite() {
                return (0, 0);
            }
            let scaled = t * SCALE;
            let strict = ((scaled - 1e-3).ceil() as i64).max(0);
            let loose = (((scaled - n as f64).floor()) as i64 + 1).max(0);
            (strict, loose)
        };
        let (h_strict, h_loose) = lower_target(req.lower_h - base[0], n);
        let (v_strict, v_loose) = lower_target(req.lower_v - base[1], n);
        let (k_strict, k_loose) = lower_target(req.lower_k - base[2], n);
        let open_max: i64 = (0..n)
            .map(|i| increments[i].iter().map(|x| x.3).max().unwrap_or(0))
            .sum();
        let (handling, ads_mult) = self.evaluator.aim_inputs(parts);
        let aim_ok = |open: f64| -> Option<bool> {
            match aim_max {
                None => Some(true),
                Some(thr) => self
                    .evaluator
                    .aim_time(handling, ads_mult, open)
                    .map(|v| v <= thr + 1e-9),
            }
        };
        let mut open_strict = 0i64;
        let mut open_loose = 0i64;
        if aim_max.is_some() {
            match aim_ok(base[3]) {
                Some(true) => {}
                Some(false) => {
                    let upper = base[3] + (open_max + n as i64) as f64 / SCALE;
                    if aim_ok(upper) != Some(true) {
                        return ExactTune::Infeasible;
                    }
                    let mut lo = 1i64;
                    let mut hi = open_max;
                    while lo < hi {
                        let mid = lo + (hi - lo) / 2;
                        let open = base[3] + mid as f64 / SCALE;
                        if aim_ok(open) == Some(true) {
                            hi = mid;
                        } else {
                            lo = mid + 1;
                        }
                    }
                    open_strict = lo;
                    open_loose = (lo - n as i64 + 1).max(0);
                }
                None => return ExactTune::Unsupported,
            }
        }
        let strict = [Some(h_strict), Some(v_strict), Some(k_strict)];
        let strict = if aim_max.is_some() {
            [strict[0], strict[1], strict[2], Some(open_strict)]
        } else {
            [strict[0], strict[1], strict[2], None]
        };
        let strict = strict.map(|t| t.filter(|v| *v > 0));
        match reach_targets(&increments, strict, 400_000) {
            ReachResult::Found(chosen) => {
                if let Some((state, values)) =
                    self.reconstruct_tuning(&groups, &chosen, aim_max, parts)
                {
                    ExactTune::Found(state, values)
                } else {
                    ExactTune::Unsupported
                }
            }
            ReachResult::Budget => ExactTune::Unsupported,
            ReachResult::Infeasible => {
                let loose = [
                    Some(h_loose).filter(|v| *v > 0),
                    Some(v_loose).filter(|v| *v > 0),
                    Some(k_loose).filter(|v| *v > 0),
                    if aim_max.is_some() {
                        Some(open_loose).filter(|v| *v > 0)
                    } else {
                        None
                    },
                ];
                if loose == strict {
                    return ExactTune::Infeasible;
                }
                match reach_targets(&increments, loose, 400_000) {
                    ReachResult::Found(_) | ReachResult::Budget => ExactTune::Unsupported,
                    ReachResult::Infeasible => ExactTune::Infeasible,
                }
            }
        }
    }

    fn reconstruct_tuning(
        &self,
        groups: &[(String, Vec<LocalOffer>)],
        chosen: &[usize],
        aim_max: Option<f64>,
        parts: &[&Part],
    ) -> Option<(TuningState, Vec<(String, f64)>)> {
        let mut state = TuningState::default();
        let mut values = Vec::with_capacity(groups.len());
        let mut open = 0.0;
        let mut move_speed = 0.0;
        let mut move_runtime = 0.0;
        let mut breath = 0.0;
        let mut move_stability = 0.0;
        for (idx, (key, offers)) in groups.iter().enumerate() {
            let o = offers.get(*chosen.get(idx)?)?;
            state.insert(key.clone(), o.value);
            values.push((key.clone(), o.value));
            open += o.e_open;
            move_speed += o.e_move;
            move_runtime += o.e_move_runtime;
            breath += o.e_breath;
            move_stability += o.e_move_stability;
        }
        if let Some(thr) = aim_max {
            let sums = DetailSums {
                open_speed: open,
                move_speed,
                move_runtime,
                breath_stability: breath,
                move_stability,
            };
            let detail = self.evaluator.detail_with(parts, &sums);
            match detail.aim_time {
                Some(v) if v <= thr + 1e-9 => {}
                _ => return None,
            }
        }
        Some((state, values))
    }

    fn pick_tuning_frontier(
        &mut self,
        instances: &[TuningInstance<'a>],
        req: &TuneReq,
        parts: &[&Part],
        excluded: &[Vec<(String, f64)>],
    ) -> Option<(TuningState, Vec<(String, f64)>)> {
        let cache_key = tune_cache_key(instances);
        let (frontier, keys, capped) =
            if let Some(cached) = self.tune_frontier_cache.get(&cache_key) {
                (cached.offers.clone(), cached.keys.clone(), cached.capped)
            } else {
                let (built, keys, capped) = self.build_tune_frontier(instances)?;
                let built = Rc::new(built);
                let keys = Rc::new(keys);
                if self.tune_frontier_cache.len() >= TUNE_FRONTIER_CACHE_LIMIT {
                    self.tune_frontier_cache.clear();
                }
                self.tune_frontier_cache.insert(
                    cache_key,
                    CachedFrontier {
                        offers: built.clone(),
                        keys: keys.clone(),
                        capped,
                    },
                );
                (built, keys, capped)
            };
        if capped {
            self.heuristic_incomplete = true;
            self.tune_incomplete = true;
        }
        Self::pick_from_frontier(
            &frontier,
            &keys,
            req,
            parts,
            self.options,
            &self.evaluator,
            excluded,
        )
    }

    fn stage2_time_ok(&self) -> bool {
        match self.deadline_at {
            None => true,
            Some(deadline) => deadline.saturating_sub(now_ms()) >= TUNE_STAGE2_MIN_MS,
        }
    }

    fn build_tune_frontier(
        &mut self,
        instances: &[TuningInstance<'a>],
    ) -> Option<(Vec<Offer>, Vec<String>, bool)> {
        let tune_cap = self.tune_cap;
        let tune_next = self.tune_next;
        let mut capped = false;
        let empty = || Offer {
            e_h: 0.0,
            e_v: 0.0,
            e_k: 0.0,
            e_open: 0.0,
            e_move: 0.0,
            e_move_runtime: 0.0,
            e_breath: 0.0,
            e_move_stability: 0.0,
            values: Vec::new(),
            deviation: 0.0,
        };
        let mut frontier: Vec<Offer> = vec![empty()];
        let mut keys: Vec<String> = Vec::new();
        for inst in instances {
            if self.budget_exceeded() {
                return None;
            }
            let mut group: Vec<Offer> = vec![empty()];
            for tune in &inst.part.tuning_options {
                if !tuning_available(&inst.slot_name, inst.part) {
                    continue;
                }
                let offers = {
                    let key_base =
                        Evaluator::tuning_key(&inst.slot_path, &inst.part.id, &tune.tune_id);
                    if let Some(v) = self.options.locked_tuning.get(&key_base).copied() {
                        let v = quantize(v, tune.min_value, tune.max_value, tune.step_value);
                        let c = tune_channels(tune, v);
                        vec![LocalOffer {
                            e_h: c.h,
                            e_v: c.v,
                            e_k: c.k,
                            e_open: c.open,
                            e_move: c.move_speed,
                            e_move_runtime: c.move_runtime,
                            e_breath: c.breath,
                            e_move_stability: c.move_stability,
                            value: v,
                        }]
                    } else {
                        self.tune_offers(&inst.part.id, tune)
                    }
                };
                let key_base = Evaluator::tuning_key(&inst.slot_path, &inst.part.id, &tune.tune_id);
                keys.push(key_base.clone());
                let default = tune.default_value;
                let mut next = Vec::new();
                for a in &group {
                    for b in &offers {
                        if next.len() >= tune_next {
                            break;
                        }
                        let mut values = a.values.clone();
                        values.push(b.value);
                        next.push(Offer {
                            e_h: a.e_h + b.e_h,
                            e_v: a.e_v + b.e_v,
                            e_k: a.e_k + b.e_k,
                            e_open: a.e_open + b.e_open,
                            e_move: a.e_move + b.e_move,
                            e_move_runtime: a.e_move_runtime + b.e_move_runtime,
                            e_breath: a.e_breath + b.e_breath,
                            e_move_stability: a.e_move_stability + b.e_move_stability,
                            values,
                            deviation: a.deviation + (b.value - default).abs(),
                        });
                    }
                    if next.len() >= tune_next {
                        capped = true;
                        self.heuristic_incomplete = true;
                        self.tune_incomplete = true;
                        break;
                    }
                }
                let (pruned, cut) = if self.negative_recoil_max {
                    (next, Trim::default())
                } else {
                    pareto_capped(next, &self.tune_dims, tune_cap, self.deadline_at)
                };
                self.heuristic_incomplete |= cut.capped;
                capped |= cut.capped;
                self.tune_incomplete |= cut.capped;
                self.truncated |= cut.deadline;
                group = pruned;
            }
            if group.len() == 1 && group[0].values.is_empty() {
                continue;
            }
            let mut next = Vec::new();
            for a in &frontier {
                for b in &group {
                    if next.len() >= tune_next {
                        break;
                    }
                    let mut values = a.values.clone();
                    values.extend_from_slice(&b.values);
                    next.push(Offer {
                        e_h: a.e_h + b.e_h,
                        e_v: a.e_v + b.e_v,
                        e_k: a.e_k + b.e_k,
                        e_open: a.e_open + b.e_open,
                        e_move: a.e_move + b.e_move,
                        e_move_runtime: a.e_move_runtime + b.e_move_runtime,
                        e_breath: a.e_breath + b.e_breath,
                        e_move_stability: a.e_move_stability + b.e_move_stability,
                        values,
                        deviation: a.deviation + b.deviation,
                    });
                }
                if next.len() >= tune_next {
                    capped = true;
                    self.heuristic_incomplete = true;
                    self.tune_incomplete = true;
                    break;
                }
            }
            let (pruned, cut) = if self.negative_recoil_max {
                (next, Trim::default())
            } else {
                pareto_capped(next, &self.tune_dims, tune_cap, self.deadline_at)
            };
            self.heuristic_incomplete |= cut.capped;
            capped |= cut.capped;
            self.tune_incomplete |= cut.capped;
            self.truncated |= cut.deadline;
            frontier = pruned;
        }
        // The reported tuning map zips keys with values, so a length mismatch
        // would silently drop tunings. Keep an O(frontier) check in release.
        assert!(
            frontier.iter().all(|o| o.values.len() == keys.len()),
            "tuning frontier keys and values are misaligned"
        );
        Some((frontier, keys, capped))
    }

    fn pick_from_frontier(
        frontier: &[Offer],
        keys: &[String],
        req: &TuneReq,
        parts: &[&Part],
        options: &SolveOptions,
        evaluator: &Evaluator<'a>,
        excluded: &[Vec<(String, f64)>],
    ) -> Option<(TuningState, Vec<(String, f64)>)> {
        assert!(
            frontier.iter().all(|o| o.values.len() == keys.len()),
            "tuning frontier keys and values are misaligned"
        );
        let detail_constraints: Vec<&Constraint> = options
            .constraints
            .iter()
            .filter(|c| !is_panel_attr(&c.id) && !is_recoil_metric(&c.id))
            .collect();
        // Detail metrics that ignore the tuning sums are fixed for these parts.
        // Reject the whole frontier once instead of once per offer; this is the
        // common case for expensive assemblies.
        let parts_only: Vec<&Constraint> = detail_constraints
            .iter()
            .copied()
            .filter(|c| !detail_constraint_needs_tuning(&c.id))
            .collect();
        if !parts_only.is_empty() {
            let neutral = evaluator.detail_with(parts, &DetailSums::default());
            let ok = parts_only.iter().all(|c| {
                let Some(value) = neutral.value(&c.id) else {
                    return false;
                };
                c.min.map(|m| value >= m - 1e-9).unwrap_or(true)
                    && c.max.map(|m| value <= m + 1e-9).unwrap_or(true)
            });
            if !ok {
                return None;
            }
        }
        if !detail_constraints.is_empty()
            && !frontier_reachable(frontier, parts, &detail_constraints, evaluator)
        {
            return None;
        }
        let mut best: Option<&Offer> = None;
        let mut best_score = f64::NEG_INFINITY;
        for o in frontier {
            if excluded
                .iter()
                .any(|assignment| offer_matches_assignment(o, keys, assignment))
            {
                continue;
            }
            if o.e_h < req.lower_h - 1e-9
                || o.e_h > req.upper_h + 1e-9
                || o.e_v < req.lower_v - 1e-9
                || o.e_v > req.upper_v + 1e-9
                || o.e_k < req.lower_k - 1e-9
                || o.e_k > req.upper_k + 1e-9
            {
                continue;
            }
            if !detail_constraints.is_empty() {
                let sums = DetailSums {
                    open_speed: o.e_open,
                    move_speed: o.e_move,
                    move_runtime: o.e_move_runtime,
                    breath_stability: o.e_breath,
                    move_stability: o.e_move_stability,
                };
                let detail = evaluator.detail_with(parts, &sums);
                let ok = detail_constraints.iter().all(|c| {
                    let Some(value) = detail.value(&c.id) else {
                        return false;
                    };
                    c.min.map(|m| value >= m - 1e-9).unwrap_or(true)
                        && c.max.map(|m| value <= m + 1e-9).unwrap_or(true)
                });
                if !ok {
                    continue;
                }
            }
            let mut slack = f64::INFINITY;
            for (value, lower) in [
                (o.e_h, req.lower_h),
                (o.e_v, req.lower_v),
                (o.e_k, req.lower_k),
            ] {
                if lower.is_finite() {
                    slack = slack.min(value - lower);
                }
            }
            if !slack.is_finite() {
                slack = 0.0;
            }
            let score = -o.deviation + slack * 1e-9;
            if score > best_score {
                best_score = score;
                best = Some(o);
            }
        }
        let best = best?;
        let mut state = TuningState::default();
        for (key, value) in keys.iter().zip(best.values.iter()) {
            state.insert(key.clone(), *value);
        }
        Some((
            state,
            keys.iter()
                .cloned()
                .zip(best.values.iter().copied())
                .collect(),
        ))
    }

    fn tune_offers(&mut self, part_id: &str, tune: &crate::model::TuningOption) -> Vec<LocalOffer> {
        let key = (part_id.to_string(), tune.tune_id.clone());
        if let Some(cached) = self.offer_cache.get(&key) {
            return cached
                .iter()
                .map(|o| LocalOffer {
                    e_h: o.e_h,
                    e_v: o.e_v,
                    e_k: o.e_k,
                    e_open: o.e_open,
                    e_move: o.e_move,
                    e_move_runtime: o.e_move_runtime,
                    e_breath: o.e_breath,
                    e_move_stability: o.e_move_stability,
                    value: o.values.first().copied().unwrap_or(tune.default_value),
                })
                .collect();
        }
        let (values, truncated) = tune_offer_values(tune);
        if truncated {
            self.heuristic_incomplete = true;
            self.tune_incomplete = true;
        }
        let deviation = |v: f64| (v - tune.default_value).abs();
        let mut offers: Vec<LocalOffer> = values
            .into_iter()
            .map(|v| {
                let c = tune_channels(tune, v);
                LocalOffer {
                    e_h: c.h,
                    e_v: c.v,
                    e_k: c.k,
                    e_open: c.open,
                    e_move: c.move_speed,
                    e_move_runtime: c.move_runtime,
                    e_breath: c.breath,
                    e_move_stability: c.move_stability,
                    value: v,
                }
            })
            .collect();
        offers.sort_by(|a, b| {
            b.e_h
                .partial_cmp(&a.e_h)
                .unwrap_or(Ordering::Equal)
                .then(b.e_v.partial_cmp(&a.e_v).unwrap_or(Ordering::Equal))
                .then(b.e_k.partial_cmp(&a.e_k).unwrap_or(Ordering::Equal))
                .then(
                    deviation(a.value)
                        .partial_cmp(&deviation(b.value))
                        .unwrap_or(Ordering::Equal),
                )
        });
        let dims = self.tune_dims;
        let mut pruned: Vec<LocalOffer> = Vec::new();
        if self.negative_recoil_max {
            pruned = offers;
        } else {
            for o in offers {
                if pruned.iter().any(|p| local_offer_dominates(p, &o, &dims)) {
                    continue;
                }
                pruned.retain(|p| !local_offer_dominates(&o, p, &dims));
                pruned.push(o);
            }
        }
        pruned.sort_by(|a, b| {
            b.e_h
                .partial_cmp(&a.e_h)
                .unwrap_or(Ordering::Equal)
                .then(b.e_v.partial_cmp(&a.e_v).unwrap_or(Ordering::Equal))
                .then(b.e_k.partial_cmp(&a.e_k).unwrap_or(Ordering::Equal))
                .then(
                    deviation(a.value)
                        .partial_cmp(&deviation(b.value))
                        .unwrap_or(Ordering::Equal),
                )
        });
        let cached: Vec<Offer> = pruned
            .iter()
            .map(|o| Offer {
                e_h: o.e_h,
                e_v: o.e_v,
                e_k: o.e_k,
                e_open: o.e_open,
                e_move: o.e_move,
                e_move_runtime: o.e_move_runtime,
                e_breath: o.e_breath,
                e_move_stability: o.e_move_stability,
                values: vec![o.value],
                deviation: deviation(o.value),
            })
            .collect();
        self.offer_cache.insert(key, cached);
        pruned
    }

    fn installed_list(&self, assembly: &crate::assembly::Assembly) -> Vec<InstalledPart> {
        let mut out = Vec::new();
        for slot in &assembly.visible_slots {
            if let Some(id) = &slot.selected_part_id {
                if let Some(part) = self.payload.parts.get(id) {
                    out.push(InstalledPart {
                        slot_path: slot.path_key.clone(),
                        slot_name: slot.slot.name.clone(),
                        part_id: part.id.clone(),
                        part_name: part.name.clone(),
                        price: self
                            .evaluator
                            .prices
                            .get(part.price_id())
                            .copied()
                            .unwrap_or(0),
                    });
                }
            }
        }
        for item in &assembly.internal_parts {
            if let Some(part) = self.payload.parts.get(&item.part_id) {
                out.push(InstalledPart {
                    slot_path: item.slot_path_key.clone(),
                    slot_name: item.slot.name.clone(),
                    part_id: part.id.clone(),
                    part_name: part.name.clone(),
                    price: self
                        .evaluator
                        .prices
                        .get(part.price_id())
                        .copied()
                        .unwrap_or(0),
                });
            }
        }
        out
    }
}

#[derive(Debug, Clone, Copy)]
struct LocalOffer {
    e_h: f64,
    e_v: f64,
    e_k: f64,
    e_open: f64,
    e_move: f64,
    e_move_runtime: f64,
    e_breath: f64,
    e_move_stability: f64,
    value: f64,
}

type CapPoint = [f64; 4];

/// Candidate points kept on the dominator side. Dropping reachable points
/// only makes a covering query fail, so a small cap is sound.
const CAP_COVER_LIMIT: usize = 8;
/// Corners used to upper/lower-bound the dominated side. Every corner is a
/// sound bound for its chunk, so a small cap stays sound while keeping the
/// dominance test cheap.
const CAP_REQ_LIMIT: usize = 8;

fn cap_ge(a: &CapPoint, b: &CapPoint) -> bool {
    (0..4).all(|i| a[i] >= b[i] - 1e-9)
}

fn cap_le(a: &CapPoint, b: &CapPoint) -> bool {
    (0..4).all(|i| a[i] <= b[i] + 1e-9)
}

/// Achievable tuning-vector set reduced to the points dominance ever needs:
/// coordinatewise-maximal points for higher-is-better channels and
/// coordinatewise-minimal points for lower-is-better ones. Truncation only
/// drops reachable points, so every later dominance proof stays sound, but it
/// must be applied to the two extremes independently.
#[derive(Debug, Clone, Default)]
struct CapFront {
    /// Reachable Pareto-maximal vectors, trimmed. Used as the dominator side.
    max: Vec<CapPoint>,
    /// Upper-bound corners: their upward closure covers every reachable
    /// vector. Used as the dominated side, where an unreachable corner is
    /// still a valid requirement.
    max_req: Vec<CapPoint>,
    /// Reachable Pareto-minimal vectors, trimmed.
    min: Vec<CapPoint>,
    /// Lower-bound corners, covering every reachable vector from below.
    min_req: Vec<CapPoint>,
}

fn cap_sort_dedup(mut pts: Vec<CapPoint>) -> Vec<CapPoint> {
    pts.sort_by(|a, b| {
        a.iter().zip(b.iter()).fold(Ordering::Equal, |ord, (x, y)| {
            ord.then(x.partial_cmp(y).unwrap_or(Ordering::Equal))
        })
    });
    pts.dedup_by(|a, b| (0..4).all(|i| (a[i] - b[i]).abs() <= 1e-9));
    pts
}

fn cap_trim(mut pts: Vec<CapPoint>, want_max: bool) -> Vec<CapPoint> {
    if pts.len() > CAP_COVER_LIMIT {
        // `pts` is ascending, so the useful extreme is the tail for maxima and
        // the head for minima. Dropping reachable points only weakens the
        // dominator side, never the requirement side.
        if want_max {
            pts.drain(..pts.len() - CAP_COVER_LIMIT);
        } else {
            pts.truncate(CAP_COVER_LIMIT);
        }
    }
    pts
}

/// Sorted Pareto extremes of a reachable set.
fn cap_extremes(pts: Vec<CapPoint>, want_max: bool) -> Vec<CapPoint> {
    if pts.len() <= 1 {
        return pts;
    }
    let pts = cap_sort_dedup(pts);
    let mut out: Vec<CapPoint> = Vec::new();
    for i in 0..pts.len() {
        let mut keep = true;
        for j in 0..pts.len() {
            if i == j {
                continue;
            }
            let ge = cap_ge(&pts[j], &pts[i]);
            let le = cap_le(&pts[j], &pts[i]);
            if (want_max && ge && !le) || (!want_max && le && !ge) {
                keep = false;
                break;
            }
        }
        if keep {
            out.push(pts[i]);
        }
    }
    out
}

/// Fold a sorted reachable set into at most `CAP_REQ_LIMIT` corners whose
/// orthants cover it: each chunk is replaced by its coordinatewise maximum
/// (upper bounds) or minimum (lower bounds). Every dropped point stays inside
/// the orthant of its chunk corner, so the corners are a sound over-
/// approximation.
fn cap_corners(pts: &[CapPoint], want_max: bool) -> Vec<CapPoint> {
    if pts.len() <= CAP_REQ_LIMIT {
        return pts.to_vec();
    }
    let mut out = Vec::with_capacity(CAP_REQ_LIMIT);
    let mut start = 0usize;
    for chunk in 0..CAP_REQ_LIMIT {
        let remaining_points = pts.len() - start;
        let remaining_chunks = CAP_REQ_LIMIT - chunk;
        let take = remaining_points.div_ceil(remaining_chunks);
        let end = start + take;
        let mut corner = pts[start];
        for p in &pts[start + 1..end] {
            for i in 0..4 {
                if want_max {
                    corner[i] = corner[i].max(p[i]);
                } else {
                    corner[i] = corner[i].min(p[i]);
                }
            }
        }
        out.push(corner);
        start = end;
    }
    out
}

fn cap_front(points: Vec<CapPoint>) -> CapFront {
    if points.is_empty() {
        return CapFront::default();
    }
    let pts = cap_sort_dedup(points);
    let max = cap_extremes(pts.clone(), true);
    let min = cap_extremes(pts, false);
    CapFront {
        max_req: cap_corners(&max, true),
        min_req: cap_corners(&min, false),
        max: cap_trim(max, true),
        min: cap_trim(min, false),
    }
}

/// Sums of the operands' candidate vectors. The extreme points of a Minkowski
/// sum are sums of the operands' extreme points, and a correlated sum never
/// reaches a corner that neither operand reaches alone.
fn cap_sum(x: &[CapPoint], y: &[CapPoint]) -> Vec<CapPoint> {
    if x.len() == 1 && y.len() == 1 {
        return vec![[
            x[0][0] + y[0][0],
            x[0][1] + y[0][1],
            x[0][2] + y[0][2],
            x[0][3] + y[0][3],
        ]];
    }
    let mut out = Vec::with_capacity(x.len().saturating_mul(y.len()));
    for p in x {
        for q in y {
            out.push([p[0] + q[0], p[1] + q[1], p[2] + q[2], p[3] + q[3]]);
        }
    }
    out
}

fn cap_combine(a: &CapFront, b: &CapFront) -> CapFront {
    let max = cap_sort_dedup(cap_sum(&a.max, &b.max));
    let min = cap_sort_dedup(cap_sum(&a.min, &b.min));
    let max_req = cap_corners(&cap_sort_dedup(cap_sum(&a.max_req, &b.max_req)), true);
    let min_req = cap_corners(&cap_sort_dedup(cap_sum(&a.min_req, &b.min_req)), false);
    let max = cap_trim(max, true);
    let min = cap_trim(min, false);
    CapFront {
        max,
        max_req,
        min,
        min_req,
    }
}

#[derive(Debug, Clone, Default)]
struct ExactPart {
    d: f64,
    ah: f64,
    av: f64,
    ch: f64,
    cv: f64,
    ck: f64,
    eh: f64,
    ev: f64,
    ek: f64,
    hs: f64,
    adsm: f64,
    eopen: f64,
    eh_min: f64,
    ev_min: f64,
    ek_min: f64,
    eopen_min: f64,
    e_front: Option<Rc<CapFront>>,
    panel: Vec<f64>,
}

impl ExactPart {
    fn neutral() -> Self {
        Self {
            d: 0.0,
            ah: 0.0,
            av: 0.0,
            ch: 1.0,
            cv: 1.0,
            ck: 1.0,
            eh: 0.0,
            ev: 0.0,
            ek: 0.0,
            hs: 0.0,
            adsm: 0.0,
            eopen: 0.0,
            eh_min: 0.0,
            ev_min: 0.0,
            ek_min: 0.0,
            eopen_min: 0.0,
            e_front: None,
            panel: Vec::new(),
        }
    }

    fn neutral_panel(n: usize) -> Self {
        let mut m = Self::neutral();
        m.panel = vec![0.0; n];
        m
    }

    fn add(&mut self, other: &ExactPart) {
        self.d += other.d;
        self.ah += other.ah;
        self.av += other.av;
        self.ch *= other.ch;
        self.cv *= other.cv;
        self.ck *= other.ck;
        self.eh += other.eh;
        self.ev += other.ev;
        self.ek += other.ek;
        self.hs += other.hs;
        self.adsm += other.adsm;
        self.eopen += other.eopen;
        self.eh_min += other.eh_min;
        self.ev_min += other.ev_min;
        self.ek_min += other.ek_min;
        self.eopen_min += other.eopen_min;
        self.e_front = match (&self.e_front, &other.e_front) {
            (Some(a), Some(b)) => Some(Rc::new(cap_combine(a, b))),
            (Some(front), None) => Some(front.clone()),
            (None, Some(front)) => Some(front.clone()),
            (None, None) => None,
        };
        if self.panel.len() < other.panel.len() {
            self.panel.resize(other.panel.len(), 0.0);
        }
        for (i, v) in other.panel.iter().enumerate() {
            self.panel[i] += *v;
        }
    }

    /// `exact_laser_pro` only fills the additive fields below; the
    /// multiplicative and tuning fields stay neutral, so applying and
    /// removing the delta is exact. The detail and panel channels keep
    /// separate winners because the site ranks them over different part
    /// lists.
    fn add_laser_pro_detail(&mut self, pro: &ExactPart) {
        self.d += pro.d;
        self.hs += pro.hs;
    }

    fn remove_laser_pro_detail(&mut self, pro: &ExactPart) {
        self.d -= pro.d;
        self.hs -= pro.hs;
    }

    fn add_laser_pro_panel(&mut self, pro: &ExactPart) {
        if self.panel.len() < pro.panel.len() {
            self.panel.resize(pro.panel.len(), 0.0);
        }
        for (i, v) in pro.panel.iter().enumerate() {
            self.panel[i] += *v;
        }
    }

    fn remove_laser_pro_panel(&mut self, pro: &ExactPart) {
        for (i, v) in pro.panel.iter().enumerate() {
            self.panel[i] -= *v;
        }
    }

    fn score(&self) -> f64 {
        self.d * 10.0 - (self.ah + self.av) * 20.0
            + ((1.0 - self.ch) + (1.0 - self.cv) + (1.0 - self.ck)) * 20.0
            + (self.eh + self.ev + self.ek) * 2.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Action {
    Unset,
    Remove,
    Defer,
    Part(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum SlotChoice {
    Unset,
    Remove,
    Part(String),
}

/// Tuning-free contribution of one laser part's `pro` modifiers. The
/// evaluator applies the modifiers of a single laser part, the one with the
/// highest `score`, so the DP keeps the candidate deltas out of the slot
/// metrics and applies only the winner. Options and states share the deltas
/// through `Rc` so a state only carries two pointers.
#[derive(Debug, Clone)]
struct LaserPro {
    score: f64,
    metric: ExactPart,
}

#[derive(Debug, Clone)]
struct DpOption {
    cost: i64,
    metric: ExactPart,
    laser_pro: Option<Rc<LaserPro>>,
    visible: bool,
    choice: SlotChoice,
}

#[derive(Debug, Clone)]
struct DpState {
    cost: i64,
    metric: ExactPart,
    detail_pro: Option<Rc<LaserPro>>,
    panel_pro: Option<Rc<LaserPro>>,
    choices: Vec<SlotChoice>,
}

/// Optimistic completion bounds for the slots after a DP level. Each field is
/// an extremum that can be added/multiplied to a partial state without looking
/// at the actual choices, so a state that fails a bound here cannot be
/// completed into a feasible build.
#[derive(Debug, Clone, Copy)]
struct DpSuffixBound {
    d_min: f64,
    d_max: f64,
    ah_min: f64,
    av_min: f64,
    ch_min: f64,
    cv_min: f64,
    ck_min: f64,
    ch_negative: bool,
    cv_negative: bool,
    ck_negative: bool,
    eh_max: f64,
    ev_max: f64,
    ek_max: f64,
    eopen_max: f64,
    hs_max: f64,
    adsm_min: f64,
}

fn dp_suffix_bounds(options_per_slot: &[Vec<DpOption>]) -> Vec<DpSuffixBound> {
    let mut out = vec![
        DpSuffixBound {
            d_min: 0.0,
            d_max: 0.0,
            ah_min: 0.0,
            av_min: 0.0,
            ch_min: 1.0,
            cv_min: 1.0,
            ck_min: 1.0,
            ch_negative: false,
            cv_negative: false,
            ck_negative: false,
            eh_max: 0.0,
            ev_max: 0.0,
            ek_max: 0.0,
            eopen_max: 0.0,
            hs_max: 0.0,
            adsm_min: 0.0,
        };
        options_per_slot.len() + 1
    ];
    for idx in (0..options_per_slot.len()).rev() {
        let opts = &options_per_slot[idx];
        let mut d_min = f64::INFINITY;
        let mut d_max = f64::NEG_INFINITY;
        let mut ah_min = f64::INFINITY;
        let mut av_min = f64::INFINITY;
        let mut ch_min = f64::INFINITY;
        let mut cv_min = f64::INFINITY;
        let mut ck_min = f64::INFINITY;
        let mut ch_negative = false;
        let mut cv_negative = false;
        let mut ck_negative = false;
        let mut eh_max = f64::NEG_INFINITY;
        let mut ev_max = f64::NEG_INFINITY;
        let mut ek_max = f64::NEG_INFINITY;
        let mut eopen_max = f64::NEG_INFINITY;
        let mut hs_max = f64::NEG_INFINITY;
        let mut adsm_min = f64::INFINITY;
        for o in opts {
            let m = &o.metric;
            d_min = d_min.min(m.d);
            d_max = d_max.max(m.d);
            ah_min = ah_min.min(m.ah);
            av_min = av_min.min(m.av);
            ch_min = ch_min.min(m.ch);
            cv_min = cv_min.min(m.cv);
            ck_min = ck_min.min(m.ck);
            ch_negative |= m.ch < 0.0;
            cv_negative |= m.cv < 0.0;
            ck_negative |= m.ck < 0.0;
            eh_max = eh_max.max(m.eh);
            ev_max = ev_max.max(m.ev);
            ek_max = ek_max.max(m.ek);
            eopen_max = eopen_max.max(m.eopen);
            hs_max = hs_max.max(m.hs);
            adsm_min = adsm_min.min(m.adsm);
        }
        if opts.is_empty() {
            d_min = 0.0;
            d_max = 0.0;
            ah_min = 0.0;
            av_min = 0.0;
            ch_min = 1.0;
            cv_min = 1.0;
            ck_min = 1.0;
            eh_max = 0.0;
            ev_max = 0.0;
            ek_max = 0.0;
            eopen_max = 0.0;
            hs_max = 0.0;
            adsm_min = 0.0;
        }
        let next = out[idx + 1];
        out[idx] = DpSuffixBound {
            d_min: d_min + next.d_min,
            d_max: d_max + next.d_max,
            ah_min: ah_min + next.ah_min,
            av_min: av_min + next.av_min,
            ch_min: ch_min * next.ch_min,
            cv_min: cv_min * next.cv_min,
            ck_min: ck_min * next.ck_min,
            ch_negative: ch_negative || next.ch_negative,
            cv_negative: cv_negative || next.cv_negative,
            ck_negative: ck_negative || next.ck_negative,
            eh_max: eh_max + next.eh_max,
            ev_max: ev_max + next.ev_max,
            ek_max: ek_max + next.ek_max,
            eopen_max: eopen_max + next.eopen_max,
            hs_max: hs_max + next.hs_max,
            adsm_min: adsm_min + next.adsm_min,
        };
    }
    out
}

/// Applies `opt` to the metric and laser winner of `state`. Split out from
/// `dp_advance` so callers can evaluate bounds on the advanced metric before
/// paying for a choices clone.
fn dp_metric_advance(
    state: &DpState,
    opt: &DpOption,
) -> (ExactPart, Option<Rc<LaserPro>>, Option<Rc<LaserPro>>) {
    let mut metric = state.metric.clone();
    metric.add(&opt.metric);
    let mut detail_pro = state.detail_pro.clone();
    let mut panel_pro = state.panel_pro.clone();
    if let Some(candidate) = &opt.laser_pro {
        if detail_pro
            .as_ref()
            .map(|cur| candidate.score > cur.score)
            .unwrap_or(true)
        {
            if let Some(cur) = &detail_pro {
                metric.remove_laser_pro_detail(&cur.metric);
            }
            metric.add_laser_pro_detail(&candidate.metric);
            detail_pro = Some(candidate.clone());
        }
        // Internal parts never reach the panel, so an internal laser cannot
        // occupy the panel's pro slot.
        if opt.visible
            && panel_pro
                .as_ref()
                .map(|cur| candidate.score > cur.score)
                .unwrap_or(true)
        {
            if let Some(cur) = &panel_pro {
                metric.remove_laser_pro_panel(&cur.metric);
            }
            metric.add_laser_pro_panel(&candidate.metric);
            panel_pro = Some(candidate.clone());
        }
    }
    (metric, detail_pro, panel_pro)
}

/// Extends `state` with `opt`. A laser candidate replaces the current laser
/// winner only when it scores strictly higher, matching `laser_pro_index`,
/// which keeps the first part on a tie.
#[cfg(test)]
fn dp_advance(state: &DpState, opt: &DpOption, cost: i64) -> DpState {
    let (metric, detail_pro, panel_pro) = dp_metric_advance(state, opt);
    let mut choices = state.choices.clone();
    choices.push(opt.choice.clone());
    DpState {
        cost,
        metric,
        detail_pro,
        panel_pro,
        choices,
    }
}

fn exact_part_metric(
    part: &Part,
    weapon_id: &str,
    slot_name: &str,
    mode: &str,
    bipod_deployed: bool,
    is_laser: bool,
    include_laser_pro: bool,
    include_panel: bool,
    panel_attrs: &[String],
) -> ExactPart {
    let mut m = ExactPart::neutral_panel(panel_attrs.len());
    if include_panel && !panel_attrs.is_empty() {
        for modif in part.display_mods_for(weapon_id, mode) {
            if modif.condition_id.is_some() || modif.modifier_type != "Addend" {
                continue;
            }
            if is_laser && modif.polarity.as_deref() != Some("con") && !include_laser_pro {
                continue;
            }
            let attr = normalize_attr(modif.attribute_id.as_deref());
            if let Some(pos) = panel_attrs.iter().position(|a| *a == attr) {
                m.panel[pos] += modif.delta.map(|v| v.0).unwrap_or(0.0);
            }
        }
    }
    for modif in part.main_mods_for(weapon_id, mode) {
        if modif.condition_id.is_some() || modif.modifier_type != "Addend" {
            continue;
        }
        if is_laser && modif.polarity.as_deref() != Some("con") && !include_laser_pro {
            continue;
        }
        if normalize_attr(modif.attribute_id.as_deref()) == "10005" {
            m.d += modif.delta.map(|v| v.0).unwrap_or(0.0);
        }
        if HANDLING_ATTRS.contains(&normalize_attr(modif.attribute_id.as_deref())) {
            m.hs += modif.delta.map(|v| v.0).unwrap_or(0.0);
        }
    }
    if let Some(rr) = part.raw_recoil.get(mode) {
        if let Some(v) = rr.horizontal {
            if v.is_finite() {
                m.ah += v;
            }
        }
        if let Some(v) = rr.vertical {
            if v.is_finite() {
                m.av += v;
            }
        }
    }
    let bipod = is_bipod(part);
    let mut explicit_spring = false;
    if let Some(list) = part.raw_mods.get(mode) {
        for modif in list {
            if modif.target_path == "GAiming_ADSTime"
                && modif.modifier_type == "Mult_A"
                && modif.param1.map(|v| v.0.is_finite()).unwrap_or(false)
            {
                m.adsm += modif.param1.map(|v| v.0).unwrap_or(0.0);
            }
            if modif.modifier_type != "Mult_C" {
                continue;
            }
            let Some(p) = modif.param1.map(|v| v.0) else {
                continue;
            };
            if !p.is_finite() {
                continue;
            }
            if bipod && !bipod_deployed && is_recoil_target(&modif.target_path) {
                continue;
            }
            match modif.target_path.as_str() {
                "GRecoil_H" => m.ch *= p,
                "GRecoil_V" => m.cv *= p,
                "GGunkickSpring" => {
                    m.ck *= p;
                    explicit_spring = true;
                }
                _ => {}
            }
        }
    }
    if (!bipod || bipod_deployed) && !explicit_spring {
        if let Some(k) = part
            .raw_recoil
            .get(mode)
            .and_then(|r| r.gunkick_spring_mult_c)
        {
            if k.is_finite() {
                m.ck *= 1.0 + k;
            }
        }
    }
    if !crate::metrics::tuning_available(slot_name, part) {
        return m;
    }
    let mut front = CapFront::default();
    let mut has_front = false;
    let mut tuned = false;
    for tune in &part.tuning_options {
        let mut max_h = f64::NEG_INFINITY;
        let mut max_v = f64::NEG_INFINITY;
        let mut max_k = f64::NEG_INFINITY;
        let mut max_open = f64::NEG_INFINITY;
        let mut min_h = f64::INFINITY;
        let mut min_v = f64::INFINITY;
        let mut min_k = f64::INFINITY;
        let mut min_open = f64::INFINITY;
        let mut points: Vec<CapPoint> = Vec::new();
        for v in tune_offer_values(tune).0 {
            let channels = tune_channels(tune, v);
            max_h = max_h.max(channels.h);
            max_v = max_v.max(channels.v);
            max_k = max_k.max(channels.k);
            max_open = max_open.max(channels.open);
            min_h = min_h.min(channels.h);
            min_v = min_v.min(channels.v);
            min_k = min_k.min(channels.k);
            min_open = min_open.min(channels.open);
            let point = [channels.h, channels.v, channels.k, channels.open];
            if point.iter().all(|v| v.is_finite()) {
                points.push(point);
            }
        }
        if !points.is_empty() {
            let option_front = cap_front(points);
            front = if has_front {
                cap_combine(&front, &option_front)
            } else {
                option_front
            };
            has_front = true;
            tuned = true;
        }
        if max_h.is_finite() {
            m.eh += max_h;
        }
        if max_v.is_finite() {
            m.ev += max_v;
        }
        if max_k.is_finite() {
            m.ek += max_k;
        }
        if max_open.is_finite() {
            m.eopen += max_open;
        }
        if min_h.is_finite() {
            m.eh_min += min_h;
        }
        if min_v.is_finite() {
            m.ev_min += min_v;
        }
        if min_k.is_finite() {
            m.ek_min += min_k;
        }
        if min_open.is_finite() {
            m.eopen_min += min_open;
        }
    }
    if tuned {
        m.e_front = Some(Rc::new(front));
    }
    m
}

/// Additive `pro` modifiers of a laser part, with every other field neutral.
/// `exact_part_metric` skips these modifiers for laser parts unless the caller
/// knows the part wins the single-laser selection; this helper extracts them so
/// the DP can apply exactly one winner's delta.
fn exact_laser_pro(part: &Part, weapon_id: &str, mode: &str, panel_attrs: &[String]) -> ExactPart {
    let mut m = ExactPart::neutral_panel(panel_attrs.len());
    // `modifier_included` treats every non-`con` addend of the selected laser
    // part as a laser bonus, so the additive delta must cover them all, not
    // just the `pro` ones used to rank laser parts.
    if !panel_attrs.is_empty() {
        for modif in part.display_mods_for(weapon_id, mode) {
            if modif.condition_id.is_some()
                || modif.modifier_type != "Addend"
                || modif.polarity.as_deref() == Some("con")
            {
                continue;
            }
            let attr = normalize_attr(modif.attribute_id.as_deref());
            if let Some(pos) = panel_attrs.iter().position(|a| *a == attr) {
                m.panel[pos] += modif.delta.map(|v| v.0).unwrap_or(0.0);
            }
        }
    }
    for modif in part.main_mods_for(weapon_id, mode) {
        if modif.condition_id.is_some()
            || modif.modifier_type != "Addend"
            || modif.polarity.as_deref() == Some("con")
        {
            continue;
        }
        if normalize_attr(modif.attribute_id.as_deref()) == "10005" {
            m.d += modif.delta.map(|v| v.0).unwrap_or(0.0);
        }
        if HANDLING_ATTRS.contains(&normalize_attr(modif.attribute_id.as_deref())) {
            m.hs += modif.delta.map(|v| v.0).unwrap_or(0.0);
        }
    }
    m
}

fn metric_hash(m: &ExactPart) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut mix = |v: f64| {
        h ^= v.to_bits();
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    };
    mix(m.d);
    mix(m.ah);
    mix(m.av);
    mix(m.ch);
    mix(m.cv);
    mix(m.ck);
    mix(m.hs);
    mix(m.adsm);
    for p in &m.panel {
        mix(*p);
    }
    mix(m.eh);
    mix(m.ev);
    mix(m.ek);
    mix(m.eopen);
    mix(m.eh_min);
    mix(m.ev_min);
    mix(m.ek_min);
    mix(m.eopen_min);
    if let Some(front) = &m.e_front {
        for points in [&front.max, &front.max_req, &front.min, &front.min_req] {
            mix(points.len() as f64);
            for p in points {
                for v in p {
                    mix(*v);
                }
            }
        }
    }
    h
}

fn cap_front_eq(a: &Option<Rc<CapFront>>, b: &Option<Rc<CapFront>>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(x), Some(y)) => {
            x.max == y.max && x.max_req == y.max_req && x.min == y.min && x.min_req == y.min_req
        }
        _ => false,
    }
}

fn metric_eq(a: &ExactPart, b: &ExactPart) -> bool {
    a.d == b.d
        && a.ah == b.ah
        && a.av == b.av
        && a.ch == b.ch
        && a.cv == b.cv
        && a.ck == b.ck
        && a.hs == b.hs
        && a.adsm == b.adsm
        && a.eh == b.eh
        && a.ev == b.ev
        && a.ek == b.ek
        && a.eopen == b.eopen
        && a.eh_min == b.eh_min
        && a.ev_min == b.ev_min
        && a.ek_min == b.ek_min
        && a.eopen_min == b.eopen_min
        && a.panel == b.panel
        && cap_front_eq(&a.e_front, &b.e_front)
}

fn e_max_channels(m: &ExactPart) -> CapPoint {
    [m.eh, m.ev, m.ek, m.eopen]
}

fn e_min_channels(m: &ExactPart) -> CapPoint {
    [m.eh_min, m.ev_min, m.ek_min, m.eopen_min]
}

const ZERO_CAP: CapPoint = [0.0; 4];

fn cap_points(m: &ExactPart, want_max: bool) -> &[CapPoint] {
    match &m.e_front {
        Some(front) => {
            if want_max {
                &front.max
            } else {
                &front.min
            }
        }
        None => std::slice::from_ref(&ZERO_CAP),
    }
}

fn cap_requirements(m: &ExactPart, want_max: bool) -> &[CapPoint] {
    match &m.e_front {
        Some(front) => {
            if want_max {
                &front.max_req
            } else {
                &front.min_req
            }
        }
        None => std::slice::from_ref(&ZERO_CAP),
    }
}

/// Does every reachable tuning vector of `y` have a reachable `x` vector that
/// is at least as good on every channel in `mask`? `y` is represented by
/// upper-bound corners whose orthants cover its reachable set, so covering
/// every corner covers every reachable vector. `x` uses its reachable maximal
/// points; dropping some of those can only make the query fail, never succeed
/// falsely.
fn e_higher_cover(x: &ExactPart, y: &ExactPart, mask: u8) -> bool {
    if mask == 0 {
        return true;
    }
    let x_points = cap_points(x, true);
    let y_points = cap_requirements(y, true);
    let covers =
        |p: &CapPoint, q: &CapPoint| (0..4).all(|i| mask & (1 << i) == 0 || p[i] >= q[i] - 1e-9);
    y_points
        .iter()
        .all(|q| x_points.iter().any(|p| covers(p, q)))
}

/// Counterpart of `e_higher_cover` for lower-is-better channels.
fn e_lower_cover(x: &ExactPart, y: &ExactPart, mask: u8) -> bool {
    if mask == 0 {
        return true;
    }
    let x_points = cap_points(x, false);
    let y_points = cap_requirements(y, false);
    let covers =
        |p: &CapPoint, q: &CapPoint| (0..4).all(|i| mask & (1 << i) == 0 || p[i] <= q[i] + 1e-9);
    y_points
        .iter()
        .all(|q| x_points.iter().any(|p| covers(p, q)))
}

/// Mixed directions need one tuning choice that is both high on `higher_mask`
/// and low on `lower_mask`. The envelope cannot prove that, so use bounds that
/// hold for every choice.
fn e_mixed_ok(x: &ExactPart, y: &ExactPart, higher_mask: u8, lower_mask: u8) -> bool {
    let x_max = e_max_channels(x);
    let x_min = e_min_channels(x);
    let y_max = e_max_channels(y);
    let y_min = e_min_channels(y);
    (0..4).all(|i| {
        if higher_mask & (1 << i) != 0 && x_min[i] < y_max[i] - 1e-9 {
            return false;
        }
        if lower_mask & (1 << i) != 0 && x_max[i] > y_min[i] + 1e-9 {
            return false;
        }
        true
    })
}

fn dp_dominates(
    a: &DpState,
    b: &DpState,
    dims: &DpDims,
    panel_dirs: &[PanelDir],
    aim_dir: AimDir,
) -> bool {
    let x = &a.metric;
    let y = &b.metric;
    if a.cost > b.cost {
        return false;
    }
    if dims.d && x.d < y.d - 1e-9 {
        return false;
    }
    if dims.ah && x.ah > y.ah + 1e-9 {
        return false;
    }
    if dims.av && x.av > y.av + 1e-9 {
        return false;
    }
    if dims.ch && x.ch > y.ch + 1e-9 {
        return false;
    }
    if dims.cv && x.cv > y.cv + 1e-9 {
        return false;
    }
    if dims.ck && x.ck > y.ck + 1e-9 {
        return false;
    }
    let aim_lower = dims.hs && aim_dir == AimDir::LessHandling;
    let aim_higher = dims.hs && aim_dir == AimDir::MoreHandling;
    let mut higher_mask = 0u8;
    if dims.eh {
        higher_mask |= 1;
    }
    if dims.ev {
        higher_mask |= 2;
    }
    if dims.ek {
        higher_mask |= 4;
    }
    if aim_higher {
        higher_mask |= 8;
    }
    let lower_mask = if aim_lower { 8u8 } else { 0 };
    // Every reachable vector of `y` is bounded by `y`'s per-channel maxima,
    // and `y` attains each maximum at some reachable vector, so matching those
    // maxima is a necessary condition for any covering. Checking it first
    // rejects most pairs before the envelope scan.
    let x_max = e_max_channels(x);
    let y_max = e_max_channels(y);
    let x_min = e_min_channels(x);
    let y_min = e_min_channels(y);
    for i in 0..4 {
        let bit = 1 << i;
        if higher_mask & bit != 0 && x_max[i] < y_max[i] - 1e-9 {
            return false;
        }
        if lower_mask & bit != 0 && x_min[i] > y_min[i] + 1e-9 {
            return false;
        }
    }
    if higher_mask != 0 && lower_mask != 0 {
        if !e_mixed_ok(x, y, higher_mask, lower_mask) {
            return false;
        }
    } else {
        if !e_higher_cover(x, y, higher_mask) {
            return false;
        }
        if !e_lower_cover(x, y, lower_mask) {
            return false;
        }
    }
    if dims.hs {
        match aim_dir {
            AimDir::MoreHandling => {
                if x.hs < y.hs - 1e-9 || x.adsm > y.adsm + 1e-9 {
                    return false;
                }
            }
            AimDir::LessHandling => {
                if x.hs > y.hs + 1e-9 || x.adsm < y.adsm - 1e-9 {
                    return false;
                }
            }
            AimDir::Equal => {
                if (x.hs - y.hs).abs() > 1e-9
                    || (x.adsm - y.adsm).abs() > 1e-9
                    || (x.eopen - x.eopen_min).abs() > 1e-9
                    || (y.eopen - y.eopen_min).abs() > 1e-9
                    || (x.eopen - y.eopen).abs() > 1e-9
                {
                    return false;
                }
            }
            AimDir::Ignore => {}
        }
    }
    if x.panel.len() != y.panel.len() {
        return false;
    }
    for (idx, (a, b)) in x.panel.iter().zip(y.panel.iter()).enumerate() {
        match panel_dirs.get(idx).copied().unwrap_or(PanelDir::Ignore) {
            PanelDir::Higher => {
                if *a < *b - 1e-9 {
                    return false;
                }
            }
            PanelDir::Lower => {
                if *a > *b + 1e-9 {
                    return false;
                }
            }
            PanelDir::Equal => {
                if (*a - *b).abs() > 1e-9 {
                    return false;
                }
            }
            PanelDir::Ignore => {}
        }
    }
    true
}

#[cfg(test)]
fn dp_pareto(
    states: Vec<DpState>,
    dims: &DpDims,
    panel_dirs: &[PanelDir],
    aim_dir: AimDir,
) -> Vec<DpState> {
    dp_pareto_capped(
        states,
        dims,
        panel_dirs,
        aim_dir,
        usize::MAX,
        None,
        false,
        None,
        None,
    )
    .0
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Trim {
    capped: bool,
    deadline: bool,
    aborted: bool,
}

fn dp_pareto_capped(
    mut states: Vec<DpState>,
    dims: &DpDims,
    panel_dirs: &[PanelDir],
    aim_dir: AimDir,
    limit: usize,
    deadline_at: Option<u64>,
    seed: bool,
    abort_at: Option<usize>,
    abort_not_before: Option<u64>,
) -> (Vec<DpState>, Trim) {
    if seed {
        // The seed pass only has to return one feasible build, and the cap
        // keeps the head of the list, so rank by capability instead of price.
        states.sort_by(|a, b| {
            b.metric
                .score()
                .partial_cmp(&a.metric.score())
                .unwrap_or(Ordering::Equal)
                .then(a.cost.cmp(&b.cost))
        });
    } else {
        states.sort_by(|a, b| {
            a.cost.cmp(&b.cost).then(
                b.metric
                    .score()
                    .partial_cmp(&a.metric.score())
                    .unwrap_or(Ordering::Equal),
            )
        });
    }
    // The cartesian product repeats the same metric vector under many different
    // choice lists (slots that do not affect any constrained metric). Sorting by
    // cost first means the first occurrence is the cheapest one, so the rest can
    // be dropped without touching the optimality argument. States with tuning
    // headroom are interchangeable only when their whole reachable tuning set
    // matches, so the envelope is part of the hash and the equality check.
    let mut buckets: FxHashMap<u64, Vec<usize>> = FxHashMap::default();
    let mut deduped: Vec<DpState> = Vec::with_capacity(states.len());
    for st in states {
        let hash = metric_hash(&st.metric);
        let bucket = buckets.entry(hash).or_default();
        if bucket
            .iter()
            .any(|&idx| metric_eq(&deduped[idx].metric, &st.metric))
        {
            continue;
        }
        bucket.push(deduped.len());
        deduped.push(st);
    }
    let states = deduped;
    let mut trim = Trim::default();
    let mut out: Vec<DpState> = Vec::new();
    for (idx, st) in states.into_iter().enumerate() {
        if out.len() >= limit {
            trim.capped = true;
            break;
        }
        if idx & 1023 == 0 && deadline_at.is_some_and(|d| now_ms() >= d) {
            trim.deadline = true;
            break;
        }
        if abort_at.is_some_and(|a| out.len() >= a)
            && abort_not_before.is_none_or(|t| now_ms() >= t)
        {
            trim.aborted = true;
            break;
        }
        if out
            .iter()
            .any(|o| dp_dominates(o, &st, dims, panel_dirs, aim_dir))
        {
            continue;
        }
        out.retain(|o| !dp_dominates(&st, o, dims, panel_dirs, aim_dir));
        out.push(st);
    }
    (out, trim)
}

#[derive(Debug, Clone, Copy)]
struct MetricBound {
    max_d: f64,
    min_a_h: f64,
    min_a_v: f64,
    min_c_h: f64,
    min_c_v: f64,
    min_c_k: f64,
    min_t_h: f64,
    min_t_v: f64,
    min_t_k: f64,
    panel_max: [f64; 5],
    panel_min: [f64; 5],
}

impl MetricBound {
    fn neutral() -> Self {
        Self {
            max_d: 0.0,
            min_a_h: 0.0,
            min_a_v: 0.0,
            min_c_h: 1.0,
            min_c_v: 1.0,
            min_c_k: 1.0,
            min_t_h: 0.0,
            min_t_v: 0.0,
            min_t_k: 0.0,
            panel_max: [0.0; 5],
            panel_min: [0.0; 5],
        }
    }

    fn add_a(&mut self, other: &MetricBound) {
        self.max_d += other.max_d;
        self.min_a_h += other.min_a_h;
        self.min_a_v += other.min_a_v;
        self.min_c_h *= other.min_c_h;
        self.min_c_v *= other.min_c_v;
        self.min_c_k *= other.min_c_k;
        self.min_t_h += other.min_t_h;
        self.min_t_v += other.min_t_v;
        self.min_t_k += other.min_t_k;
        for idx in 0..5 {
            self.panel_max[idx] += other.panel_max[idx];
            self.panel_min[idx] += other.panel_min[idx];
        }
    }

    /// Adds every channel except the panel. Internal parts still feed the
    /// detail/recoil metrics but never the panel.
    fn add_non_panel(&mut self, other: &MetricBound) {
        self.max_d += other.max_d;
        self.min_a_h += other.min_a_h;
        self.min_a_v += other.min_a_v;
        self.min_c_h *= other.min_c_h;
        self.min_c_v *= other.min_c_v;
        self.min_c_k *= other.min_c_k;
        self.min_t_h += other.min_t_h;
        self.min_t_v += other.min_t_v;
        self.min_t_k += other.min_t_k;
    }

    fn add_optional_non_panel(&mut self, other: &MetricBound) {
        self.max_d += other.max_d.max(0.0);
        self.min_a_h += other.min_a_h.min(0.0);
        self.min_a_v += other.min_a_v.min(0.0);
        self.min_c_h *= other.min_c_h.min(1.0);
        self.min_c_v *= other.min_c_v.min(1.0);
        self.min_c_k *= other.min_c_k.min(1.0);
        self.min_t_h += other.min_t_h.min(0.0);
        self.min_t_v += other.min_t_v.min(0.0);
        self.min_t_k += other.min_t_k.min(0.0);
    }

    fn best_of(&mut self, other: &MetricBound) {
        self.max_d = self.max_d.max(other.max_d);
        self.min_a_h = self.min_a_h.min(other.min_a_h);
        self.min_a_v = self.min_a_v.min(other.min_a_v);
        self.min_c_h = self.min_c_h.min(other.min_c_h);
        self.min_c_v = self.min_c_v.min(other.min_c_v);
        self.min_c_k = self.min_c_k.min(other.min_c_k);
        self.min_t_h = self.min_t_h.min(other.min_t_h);
        self.min_t_v = self.min_t_v.min(other.min_t_v);
        self.min_t_k = self.min_t_k.min(other.min_t_k);
        for idx in 0..5 {
            self.panel_max[idx] = self.panel_max[idx].max(other.panel_max[idx]);
            self.panel_min[idx] = self.panel_min[idx].min(other.panel_min[idx]);
        }
    }
}

/// One assembly node whose metric bound `cannot_satisfy` accumulates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BoundRoot {
    /// A decided visible slot: add the selected part's own bound.
    DecidedVisible(usize),
    /// A decided hidden slot carrying an internal part: add the part's own bound.
    DecidedInternal(usize),
    /// An undecided visible slot: add the recursive slot bound for its subtree.
    AssumedVisible(usize),
    /// An implicitly selected part below an active parent: add its own bound
    /// optimistically (it may or may not be installed in a completion).
    OptionalInternal(usize),
}

/// Selects the nodes whose metric bounds are disjoint, so every part below an
/// active slot is counted at most once.
///
/// `slot_bounds` for an undecided visible slot is recursive: it is the best
/// bound over every candidate part and all of that part's child slots. Its
/// descendants therefore must not contribute again. Decided visible slots and
/// internal parts only contribute their own [`part_own_bound`], so they do not
/// cover their descendants: those are visited and counted individually.
///
/// This keeps the result optimistic: an uncovered part is either counted
/// exactly once through its own bound, or absorbed by a recursive slot bound
/// that dominates every candidate in that slot. A part under a counted
/// subtree is skipped, and subtrees are rooted only at top-most covered nodes.
fn bound_roots(
    visible_slots: &[crate::assembly::VisibleSlot],
    internal_parts: &[crate::assembly::InternalPart],
    decided: &FxHashSet<String>,
    has_visible_bound: impl Fn(&crate::assembly::VisibleSlot) -> bool,
) -> Vec<BoundRoot> {
    let mut parent: FxHashMap<&str, Option<&str>> = FxHashMap::default();
    for slot in visible_slots {
        parent.insert(slot.path_key.as_str(), slot.parent_slot_path_key.as_deref());
    }
    for item in internal_parts {
        parent.insert(
            item.slot_path_key.as_str(),
            item.parent_slot_path_key.as_deref(),
        );
    }
    fn covered_ancestor<'a>(
        mut path: Option<&'a str>,
        parent: &FxHashMap<&'a str, Option<&'a str>>,
        covered: &FxHashSet<&'a str>,
    ) -> bool {
        while let Some(key) = path {
            if covered.contains(key) {
                return true;
            }
            path = parent.get(key).copied().flatten();
        }
        false
    }
    // Coverage invariant: `covered` holds exactly the paths whose recursive
    // `slot_bound` was added. Every later node under a covered path is skipped,
    // so each subtree is counted once; nodes outside one count only their own
    // bound and never extend coverage.
    let mut nodes: Vec<(usize, bool, usize)> =
        Vec::with_capacity(visible_slots.len() + internal_parts.len());
    for (idx, slot) in visible_slots.iter().enumerate() {
        nodes.push((slot.depth, true, idx));
    }
    for (idx, item) in internal_parts.iter().enumerate() {
        nodes.push((item.depth, false, idx));
    }
    // Ancestors always have a smaller depth, so this visits every node before
    // any descendant. Visible slots sort first within a depth because a visible
    // slot and its internal-part record describe the same node.
    nodes.sort_by_key(|&(depth, visible, idx)| (depth, !visible, idx));

    let mut roots = Vec::new();
    let mut covered: FxHashSet<&str> = FxHashSet::default();
    let mut optional_paths: FxHashSet<&str> = FxHashSet::default();
    let mut seen: FxHashSet<(&str, &str)> = FxHashSet::default();
    for (_, visible, idx) in nodes {
        if visible {
            let slot = &visible_slots[idx];
            if covered_ancestor(slot.parent_slot_path_key.as_deref(), &parent, &covered) {
                continue;
            }
            if decided.contains(&slot.path_key) {
                let Some(id) = slot
                    .selected_part_id
                    .as_ref()
                    .or(slot.internal_part_id.as_ref())
                else {
                    continue;
                };
                if seen.insert((slot.path_key.as_str(), id.as_str())) {
                    roots.push(BoundRoot::DecidedVisible(idx));
                }
            } else if has_visible_bound(slot) {
                covered.insert(slot.path_key.as_str());
                roots.push(BoundRoot::AssumedVisible(idx));
            }
        } else {
            let item = &internal_parts[idx];
            if covered.contains(item.slot_path_key.as_str())
                || covered_ancestor(item.parent_slot_path_key.as_deref(), &parent, &covered)
            {
                continue;
            }
            if !seen.insert((item.slot_path_key.as_str(), item.part_id.as_str())) {
                continue;
            }
            if decided.contains(&item.slot_path_key) {
                roots.push(BoundRoot::DecidedInternal(idx));
                continue;
            }
            let parent_active = match item.parent_slot_path_key.as_deref() {
                Some(parent) => decided.contains(parent) || optional_paths.contains(parent),
                None => true,
            };
            if parent_active {
                optional_paths.insert(item.slot_path_key.as_str());
                roots.push(BoundRoot::OptionalInternal(idx));
            }
        }
    }
    roots
}

fn laser_delta_range(
    modifier: &crate::model::Modifier,
    is_laser: bool,
    laser_enabled: bool,
    delta: f64,
) -> (f64, f64) {
    if !is_laser || modifier.polarity.as_deref() == Some("con") {
        return (delta, delta);
    }
    if laser_enabled {
        (delta.min(0.0), delta.max(0.0))
    } else {
        (0.0, 0.0)
    }
}

fn part_own_bound(part: &Part, weapon_id: &str, mode: &str, laser_enabled: bool) -> MetricBound {
    let mut b = MetricBound::neutral();
    let is_laser = crate::metrics::has_laser(part);
    for m in part.display_mods_for(weapon_id, mode) {
        if m.condition_id.is_some() || m.modifier_type != "Addend" {
            continue;
        }
        let Some(idx) = panel_attr_index(normalize_attr(m.attribute_id.as_deref())) else {
            continue;
        };
        let Some(delta) = m.delta.map(|v| v.0).filter(|v| v.is_finite()) else {
            continue;
        };
        let (lower, upper) = laser_delta_range(m, is_laser, laser_enabled, delta);
        b.panel_min[idx] += lower;
        b.panel_max[idx] += upper;
    }
    for m in part.main_mods_for(weapon_id, mode) {
        if m.condition_id.is_none()
            && m.modifier_type == "Addend"
            && normalize_attr(m.attribute_id.as_deref()) == "10005"
        {
            let delta = m.delta.map(|v| v.0).unwrap_or(0.0);
            let (_, upper) = laser_delta_range(m, is_laser, laser_enabled, delta);
            b.max_d += upper;
        }
    }
    if let Some(rr) = part.raw_recoil.get(mode) {
        if let Some(v) = rr.horizontal {
            if v.is_finite() {
                b.min_a_h += v;
            }
        }
        if let Some(v) = rr.vertical {
            if v.is_finite() {
                b.min_a_v += v;
            }
        }
    }
    let mut explicit_spring = false;
    if let Some(list) = part.raw_mods.get(mode) {
        for m in list {
            if m.modifier_type != "Mult_C" {
                continue;
            }
            let Some(p) = m.param1.map(|v| v.0) else {
                continue;
            };
            if !p.is_finite() {
                continue;
            }
            match m.target_path.as_str() {
                "GRecoil_H" => b.min_c_h *= p,
                "GRecoil_V" => b.min_c_v *= p,
                "GGunkickSpring" => {
                    b.min_c_k *= p;
                    explicit_spring = true;
                }
                _ => {}
            }
        }
    }
    if !explicit_spring {
        if let Some(k) = part
            .raw_recoil
            .get(mode)
            .and_then(|r| r.gunkick_spring_mult_c)
        {
            if k.is_finite() {
                b.min_c_k *= 1.0 + k;
            }
        }
    }
    for tune in &part.tuning_options {
        let mut max_h = f64::NEG_INFINITY;
        let mut max_v = f64::NEG_INFINITY;
        let mut max_k = f64::NEG_INFINITY;
        for v in tune_offer_values(tune).0 {
            let (h, vv, k) = tune_offer(tune, v);
            max_h = max_h.max(h);
            max_v = max_v.max(vv);
            max_k = max_k.max(k);
        }
        if max_h.is_finite() {
            b.min_t_h += -max_h / 100.0;
        }
        if max_v.is_finite() {
            b.min_t_v += -max_v / 100.0;
        }
        if max_k.is_finite() {
            b.min_t_k += -max_k / 100.0;
        }
    }
    b
}

fn compute_bounds(
    payload: &WeaponPayload,
    weapon_id: &str,
    mode: &str,
    laser_enabled: bool,
) -> FxHashMap<(String, String), MetricBound> {
    let mut part_cache: FxHashMap<(String, bool), MetricBound> = FxHashMap::default();
    let mut slot_cache: FxHashMap<(String, String), MetricBound> = FxHashMap::default();
    let mut visited: FxHashSet<String> = FxHashSet::default();
    let rules = payload
        .virtual_rules
        .get(weapon_id)
        .cloned()
        .unwrap_or_default();
    let slot_rule_parts: FxHashMap<String, Vec<String>> = {
        let mut m: FxHashMap<String, Vec<String>> = FxHashMap::default();
        for r in &rules {
            m.entry(r.socket_id.clone())
                .or_default()
                .push(r.virtual_part_id.clone());
        }
        m
    };
    for (host, slots) in &payload.slots_by_host {
        for slot in slots {
            slot_bound(
                payload,
                weapon_id,
                mode,
                laser_enabled,
                host,
                slot,
                &slot_rule_parts,
                &mut part_cache,
                &mut slot_cache,
                &mut visited,
            );
        }
    }
    slot_cache
}

#[allow(clippy::too_many_arguments)]
fn part_bound(
    payload: &WeaponPayload,
    weapon_id: &str,
    mode: &str,
    laser_enabled: bool,
    id: &str,
    include_panel: bool,
    slot_rule_parts: &FxHashMap<String, Vec<String>>,
    part_cache: &mut FxHashMap<(String, bool), MetricBound>,
    slot_cache: &mut FxHashMap<(String, String), MetricBound>,
    visited: &mut FxHashSet<String>,
) -> MetricBound {
    if let Some(v) = part_cache.get(&(id.to_string(), include_panel)) {
        return *v;
    }
    let Some(part) = payload.parts.get(id) else {
        return MetricBound::neutral();
    };
    let mut b = part_own_bound(part, weapon_id, mode, laser_enabled);
    if !include_panel {
        b.panel_max = [0.0; 5];
        b.panel_min = [0.0; 5];
    }
    if visited.contains(id) {
        return b;
    }
    visited.insert(id.to_string());
    if let Some(slots) = payload.slots_by_host.get(id) {
        for slot in slots {
            let child = slot_bound(
                payload,
                weapon_id,
                mode,
                laser_enabled,
                id,
                slot,
                slot_rule_parts,
                part_cache,
                slot_cache,
                visited,
            );
            b.add_a(&child);
        }
    }
    visited.remove(id);
    part_cache.insert((id.to_string(), include_panel), b);
    b
}

#[allow(clippy::too_many_arguments)]
fn slot_bound(
    payload: &WeaponPayload,
    weapon_id: &str,
    mode: &str,
    laser_enabled: bool,
    host: &str,
    slot: &crate::model::SlotDef,
    slot_rule_parts: &FxHashMap<String, Vec<String>>,
    part_cache: &mut FxHashMap<(String, bool), MetricBound>,
    slot_cache: &mut FxHashMap<(String, String), MetricBound>,
    visited: &mut FxHashSet<String>,
) -> MetricBound {
    let key = (host.to_string(), slot.id.clone());
    if let Some(v) = slot_cache.get(&key) {
        return *v;
    }
    let mut best = MetricBound::neutral();
    for id in &slot.candidate_ids {
        let b = part_bound(
            payload,
            weapon_id,
            mode,
            laser_enabled,
            id,
            true,
            slot_rule_parts,
            part_cache,
            slot_cache,
            visited,
        );
        best.best_of(&b);
    }
    // Virtual candidates and rule parts are installed internally, so the
    // site panel never counts them.
    for id in &slot.virtual_candidate_ids {
        let b = part_bound(
            payload,
            weapon_id,
            mode,
            laser_enabled,
            id,
            false,
            slot_rule_parts,
            part_cache,
            slot_cache,
            visited,
        );
        best.best_of(&b);
    }
    if let Some(ids) = slot_rule_parts.get(&slot.id) {
        for id in ids {
            let b = part_bound(
                payload,
                weapon_id,
                mode,
                laser_enabled,
                id,
                false,
                slot_rule_parts,
                part_cache,
                slot_cache,
                visited,
            );
            best.best_of(&b);
        }
    }
    slot_cache.insert(key, best);
    best
}

#[derive(Debug, Clone, Copy)]
struct BaseRecoil {
    a_h: f64,
    a_v: f64,
    c_h: f64,
    c_v: f64,
    c_k: f64,
    cr: f64,
    gk_ratio: f64,
    /// Aim-profile scale applied to continuous horizontal/vertical recoil.
    cont_h_scale: f64,
    cont_v_scale: f64,
    /// Aim-profile scale applied to single/burst horizontal/vertical recoil.
    single_h_scale: f64,
    single_v_scale: f64,
}

fn cubic_coeffs(a: &CurveKey, b: &CurveKey) -> (f64, f64, f64) {
    let dt = b.time - a.time;
    let c3 = 2.0 * a.value + dt * a.leave_tangent - 2.0 * b.value + dt * b.arrive_tangent;
    let c2 = -3.0 * a.value - 2.0 * dt * a.leave_tangent + 3.0 * b.value - dt * b.arrive_tangent;
    let c1 = dt * a.leave_tangent;
    (c3, c2, c1)
}

fn quadratic_roots(a: f64, b: f64, c: f64) -> Vec<f64> {
    if a.abs() < 1e-12 {
        if b.abs() < 1e-12 {
            return Vec::new();
        }
        return vec![-c / b];
    }
    let disc = b * b - 4.0 * a * c;
    if disc < 0.0 {
        return Vec::new();
    }
    let root = disc.sqrt();
    vec![(-b - root) / (2.0 * a), (-b + root) / (2.0 * a)]
}

fn curve_is_nonincreasing(curve: &Curve) -> bool {
    for pair in curve.keys.windows(2) {
        let a = &pair[0];
        let b = &pair[1];
        if b.value > a.value + 1e-9 {
            return false;
        }
        if a.interp_mode != "RCIM_Cubic" || b.time <= a.time {
            continue;
        }
        let (c3, c2, c1) = cubic_coeffs(a, b);
        let deriv = |t: f64| 3.0 * c3 * t * t + 2.0 * c2 * t + c1;
        let mut max_deriv = deriv(0.0).max(deriv(1.0));
        let vertex = if c3.abs() > 1e-12 {
            Some(-c2 / (3.0 * c3))
        } else if c2.abs() > 1e-12 {
            Some(-c1 / (2.0 * c2))
        } else {
            None
        };
        if let Some(t) = vertex {
            if t > 0.0 && t < 1.0 {
                max_deriv = max_deriv.max(deriv(t));
            }
        }
        if max_deriv > 1e-9 {
            return false;
        }
    }
    true
}

fn curve_min_value(curve: &Curve) -> Option<f64> {
    let mut min_value = f64::INFINITY;
    for key in &curve.keys {
        if key.value.is_finite() {
            min_value = min_value.min(key.value);
        }
    }
    for pair in curve.keys.windows(2) {
        let a = &pair[0];
        let b = &pair[1];
        if a.interp_mode != "RCIM_Cubic" || b.time <= a.time {
            continue;
        }
        let (c3, c2, c1) = cubic_coeffs(a, b);
        let eval = |t: f64| ((c3 * t + c2) * t + c1) * t + a.value;
        for t in quadratic_roots(3.0 * c3, 2.0 * c2, c1) {
            if t > 0.0 && t < 1.0 {
                let value = eval(t);
                if value.is_finite() {
                    min_value = min_value.min(value);
                }
            }
        }
    }
    if min_value.is_finite() {
        Some(min_value)
    } else {
        None
    }
}

const TUNE_CAP: usize = 4_096;
const TUNE_NEXT_LIMIT: usize = 16_384;
const TUNE_STAGE2_CAP: usize = 65_536;
const TUNE_STAGE2_NEXT: usize = 65_536;
const TUNE_STAGE2_MIN_MS: u64 = 1_000;

/// A tuning candidate can still miss the exact metric after the frontier
/// picked it, because the requirement bounds are the open edges of a rounded
/// band and a discrete grid value can sit exactly on such an edge. Retry with
/// the next candidate instead of dropping the whole assembly.
const TUNE_PICK_ATTEMPTS: usize = 8;
/// Upper bound on cached tuning frontiers. Entries are a few hundred KB at most;
/// the cap keeps pathological payloads from growing the solver without bound.
const TUNE_FRONTIER_CACHE_LIMIT: usize = 512;

fn tune_cache_key(instances: &[TuningInstance<'_>]) -> String {
    let mut key = String::new();
    for inst in instances {
        use std::fmt::Write;
        let _ = write!(
            key,
            "{}:{}:{}:{}:",
            inst.slot_path.len(),
            inst.slot_path,
            inst.part.id.len(),
            inst.part.id
        );
        for tune in &inst.part.tuning_options {
            let _ = write!(key, "{}:{};", tune.tune_id.len(), tune.tune_id);
        }
        key.push('\u{1}');
    }
    key
}

fn pareto_capped(
    mut offers: Vec<Offer>,
    dims: &TuneDims,
    limit: usize,
    deadline_at: Option<u64>,
) -> (Vec<Offer>, Trim) {
    let channel = |dir: TuneDir, a: f64, b: f64| {
        match dir {
            TuneDir::Higher => b.partial_cmp(&a),
            TuneDir::Lower | TuneDir::Equal => a.partial_cmp(&b),
            TuneDir::Ignore => Some(Ordering::Equal),
        }
        .unwrap_or(Ordering::Equal)
    };
    offers.sort_by(|a, b| {
        channel(dims.open, a.e_open, b.e_open)
            .then(channel(dims.move_effect, a.e_move, b.e_move))
            .then(channel(dims.breath, a.e_breath, b.e_breath))
            .then(channel(
                dims.move_stability,
                a.e_move_stability,
                b.e_move_stability,
            ))
            .then(b.e_h.partial_cmp(&a.e_h).unwrap_or(Ordering::Equal))
            .then(b.e_v.partial_cmp(&a.e_v).unwrap_or(Ordering::Equal))
            .then(b.e_k.partial_cmp(&a.e_k).unwrap_or(Ordering::Equal))
            .then(
                a.deviation
                    .partial_cmp(&b.deviation)
                    .unwrap_or(Ordering::Equal),
            )
    });
    let mut trim = Trim::default();
    let equal_dims = equal_channels(dims);
    // An `Equal` channel only admits dominators with the same value, so offers
    // can be partitioned by those values. Bucketing turns the quadratic scan
    // into an average-constant one when a band constraint (min and max on the
    // same metric) pins that channel. Without `Equal` channels every offer can
    // dominate every other, so keep the full scan.
    if equal_dims.is_empty() || equal_dims.len() > 3 {
        let mut out: Vec<Offer> = Vec::new();
        for (idx, o) in offers.into_iter().enumerate() {
            if out.len() >= limit {
                trim.capped = true;
                break;
            }
            if idx & 1023 == 0 && deadline_at.is_some_and(|d| now_ms() >= d) {
                trim.deadline = true;
                break;
            }
            if out.iter().any(|p| offer_dominates(p, &o, dims)) {
                continue;
            }
            out.retain(|p| !offer_dominates(&o, p, dims));
            out.push(o);
        }
        return (out, trim);
    }
    // Quantization is much coarser than `channel_ok`'s 1e-9 tolerance, so equal
    // channel values land in the same or an adjacent bucket; probing the 3^k
    // neighbors never misses a dominator.
    const BUCKET_SCALE: f64 = 1e6;
    let bucket_key = |o: &Offer| -> Vec<i64> {
        equal_dims
            .iter()
            .map(|channel| (channel(o) * BUCKET_SCALE).round() as i64)
            .collect()
    };
    let neighbor_keys = |key: &[i64]| -> Vec<Vec<i64>> {
        let mut keys = vec![Vec::with_capacity(key.len())];
        for &value in key {
            let mut next = Vec::with_capacity(keys.len() * 3);
            for base in &keys {
                for delta in -1..=1 {
                    let mut candidate = base.clone();
                    candidate.push(value + delta);
                    next.push(candidate);
                }
            }
            keys = next;
        }
        keys
    };
    let mut buckets: FxHashMap<Vec<i64>, Vec<usize>> = FxHashMap::default();
    let mut out: Vec<Offer> = Vec::new();
    let mut alive: Vec<bool> = Vec::new();
    let mut kept = 0usize;
    for (idx, o) in offers.into_iter().enumerate() {
        if kept >= limit {
            trim.capped = true;
            break;
        }
        if idx & 1023 == 0 && deadline_at.is_some_and(|d| now_ms() >= d) {
            trim.deadline = true;
            break;
        }
        let key = bucket_key(&o);
        let neighbors = neighbor_keys(&key);
        let mut dominated = false;
        for neighbor in &neighbors {
            let Some(list) = buckets.get(neighbor) else {
                continue;
            };
            if list
                .iter()
                .any(|&i| alive[i] && offer_dominates(&out[i], &o, dims))
            {
                dominated = true;
                break;
            }
        }
        if dominated {
            continue;
        }
        let kept_idx = out.len();
        out.push(o);
        alive.push(true);
        kept += 1;
        for neighbor in &neighbors {
            let Some(list) = buckets.get_mut(neighbor) else {
                continue;
            };
            for &i in list.iter() {
                if alive[i] && offer_dominates(&out[kept_idx], &out[i], dims) {
                    alive[i] = false;
                    kept -= 1;
                }
            }
        }
        buckets.entry(key).or_default().push(kept_idx);
    }
    let out = out
        .into_iter()
        .zip(alive)
        .filter_map(|(offer, alive)| alive.then_some(offer))
        .collect();
    (out, trim)
}

/// Channels whose dominance relation is exact equality, in a fixed order.
fn equal_channels(dims: &TuneDims) -> Vec<fn(&Offer) -> f64> {
    type Accessor = fn(&Offer) -> f64;
    let mut channels: Vec<Accessor> = Vec::new();
    if dims.open == TuneDir::Equal {
        channels.push(|o| o.e_open);
    }
    if dims.move_effect == TuneDir::Equal {
        channels.push(|o| o.e_move);
        channels.push(|o| o.e_move_runtime);
    }
    if dims.breath == TuneDir::Equal {
        channels.push(|o| o.e_breath);
    }
    if dims.move_stability == TuneDir::Equal {
        channels.push(|o| o.e_move_stability);
    }
    if dims.recoil_h == TuneDir::Equal {
        channels.push(|o| o.e_h);
    }
    if dims.recoil_v == TuneDir::Equal {
        channels.push(|o| o.e_v);
    }
    if dims.recoil_k == TuneDir::Equal {
        channels.push(|o| o.e_k);
    }
    channels
}

fn offer_dominates(a: &Offer, b: &Offer, dims: &TuneDims) -> bool {
    channel_ok(a.e_h, b.e_h, dims.recoil_h)
        && channel_ok(a.e_v, b.e_v, dims.recoil_v)
        && channel_ok(a.e_k, b.e_k, dims.recoil_k)
        && channel_ok(a.e_open, b.e_open, dims.open)
        && channel_ok(a.e_move, b.e_move, dims.move_effect)
        && channel_ok(a.e_move_runtime, b.e_move_runtime, dims.move_effect)
        && channel_ok(a.e_breath, b.e_breath, dims.breath)
        && channel_ok(a.e_move_stability, b.e_move_stability, dims.move_stability)
}

fn local_offer_dominates(a: &LocalOffer, b: &LocalOffer, dims: &TuneDims) -> bool {
    channel_ok(a.e_h, b.e_h, dims.recoil_h)
        && channel_ok(a.e_v, b.e_v, dims.recoil_v)
        && channel_ok(a.e_k, b.e_k, dims.recoil_k)
        && channel_ok(a.e_open, b.e_open, dims.open)
        && channel_ok(a.e_move, b.e_move, dims.move_effect)
        && channel_ok(a.e_move_runtime, b.e_move_runtime, dims.move_effect)
        && channel_ok(a.e_breath, b.e_breath, dims.breath)
        && channel_ok(a.e_move_stability, b.e_move_stability, dims.move_stability)
}

/// Inverts a monotone metric exactly: finds the tuning total (in percent)
/// where `metric` crosses `threshold`. `scale_ratio` rounds twice, so the
/// closed-form linear inversion can sit on the wrong side of the rounding
/// step; bisecting the real metric yields a bound every admissible tuning
/// total on the feasible side satisfies.
fn apply_exact_req(
    lower: &mut f64,
    upper: &mut f64,
    impossible: &mut bool,
    k: f64,
    base: f64,
    threshold: f64,
    is_max: bool,
    metric: &dyn Fn(f64) -> f64,
) {
    if !k.is_finite() || !threshold.is_finite() || !base.is_finite() {
        *impossible = true;
        return;
    }
    if k == 0.0 {
        let value = metric(0.0);
        if (is_max && value > threshold) || (!is_max && value < threshold) {
            *impossible = true;
        }
        return;
    }
    let linear = 100.0 * (base - threshold / k);
    let feasible_right = (k > 0.0) == is_max;
    let ok = |t: f64| {
        let value = metric(t);
        if is_max {
            value <= threshold
        } else {
            value >= threshold
        }
    };
    let mut lo = linear;
    let mut hi = linear;
    let mut step = 1.0;
    let mut separated = false;
    for _ in 0..60 {
        lo -= step;
        hi += step;
        step *= 2.0;
        if ok(lo) != ok(hi) {
            separated = true;
            break;
        }
    }
    if !separated {
        if ok(linear) {
            return;
        }
        *impossible = true;
        return;
    }
    let (mut bad, mut good) = if ok(lo) { (hi, lo) } else { (lo, hi) };
    for _ in 0..100 {
        let mid = (bad + good) * 0.5;
        if ok(mid) {
            good = mid;
        } else {
            bad = mid;
        }
    }
    if feasible_right {
        *lower = lower.max(good);
    } else {
        *upper = upper.min(good);
    }
}

/// Lower bound on the aim-scale multiplier applied to raw recoil, per axis.
/// A part with `aimRecoilProfilesByMode` can replace the base aim profile, and
/// `Evaluator::evaluate` multiplies the raw recoil by `override / base`. Values
/// below 1 shrink the constrained metric, so the prefilter must scale its
/// optimistic value by the smallest factor any installable part could apply,
/// and subtract the rounding error of `scale_ratio`.
#[derive(Debug, Clone, Copy)]
struct AimScaleFloor {
    h_factor: f64,
    v_factor: f64,
    h_margin: f64,
    v_margin: f64,
    enabled: bool,
}

impl AimScaleFloor {
    fn identity() -> Self {
        Self {
            h_factor: 1.0,
            v_factor: 1.0,
            h_margin: 0.0,
            v_margin: 0.0,
            enabled: true,
        }
    }
}

fn aim_round2(value: f64) -> f64 {
    ((value + f64::EPSILON) * 100.0).round() / 100.0
}

/// Folds one base/override pair into `(min factor, max rounding margin)`.
/// Returns false when the pair leaves the domain where the factor is a
/// monotone lower bound; the caller then disables the axis prefilter.
fn fold_aim_scale(acc: &mut (f64, f64), base: Option<f64>, current: Option<f64>) -> bool {
    let Some(base) = base else {
        return true;
    };
    if !base.is_finite() || current.is_some_and(|c| !c.is_finite()) {
        return false;
    }
    if base == 0.0 {
        if current.is_some_and(|c| c != 0.0) {
            acc.0 = 0.0;
        }
        return true;
    }
    let rounded = aim_round2(base);
    if rounded == 0.0 {
        let factor = current.map_or(1.0, |c| c / base);
        acc.0 = acc.0.min(factor);
        return true;
    }
    if rounded < 0.0 || current.is_some_and(|c| c < 0.0) {
        return false;
    }
    let factor = current.map_or(1.0, |c| c / base);
    let margin = 0.005 / rounded + 0.005;
    acc.0 = acc.0.min(factor);
    acc.1 = acc.1.max(margin);
    true
}

fn aim_scale_floor(payload: &WeaponPayload, weapon_id: &str, mode: &str) -> AimScaleFloor {
    let mut floor = AimScaleFloor::identity();
    let Some(base) = payload
        .numeric_profiles
        .get(weapon_id)
        .map(|p| &p.linked_tables.aim_recoil)
    else {
        return floor;
    };
    let mut h = (1.0f64, 0.0f64);
    let mut v = (1.0f64, 0.0f64);
    if !fold_aim_scale(
        &mut h,
        base.continue_shoot.horizontal_scale,
        base.continue_shoot.horizontal_scale,
    ) || !fold_aim_scale(
        &mut h,
        base.single_or_burst.horizontal_scale,
        base.single_or_burst.horizontal_scale,
    ) || !fold_aim_scale(
        &mut v,
        base.continue_shoot.vertical_scale,
        base.continue_shoot.vertical_scale,
    ) || !fold_aim_scale(
        &mut v,
        base.single_or_burst.vertical_scale,
        base.single_or_burst.vertical_scale,
    ) {
        floor.enabled = false;
        return floor;
    }
    for part in payload.parts.values() {
        let Some(over) = part.aim_recoil_profiles.get(mode) else {
            continue;
        };
        if !fold_aim_scale(
            &mut h,
            base.continue_shoot.horizontal_scale,
            over.continue_shoot.horizontal_scale,
        ) || !fold_aim_scale(
            &mut h,
            base.single_or_burst.horizontal_scale,
            over.single_or_burst.horizontal_scale,
        ) || !fold_aim_scale(
            &mut v,
            base.continue_shoot.vertical_scale,
            over.continue_shoot.vertical_scale,
        ) || !fold_aim_scale(
            &mut v,
            base.single_or_burst.vertical_scale,
            over.single_or_burst.vertical_scale,
        ) {
            floor.enabled = false;
            return floor;
        }
    }
    floor.h_factor = h.0;
    floor.v_factor = v.0;
    floor.h_margin = h.1;
    floor.v_margin = v.1;
    floor
}

/// Reject a fully assembled DP state when even the best possible tuning cannot
/// bring recoil under a max constraint. `ExactPart.eh/ev/ek` are per-part maxima
/// over every tuning value, so `-eh/100` is the largest recoil reduction the
/// tuning pass could apply; the result is a lower bound on the real recoil.
/// The raw recoil is additionally scaled by the aim profile override, so the
/// bound uses the smallest multiplier any candidate part could apply.
#[allow(clippy::too_many_arguments)]
fn recoil_maxima(constraints: &[Constraint]) -> (Option<f64>, Option<f64>, Option<f64>) {
    let mut h_max: Option<f64> = None;
    let mut v_max: Option<f64> = None;
    let mut k_max: Option<f64> = None;
    for c in constraints {
        let Some(max) = c.max else { continue };
        let slot = match c.id.as_str() {
            "recoil-continuous-horizontal" | "recoil-single-horizontal" => &mut h_max,
            "recoil-continuous-vertical" | "recoil-single-vertical" => &mut v_max,
            "recoil-fire-stability" => &mut k_max,
            _ => continue,
        };
        *slot = Some(slot.map_or(max, |cur| cur.min(max)));
    }
    (h_max, v_max, k_max)
}

fn aim_time_max(constraints: &[Constraint]) -> Option<f64> {
    constraints
        .iter()
        .find(|c| c.id == "handling-aim-time" && c.min.is_none())
        .and_then(|c| c.max)
}

fn relative_margin(max: f64, value: f64) -> f64 {
    (max - value) / max.abs().max(1e-6)
}

/// Slack of the best possible completion of `metric` against every recoil
/// maximum, using the per-channel tuning maxima. Positive values are
/// optimistically feasible. `INFINITY` means no recoil maximum applies.
fn optimistic_recoil_margin(
    metric: &ExactPart,
    h_max: Option<f64>,
    v_max: Option<f64>,
    k_max: Option<f64>,
    control: Option<&Curve>,
    gunkick: Option<&Curve>,
    gunkick0: f64,
    aim: &AimScaleFloor,
) -> f64 {
    let mut margin = f64::INFINITY;
    let cr = control.and_then(|c| c.eval(metric.d)).unwrap_or(1.0);
    if let Some(max) = h_max {
        if aim.enabled {
            let f_h = cr * (1.0 + metric.ah - metric.eh / 100.0) * metric.ch;
            let lower = f_h * aim.h_factor - aim.h_margin;
            margin = margin.min(relative_margin(max, lower));
        }
    }
    if let Some(max) = v_max {
        if aim.enabled {
            let f_v = cr * (1.0 + metric.av - metric.ev / 100.0) * metric.cv;
            let lower = f_v * aim.v_factor - aim.v_margin;
            margin = margin.min(relative_margin(max, lower));
        }
    }
    if let Some(max) = k_max {
        if let Some(gk) = gunkick.and_then(|c| c.eval(metric.d)) {
            if gunkick0 != 0.0 {
                let stability = gk / gunkick0 * (1.0 - metric.ek / 100.0) * metric.ck;
                let rounded0 = ((gunkick0 + f64::EPSILON) * 100.0).round() / 100.0;
                let lower = if rounded0 == 0.0 {
                    stability
                } else {
                    stability - (0.005 / rounded0.abs() + 0.005)
                };
                margin = margin.min(relative_margin(max, lower));
            }
        }
    }
    margin
}

fn seed_candidate_margin(
    metric: &ExactPart,
    h_max: Option<f64>,
    v_max: Option<f64>,
    k_max: Option<f64>,
    aim_max: Option<f64>,
    control: Option<&Curve>,
    gunkick: Option<&Curve>,
    gunkick0: f64,
    aim: &AimScaleFloor,
    evaluator: &Evaluator,
) -> f64 {
    let mut margin =
        optimistic_recoil_margin(metric, h_max, v_max, k_max, control, gunkick, gunkick0, aim);
    if let Some(max) = aim_max {
        if let Some(value) = evaluator.aim_time(metric.hs, metric.adsm, metric.eopen.max(0.0)) {
            margin = margin.min(relative_margin(max, value));
        }
    }
    if margin.is_finite() {
        margin
    } else {
        0.0
    }
}

/// Orders incumbent candidates: optimistically feasible states first (they are
/// the ones whose tuning maxima can still meet every bound), cheapest first.
fn incumbent_eval_order(beam: &[DpState], margins: &[f64]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..beam.len()).collect();
    order.sort_by(|&a, &b| {
        let fa = margins.get(a).copied().unwrap_or(f64::NEG_INFINITY) >= 0.0;
        let fb = margins.get(b).copied().unwrap_or(f64::NEG_INFINITY) >= 0.0;
        fb.cmp(&fa)
            .then_with(|| beam[a].cost.cmp(&beam[b].cost))
            .then_with(|| a.cmp(&b))
    });
    order
}

fn optimistic_recoil_exceeds(
    metric: &ExactPart,
    h_max: Option<f64>,
    v_max: Option<f64>,
    k_max: Option<f64>,
    control: Option<&Curve>,
    gunkick: Option<&Curve>,
    gunkick0: f64,
    aim: &AimScaleFloor,
) -> bool {
    if h_max.is_none() && v_max.is_none() && k_max.is_none() {
        return false;
    }
    let Some(cr) = control.and_then(|c| c.eval(metric.d)) else {
        return false;
    };
    if let Some(max) = h_max {
        if aim.enabled {
            let f_h = cr * (1.0 + metric.ah - metric.eh / 100.0) * metric.ch;
            let lower = f_h * aim.h_factor - aim.h_margin;
            if lower > max + 1e-9 {
                return true;
            }
        }
    }
    if let Some(max) = v_max {
        if aim.enabled {
            let f_v = cr * (1.0 + metric.av - metric.ev / 100.0) * metric.cv;
            let lower = f_v * aim.v_factor - aim.v_margin;
            if lower > max + 1e-9 {
                return true;
            }
        }
    }
    if let Some(max) = k_max {
        let Some(gk) = gunkick.and_then(|c| c.eval(metric.d)) else {
            return false;
        };
        if gunkick0 != 0.0 {
            let s = gk / gunkick0 * (1.0 - metric.ek / 100.0) * metric.ck;
            let rounded0 = ((gunkick0 + f64::EPSILON) * 100.0).round() / 100.0;
            let lower = if rounded0 == 0.0 {
                s
            } else {
                s - (0.005 / rounded0.abs() + 0.005)
            };
            if lower > max + 1e-9 {
                return true;
            }
        }
    }
    false
}

fn metric_value(id: &str, metrics: &crate::metrics::BuildMetrics) -> Option<f64> {
    match id {
        "recoil-continuous-horizontal" => Some(metrics.recoil.continuous_h),
        "recoil-continuous-vertical" => Some(metrics.recoil.continuous_v),
        "recoil-single-horizontal" => Some(metrics.recoil.single_h),
        "recoil-single-vertical" => Some(metrics.recoil.single_v),
        "recoil-fire-stability" => Some(metrics.recoil.fire_stability),
        other => metrics
            .detail
            .value(other)
            .or_else(|| metrics.panel.get(other).copied()),
    }
}

fn normalize_attr(attr: Option<&str>) -> &str {
    match attr {
        Some("100103") => "10001",
        Some(v) => v,
        None => "",
    }
}

fn is_recoil_target(target: &str) -> bool {
    target.starts_with("GGunkick")
        || target.starts_with("GRecoil_")
        || target.ends_with("BreathFactor")
}

fn is_bipod(part: &Part) -> bool {
    for list in part.ui_effects.values() {
        if list.iter().any(|e| {
            e.condition_id
                .as_ref()
                .map(|v| v.is("10047"))
                .unwrap_or(false)
        }) {
            return true;
        }
    }
    if !part.is_virtual {
        return false;
    }
    let mut targets = FxHashSet::default();
    for list in part.raw_mods.values() {
        for m in list {
            if m.modifier_type == "Mult_C" && is_recoil_target(&m.target_path) {
                targets.insert(m.target_path.as_str());
            }
        }
    }
    targets.iter().any(|t| t.starts_with("GGunkick"))
        && targets.contains("GRecoil_H")
        && targets.contains("GRecoil_V")
        && targets.iter().any(|t| t.ends_with("StandBreathFactor"))
}

/// Two locked paths can be mutually exclusive regardless of every other
/// choice: a locked part that claims the other lock's socket through
/// `extraOccupySocketIds` is installed before that slot is walked, so the
/// resolver drops the other lock and no completion can restore it.
fn locked_paths_conflict(
    payload: &WeaponPayload,
    locked_paths: &FxHashMap<String, Option<String>>,
) -> bool {
    let entries: Vec<(Option<String>, Vec<&str>)> = locked_paths
        .iter()
        .map(|(path, part)| {
            let claimed = part
                .as_ref()
                .and_then(|id| payload.parts.get(id))
                .map(|part| {
                    part.extra_occupy_socket_ids
                        .iter()
                        .map(String::as_str)
                        .collect()
                })
                .unwrap_or_default();
            (crate::assembly::socket_id_from_path(path), claimed)
        })
        .collect();
    for (idx, (socket, claimed)) in entries.iter().enumerate() {
        for (other_socket, other_claimed) in &entries[idx + 1..] {
            let (Some(socket), Some(other_socket)) = (socket, other_socket) else {
                continue;
            };
            if claimed.contains(&other_socket.as_str())
                || other_claimed.contains(&socket.as_str())
            {
                return true;
            }
        }
    }
    false
}

fn compute_relevance(
    payload: &WeaponPayload,
    weapon_id: &str,
    constraints: &[Constraint],
) -> Relevance {
    let mut parts: FxHashSet<String> = FxHashSet::default();
    let mut direct: FxHashSet<String> = FxHashSet::default();
    let mut attribute_constraints: FxHashSet<String> = FxHashSet::default();
    for c in constraints {
        if c.id.chars().all(|ch| ch.is_ascii_digit()) {
            attribute_constraints.insert(c.id.clone());
        }
    }
    let detail_ids: FxHashSet<&str> = constraints
        .iter()
        .map(|c| c.id.as_str())
        .filter(|id| DETAIL_METRICS.contains(id))
        .collect();
    let want_handling = [
        "handling-aim-time",
        "handling-sprint-fire-delay",
        "handling-aim-move-speed",
    ]
    .iter()
    .any(|id| detail_ids.contains(id));
    let want_reload = ["handling-reload-time", "handling-empty-reload-time"]
        .iter()
        .any(|id| detail_ids.contains(id));
    let want_stability = [
        "stability-breath-range",
        "stability-breath-duration",
        "stability-movement-camera",
        "stability-fire-camera",
        "stability-hit-sway",
        "stability-hold-breath-recoil",
    ]
    .iter()
    .any(|id| detail_ids.contains(id));
    let want_hipfire = ["hipfire-standing-spread", "hipfire-maximum-spread"]
        .iter()
        .any(|id| detail_ids.contains(id));
    let want_handling_tune = want_handling || detail_ids.contains("handling-aim-move-speed");
    let want_stability_tune = want_stability;

    for (id, part) in &payload.parts {
        let mut relevant = !part.extra_occupy_socket_ids.is_empty();
        for list in part.main_mods.values().chain(
            part.main_baseline_mods
                .get(weapon_id)
                .into_iter()
                .flat_map(|m| m.values()),
        ) {
            for m in list {
                if m.condition_id.is_none()
                    && m.modifier_type == "Addend"
                    && normalize_attr(m.attribute_id.as_deref()) == "10005"
                {
                    relevant = true;
                }
                if want_handling
                    && m.condition_id.is_none()
                    && m.modifier_type == "Addend"
                    && HANDLING_ATTRS.contains(&normalize_attr(m.attribute_id.as_deref()))
                {
                    relevant = true;
                }
            }
        }
        if want_handling || want_stability || want_hipfire {
            for list in part.display_mods.values().chain(
                part.display_baseline_mods
                    .get(weapon_id)
                    .into_iter()
                    .flat_map(|m| m.values()),
            ) {
                for m in list {
                    if m.condition_id.is_some() || m.modifier_type != "Addend" {
                        continue;
                    }
                    let attr = normalize_attr(m.attribute_id.as_deref());
                    if (want_stability && STABILITY_ATTRS.contains(&attr))
                        || (want_hipfire && HIPFIRE_ATTRS.contains(&attr))
                    {
                        relevant = true;
                    }
                }
            }
        }
        for rr in part.raw_recoil.values() {
            if rr.horizontal.map(|v| v != 0.0).unwrap_or(false)
                || rr.vertical.map(|v| v != 0.0).unwrap_or(false)
                || rr
                    .gunkick_spring_mult_c
                    .map(|v| v != 0.0 && v.is_finite())
                    .unwrap_or(false)
            {
                relevant = true;
            }
        }
        for list in part.raw_mods.values() {
            for m in list {
                if m.modifier_type == "Mult_C" && is_recoil_target(&m.target_path) {
                    relevant = true;
                }
                if want_handling
                    && ((m.target_path == "GAiming_ADSTime" && m.modifier_type == "Mult_A")
                        || (m.target_path == "GMovement_ADSSpeed" && m.modifier_type == "Addend"))
                {
                    relevant = true;
                }
                if want_reload && m.modifier_type == "Mult_A" && m.target_path == "GReload_Time" {
                    relevant = true;
                }
            }
        }
        if want_reload {
            for list in part.runtime_mods.values() {
                if list.iter().any(|m| {
                    m.target == "GReload_Time"
                        || m.target == "ChangeClipTime"
                        || m.target == "ChangeClipTimeWhenEmpty"
                        || m.target == "ChangeClipTimewhenEmpty"
                }) {
                    relevant = true;
                }
            }
        }
        if want_stability
            && part
                .raw_gun_sway
                .values()
                .any(|v| v.map(|x| x != 0.0 && x.is_finite()).unwrap_or(false))
        {
            relevant = true;
        }
        for tune in &part.tuning_options {
            for effect in &tune.display_effects {
                let name = effect.attribute_name_snapshot.as_str();
                if name == "%额外后坐力控制" || name == "%开火时武器稳定性" {
                    relevant = true;
                }
                if want_handling_tune
                    && (name.contains("开镜速度")
                        || name.contains("举镜瞄准速度")
                        || name.contains("举枪瞄准速度")
                        || name.contains("移动速度"))
                {
                    relevant = true;
                }
                if want_stability_tune
                    && (name.contains("瞄准时呼吸稳定性") || name.contains("移动时据枪稳定性"))
                {
                    relevant = true;
                }
            }
        }
        if !part.aim_recoil_profiles.is_empty() {
            relevant = true;
        }
        if !attribute_constraints.is_empty() {
            for list in part.display_mods.values().chain(
                part.display_baseline_mods
                    .get(weapon_id)
                    .into_iter()
                    .flat_map(|m| m.values()),
            ) {
                for m in list {
                    if attribute_constraints.contains(normalize_attr(m.attribute_id.as_deref())) {
                        relevant = true;
                    }
                }
            }
        }
        if relevant {
            direct.insert(id.clone());
        }
    }

    // Any part referenced by a virtual rule condition can change virtual selection.
    if let Some(rules) = payload.virtual_rules.get(weapon_id) {
        for rule in rules {
            for id in &rule.condition_item_ids {
                direct.insert(id.clone());
            }
        }
    }

    parts.extend(direct);

    // Fixpoint: a part is relevant if any slot it hosts has a relevant option.
    let mut changed = true;
    while changed {
        changed = false;
        for (host, slots) in &payload.slots_by_host {
            for slot in slots {
                let mut slot_relevant = false;
                for id in &slot.candidate_ids {
                    if parts.contains(id) {
                        slot_relevant = true;
                        break;
                    }
                }
                if !slot_relevant {
                    for id in &slot.virtual_candidate_ids {
                        if parts.contains(id) {
                            slot_relevant = true;
                            break;
                        }
                    }
                }
                if !slot_relevant {
                    if let Some(rules) = payload.virtual_rules.get(weapon_id) {
                        for rule in rules {
                            if rule.socket_id == slot.id && parts.contains(&rule.virtual_part_id) {
                                slot_relevant = true;
                                break;
                            }
                        }
                    }
                }
                if slot_relevant && parts.insert(host.clone()) {
                    changed = true;
                }
            }
        }
    }

    let mut slots: FxHashMap<String, FxHashSet<String>> = FxHashMap::default();
    for (host, list) in &payload.slots_by_host {
        for slot in list {
            let mut slot_relevant = slot.candidate_ids.iter().any(|id| parts.contains(id))
                || slot
                    .virtual_candidate_ids
                    .iter()
                    .any(|id| parts.contains(id));
            if !slot_relevant {
                if let Some(rules) = payload.virtual_rules.get(weapon_id) {
                    slot_relevant = rules
                        .iter()
                        .any(|r| r.socket_id == slot.id && parts.contains(&r.virtual_part_id));
                }
            }
            if slot_relevant {
                slots
                    .entry(host.clone())
                    .or_default()
                    .insert(slot.id.clone());
            }
        }
    }

    // A default preset part is not a slot candidate, but its slot still hosts
    // it as an internal part, and keeping it can be what satisfies a
    // constraint. Mark that slot relevant so the search can choose between
    // keeping and removing the default.
    if let Some(preset) = payload.default_presets.get(weapon_id) {
        for node in &preset.nodes {
            let Some(path) = &node.path_key else {
                continue;
            };
            if !parts.contains(&node.part_id) {
                continue;
            }
            let Some(slot_id) = crate::assembly::socket_id_from_path(path) else {
                continue;
            };
            let host = match path.rfind("/part:") {
                Some(idx) => {
                    let encoded = &path[idx + 6..];
                    match payload
                        .parts
                        .keys()
                        .find(|id| crate::assembly::enc(id) == encoded)
                    {
                        Some(id) => id.clone(),
                        None => continue,
                    }
                }
                None => weapon_id.to_string(),
            };
            slots.entry(host).or_default().insert(slot_id);
        }
    }

    Relevance { slots }
}

impl Constraint {
    pub fn new(id: impl Into<String>, min: Option<f64>, max: Option<f64>) -> Self {
        Self {
            id: id.into(),
            min,
            max,
        }
    }
}

#[allow(dead_code)]
fn curve_value(curve: Option<&Curve>, x: f64) -> Option<f64> {
    curve.and_then(|c| c.eval(x))
}

#[allow(dead_code)]
fn unused(_: &WeaponPayload, _: &TuningInstance, _: &Assembler, _: &[Curve], _: &Part) {
    let _ = eval_curve_points(&[], 0.0);
    let _ = quantize(0.0, 0.0, 1.0, 1.0);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn part(d: f64, cost: i64, panel: Vec<f64>) -> DpState {
        DpState {
            cost,
            metric: ExactPart {
                d,
                ch: 1.0,
                cv: 1.0,
                ck: 1.0,
                panel,
                ..ExactPart::neutral()
            },
            detail_pro: None,
            panel_pro: None,
            choices: Vec::new(),
        }
    }

    #[test]
    fn dp_suffix_bounds_accumulate_extrema_and_products() {
        let opt = |d: f64, ah: f64, ch: f64, eh: f64, hs: f64, adsm: f64, eopen: f64| DpOption {
            cost: 0,
            metric: ExactPart {
                d,
                ah,
                ch,
                eh,
                hs,
                adsm,
                eopen,
                ..ExactPart::neutral()
            },
            choice: SlotChoice::Unset,
            laser_pro: None,
            visible: false,
        };
        let slots = vec![
            vec![
                opt(2.0, 0.1, 0.9, 5.0, 1.0, 0.05, 3.0),
                opt(-1.0, -0.2, 1.0, 2.0, 0.0, 0.0, 1.0),
            ],
            vec![opt(3.0, 0.3, 0.8, 4.0, 2.0, 0.1, 2.0)],
        ];
        let b = dp_suffix_bounds(&slots);
        assert_eq!(b[2].d_max, 0.0);
        assert_eq!(b[1].d_max, 3.0);
        assert_eq!(b[1].d_min, 3.0);
        assert!((b[0].d_max - 5.0).abs() < 1e-12);
        assert!((b[0].d_min - 2.0).abs() < 1e-12);
        assert!((b[0].ah_min - 0.1).abs() < 1e-12);
        assert!((b[0].ch_min - 0.72).abs() < 1e-12);
        assert!((b[0].eh_max - 9.0).abs() < 1e-12);
        assert!((b[0].hs_max - 3.0).abs() < 1e-12);
        assert!((b[0].adsm_min - 0.1).abs() < 1e-12);
        assert!((b[0].eopen_max - 5.0).abs() < 1e-12);
        assert!(!b[0].ch_negative);
        let negative = vec![vec![opt(0.0, 0.0, -1.0, 0.0, 0.0, 0.0, 0.0)]];
        assert!(dp_suffix_bounds(&negative)[0].ch_negative);
    }

    fn tuning_instance<'a>(slot_path: &str, slot_name: &str, part: &'a Part) -> TuningInstance<'a> {
        TuningInstance {
            slot_path: slot_path.to_string(),
            slot_name: slot_name.to_string(),
            part,
        }
    }

    fn tuning_part(id: &str, tune_ids: &[&str]) -> Part {
        Part {
            id: id.to_string(),
            tuning_options: tune_ids
                .iter()
                .map(|tune_id| crate::model::TuningOption {
                    tune_id: (*tune_id).to_string(),
                    ..Default::default()
                })
                .collect(),
            ..serde_json::from_value(serde_json::json!({ "id": id })).unwrap()
        }
    }

    fn curve(points: &[(f64, f64)]) -> Curve {
        Curve {
            keys: points
                .iter()
                .map(|(time, value)| crate::model::CurveKey {
                    time: *time,
                    value: *value,
                    interp_mode: "RCIM_Linear".to_string(),
                    leave_tangent: 0.0,
                    arrive_tangent: 0.0,
                    key_index: 0,
                })
                .collect(),
        }
    }

    fn state_metric(d: f64, ah: f64, ch: f64, eh: f64) -> ExactPart {
        ExactPart {
            d,
            ah,
            ch,
            eh,
            cv: 1.0,
            ck: 1.0,
            panel: Vec::new(),
            ..ExactPart::neutral()
        }
    }

    #[test]
    fn optimistic_prefilter_rejects_only_unreachable_states() {
        let control = curve(&[(0.0, 1.0), (100.0, 0.5)]);
        let gunkick = curve(&[(0.0, 1.0), (100.0, 0.5)]);
        // f_h = 1.0 * (1 + 0 - 0.5) * 1 = 0.5
        let tuned = state_metric(0.0, 0.0, 1.0, 50.0);
        assert!(!optimistic_recoil_exceeds(
            &tuned,
            Some(0.55),
            None,
            None,
            Some(&control),
            Some(&gunkick),
            1.0,
            &AimScaleFloor::identity(),
        ));
        assert!(optimistic_recoil_exceeds(
            &tuned,
            Some(0.49),
            None,
            None,
            Some(&control),
            Some(&gunkick),
            1.0,
            &AimScaleFloor::identity(),
        ));
        // Without tuning headroom the same state is already at 1.0.
        let untuned = state_metric(0.0, 0.0, 1.0, 0.0);
        assert!(optimistic_recoil_exceeds(
            &untuned,
            Some(0.9),
            None,
            None,
            Some(&control),
            Some(&gunkick),
            1.0,
            &AimScaleFloor::identity(),
        ));
    }

    #[test]
    fn optimistic_prefilter_is_disabled_without_curves_or_bounds() {
        let state = state_metric(0.0, 0.0, 1.0, 0.0);
        assert!(!optimistic_recoil_exceeds(
            &state,
            None,
            None,
            None,
            None,
            None,
            1.0,
            &AimScaleFloor::identity(),
        ));
        assert!(!optimistic_recoil_exceeds(
            &state,
            Some(0.1),
            None,
            None,
            None,
            None,
            1.0,
            &AimScaleFloor::identity(),
        ));
    }

    #[test]
    fn optimistic_prefilter_honors_aim_scale_overrides() {
        let control = curve(&[(0.0, 0.5), (100.0, 0.5)]);
        let gunkick = curve(&[(0.0, 1.0), (100.0, 0.5)]);
        // f_v = 0.5 and no tuning headroom.
        let state = state_metric(0.0, 0.0, 1.0, 0.0);
        assert!(optimistic_recoil_exceeds(
            &state,
            None,
            Some(0.46),
            None,
            Some(&control),
            Some(&gunkick),
            1.0,
            &AimScaleFloor::identity(),
        ));
        // A candidate override with VerticalScale 0.8 lowers the real value to
        // 0.4, so the same state is feasible and must not be rejected.
        let lowered = AimScaleFloor {
            v_factor: 0.8,
            v_margin: 0.01,
            ..AimScaleFloor::identity()
        };
        assert!(!optimistic_recoil_exceeds(
            &state,
            None,
            Some(0.46),
            None,
            Some(&control),
            Some(&gunkick),
            1.0,
            &lowered,
        ));
        // Even the scaled value is too high here.
        assert!(optimistic_recoil_exceeds(
            &state,
            None,
            Some(0.3),
            None,
            Some(&control),
            Some(&gunkick),
            1.0,
            &lowered,
        ));
    }

    #[test]
    fn optimistic_prefilter_keeps_fire_stability_rounding_slack() {
        let control = curve(&[(0.0, 1.0), (100.0, 1.0)]);
        let gunkick = curve(&[(0.0, 1.0), (100.0, 1.0)]);
        let mut state = state_metric(0.0, 0.0, 1.0, 0.0);
        state.ck = 0.772;
        // The real metric is round2(round2(1.0) * 0.772) / 1.0 = 0.77, so the
        // state is feasible even though the raw s is above the constraint.
        assert!(!optimistic_recoil_exceeds(
            &state,
            None,
            None,
            Some(0.77),
            Some(&control),
            Some(&gunkick),
            1.0,
            &AimScaleFloor::identity(),
        ));
        state.ck = 0.9;
        assert!(optimistic_recoil_exceeds(
            &state,
            None,
            None,
            Some(0.77),
            Some(&control),
            Some(&gunkick),
            1.0,
            &AimScaleFloor::identity(),
        ));
    }

    #[test]
    fn aim_scale_floor_takes_minimum_over_candidate_overrides() {
        let payload: WeaponPayload = serde_json::from_value(serde_json::json!({
            "parts": {
                "w": { "id": "w" },
                "p1": {
                    "id": "p1",
                    "aimRecoilProfilesByMode": {
                        "operations": {
                            "ContinueShootRecoil": {
                                "HorizontalScale": 0.8,
                                "VerticalScale": 1.0
                            },
                            "SingleOrBurstShootRecoil": {
                                "HorizontalScale": 0.9,
                                "VerticalScale": 0.8
                            }
                        }
                    }
                },
                "p2": {
                    "id": "p2",
                    "aimRecoilProfilesByMode": {
                        "operations": {
                            "ContinueShootRecoil": {
                                "HorizontalScale": 0.5,
                                "VerticalScale": 1.2
                            },
                            "SingleOrBurstShootRecoil": {
                                "HorizontalScale": 1.0,
                                "VerticalScale": 1.0
                            }
                        }
                    }
                }
            },
            "weaponNumericProfilesById": {
                "w": {
                    "linkedTables": {
                        "aimRecoil": {
                            "ContinueShootRecoil": {
                                "HorizontalScale": 1.0,
                                "VerticalScale": 1.0
                            },
                            "SingleOrBurstShootRecoil": {
                                "HorizontalScale": 1.0,
                                "VerticalScale": 1.0
                            }
                        }
                    }
                }
            }
        }))
        .unwrap();
        let floor = aim_scale_floor(&payload, "w", "operations");
        assert!(floor.enabled);
        assert!((floor.h_factor - 0.5).abs() < 1e-9);
        assert!((floor.v_factor - 0.8).abs() < 1e-9);
        assert!(floor.h_margin > 0.0);
        assert!(floor.v_margin > 0.0);
    }

    #[test]
    fn aim_scale_floor_treats_zero_base_as_zero_metric() {
        let payload: WeaponPayload = serde_json::from_value(serde_json::json!({
            "parts": {
                "w": { "id": "w" },
                "p": {
                    "id": "p",
                    "aimRecoilProfilesByMode": {
                        "operations": {
                            "ContinueShootRecoil": {
                                "HorizontalScale": 0.5,
                                "VerticalScale": 0.5
                            }
                        }
                    }
                }
            },
            "weaponNumericProfilesById": {
                "w": {
                    "linkedTables": {
                        "aimRecoil": {
                            "ContinueShootRecoil": {
                                "HorizontalScale": 0.0,
                                "VerticalScale": 0.0
                            },
                            "SingleOrBurstShootRecoil": {
                                "HorizontalScale": 0.0,
                                "VerticalScale": 0.0
                            }
                        }
                    }
                }
            }
        }))
        .unwrap();
        // scale_factor(Some(0), Some(nonzero)) is 0, so the real metric is 0.
        let floor = aim_scale_floor(&payload, "w", "operations");
        assert!(floor.enabled);
        assert_eq!(floor.h_factor, 0.0);
        assert_eq!(floor.v_factor, 0.0);
        // No override means scale_factor(Some(0), None) is 1 and scale_ratio
        // returns the raw multiplier, so nothing is scaled.
        let payload: WeaponPayload = serde_json::from_value(serde_json::json!({
            "parts": { "w": { "id": "w" } },
            "weaponNumericProfilesById": {
                "w": {
                    "linkedTables": {
                        "aimRecoil": {
                            "ContinueShootRecoil": {
                                "HorizontalScale": 0.0,
                                "VerticalScale": 0.0
                            },
                            "SingleOrBurstShootRecoil": {
                                "HorizontalScale": 0.0,
                                "VerticalScale": 0.0
                            }
                        }
                    }
                }
            }
        }))
        .unwrap();
        let floor = aim_scale_floor(&payload, "w", "operations");
        assert!(floor.enabled);
        assert_eq!(floor.h_factor, 1.0);
        assert_eq!(floor.v_factor, 1.0);
        // No profile at all also means no scaling.
        let payload: WeaponPayload = serde_json::from_value(serde_json::json!({
            "parts": { "w": { "id": "w" } }
        }))
        .unwrap();
        let floor = aim_scale_floor(&payload, "w", "operations");
        assert!(floor.enabled);
        assert!((floor.h_factor - 1.0).abs() < 1e-12);
    }

    #[test]
    fn aim_scale_floor_folds_tiny_base_scales_without_rounding_margin() {
        let payload: WeaponPayload = serde_json::from_value(serde_json::json!({
            "parts": {
                "w": { "id": "w" },
                "p": {
                    "id": "p",
                    "aimRecoilProfilesByMode": {
                        "operations": {
                            "ContinueShootRecoil": {
                                "HorizontalScale": 0.002,
                                "VerticalScale": 0.002
                            }
                        }
                    }
                }
            },
            "weaponNumericProfilesById": {
                "w": {
                    "linkedTables": {
                        "aimRecoil": {
                            "ContinueShootRecoil": {
                                "HorizontalScale": 0.004,
                                "VerticalScale": 0.004
                            },
                            "SingleOrBurstShootRecoil": {
                                "HorizontalScale": 0.004,
                                "VerticalScale": 0.004
                            }
                        }
                    }
                }
            }
        }))
        .unwrap();
        let floor = aim_scale_floor(&payload, "w", "operations");
        assert!(floor.enabled);
        assert!((floor.h_factor - 0.5).abs() < 1e-9);
        assert!((floor.v_factor - 0.5).abs() < 1e-9);
        assert_eq!(floor.h_margin, 0.0);
        assert_eq!(floor.v_margin, 0.0);
    }

    #[test]
    fn relevance_matches_host_and_slot_without_allocation() {
        let mut relevance = Relevance::default();
        relevance
            .slots
            .entry("host-a".to_string())
            .or_default()
            .insert("slot-1".to_string());
        assert!(relevance.contains("host-a", "slot-1"));
        assert!(!relevance.contains("host-a", "slot-2"));
        assert!(!relevance.contains("host-b", "slot-1"));
    }

    #[test]
    fn tune_cache_key_tracks_instances_and_tunes() {
        let a = tuning_part("p1", &["t1"]);
        let b = tuning_part("p2", &["t2"]);
        let i1 = tuning_instance("weapon:w/slot:1:X", "X", &a);
        let i2 = tuning_instance("weapon:w/slot:1:X", "X", &b);
        let same = tune_cache_key(std::slice::from_ref(&i1));
        assert_eq!(same, tune_cache_key(std::slice::from_ref(&i1)));
        assert_ne!(same, tune_cache_key(std::slice::from_ref(&i2)));
        assert_ne!(
            tune_cache_key(&[i1]),
            tune_cache_key(&[tuning_instance("weapon:w/slot:2:Y", "Y", &a)])
        );
        let with_extra = tuning_part("p1", &["t1", "t3"]);
        assert_ne!(
            same,
            tune_cache_key(std::slice::from_ref(&tuning_instance(
                "weapon:w/slot:1:X",
                "X",
                &with_extra
            )))
        );
    }

    #[test]
    fn pareto_keeps_cheap_and_strong_states() {
        let cheap_weak = part(5.0, 5, vec![]);
        let pricey_strong = part(20.0, 10, vec![]);
        let dominated = part(10.0, 12, vec![]);
        let dims = DpDims {
            d: true,
            ah: true,
            av: true,
            ch: true,
            cv: true,
            ck: true,
            ..DpDims::default()
        };
        let out = dp_pareto(
            vec![dominated, pricey_strong, cheap_weak],
            &dims,
            &[],
            AimDir::Ignore,
        );
        assert_eq!(out.len(), 2);
        assert!(out.iter().any(|s| s.cost == 5));
        assert!(out.iter().any(|s| s.cost == 10));
    }

    #[test]
    fn pareto_uses_panel_values() {
        let lower = part(10.0, 10, vec![40.0]);
        let higher = part(10.0, 10, vec![50.0]);
        let mut dims = DpDims::default();
        dims.d = true;
        let out = dp_pareto(
            vec![higher, lower],
            &dims,
            &[PanelDir::Lower],
            AimDir::Ignore,
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].metric.panel[0], 40.0);
    }

    #[test]
    fn pareto_respects_panel_min_direction() {
        let lower = part(10.0, 10, vec![40.0]);
        let higher = part(10.0, 10, vec![50.0]);
        let out = dp_pareto(
            vec![lower, higher],
            &DpDims::default(),
            &[PanelDir::Higher],
            AimDir::Ignore,
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].metric.panel[0], 50.0);
    }

    #[test]
    fn pareto_ignores_unconstrained_dimensions() {
        let cheap_weak = part(5.0, 5, vec![]);
        let pricey_strong = part(20.0, 10, vec![]);
        let out = dp_pareto(
            vec![pricey_strong, cheap_weak],
            &DpDims::default(),
            &[],
            AimDir::Ignore,
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].cost, 5);
    }

    #[test]
    fn pareto_dedups_identical_metrics_and_keeps_the_cheapest() {
        let mut cheap = part(10.0, 5, vec![]);
        cheap.choices.push(SlotChoice::Part("a".to_string()));
        let mut pricey = part(10.0, 7, vec![]);
        pricey.choices.push(SlotChoice::Part("b".to_string()));
        let distinct = part(11.0, 6, vec![]);
        let dims = DpDims {
            d: true,
            ..DpDims::default()
        };
        let out = dp_pareto(vec![pricey, distinct, cheap], &dims, &[], AimDir::Ignore);
        assert_eq!(out.len(), 2);
        let deduped = out.iter().find(|s| s.metric.d == 10.0).unwrap();
        assert_eq!(deduped.cost, 5);
        assert!(matches!(&deduped.choices[0], SlotChoice::Part(id) if id == "a"));
    }

    fn laser_option(
        id: &str,
        cost: i64,
        pro_display: f64,
        pro_handling: f64,
        con_handling: f64,
    ) -> (DpOption, Part) {
        let part: Part = serde_json::from_value(serde_json::json!({
            "id": id,
            "modifiersByMode": {
                "operations": [{
                    "attributeId": "10008",
                    "polarity": "pro",
                    "modifierType": "Addend",
                    "delta": pro_display,
                    "targetPath": "DisplayAttrValues.4"
                }]
            },
            "mainAttributeModifiersByMode": {
                "operations": [{
                    "attributeId": "10006",
                    "polarity": "pro",
                    "modifierType": "Addend",
                    "delta": pro_handling,
                    "targetPath": "WeaponMainAttribute.MainAttrValues.5"
                }, {
                    "attributeId": "10006",
                    "polarity": "con",
                    "modifierType": "Addend",
                    "delta": con_handling,
                    "targetPath": "WeaponMainAttribute.MainAttrValues.5"
                }]
            },
            "uiEffectsByMode": {
                "operations": [{ "conditionId": "10038" }]
            }
        }))
        .unwrap();
        let mode = "operations";
        let is_laser = crate::metrics::has_laser(&part);
        assert!(is_laser);
        let panel_attrs = vec!["10008".to_string()];
        let option = DpOption {
            cost,
            metric: exact_part_metric(
                &part,
                "w1",
                "X",
                mode,
                false,
                true,
                false,
                true,
                &panel_attrs,
            ),
            laser_pro: Some(Rc::new(LaserPro {
                score: crate::metrics::laser_pro_score(&part, mode),
                metric: exact_laser_pro(&part, "w1", mode, &panel_attrs),
            })),
            visible: true,
            choice: SlotChoice::Part(id.to_string()),
        };
        (option, part)
    }

    #[test]
    fn dp_applies_laser_pro_once_for_two_candidates() {
        let mode = "operations";
        let panel_attrs = vec!["10008".to_string()];
        let (strong, strong_part) = laser_option("laser-a", 100, 12.0, 16.0, -4.0);
        let (weak, weak_part) = laser_option("laser-b", 200, 8.0, 10.0, -3.0);
        let start = DpState {
            cost: 0,
            metric: ExactPart::neutral_panel(1),
            detail_pro: None,
            panel_pro: None,
            choices: Vec::new(),
        };

        // The evaluator applies the modifiers of one laser, the best scoring
        // one, so the DP metric for both parts must match that single delta.
        let mut once = exact_part_metric(
            &strong_part,
            "w1",
            "X",
            mode,
            false,
            true,
            true,
            true,
            &panel_attrs,
        );
        once.add(&exact_part_metric(
            &weak_part,
            "w1",
            "X",
            mode,
            false,
            true,
            false,
            true,
            &panel_attrs,
        ));

        let forward = dp_advance(&dp_advance(&start, &strong, 100), &weak, 300);
        let reverse = dp_advance(&dp_advance(&start, &weak, 200), &strong, 300);
        assert!(metric_eq(&forward.metric, &once));
        assert!(metric_eq(&reverse.metric, &once));
        assert!((forward.metric.panel[0] - 12.0).abs() < 1e-9);
        // Both lasers keep their con handling penalties; only one pro applies.
        assert!((forward.metric.hs - 9.0).abs() < 1e-9);
        assert_eq!(forward.cost, 300);
        assert_eq!(forward.detail_pro.as_ref().unwrap().score, 12.0);
        assert_eq!(forward.panel_pro.as_ref().unwrap().score, 12.0);

        // A later, higher scoring laser replaces the current winner instead of
        // stacking on top of it.
        let (best, best_part) = laser_option("laser-c", 300, 20.0, 30.0, -1.0);
        let replaced = dp_advance(&forward, &best, 600);
        let mut expected = exact_part_metric(
            &best_part,
            "w1",
            "X",
            mode,
            false,
            true,
            true,
            true,
            &panel_attrs,
        );
        expected.add(&exact_part_metric(
            &weak_part,
            "w1",
            "X",
            mode,
            false,
            true,
            false,
            true,
            &panel_attrs,
        ));
        expected.add(&exact_part_metric(
            &strong_part,
            "w1",
            "X",
            mode,
            false,
            true,
            false,
            true,
            &panel_attrs,
        ));
        assert!(metric_eq(&replaced.metric, &expected));
        assert!((replaced.metric.panel[0] - 20.0).abs() < 1e-9);
        assert!((replaced.metric.hs - 22.0).abs() < 1e-9);

        // Ties keep the first part, matching `laser_pro_index`.
        let (tie, _) = laser_option("laser-tie", 400, 12.0, 99.0, 0.0);
        let tied = dp_advance(&forward, &tie, 700);
        assert!((tied.metric.panel[0] - 12.0).abs() < 1e-9);
        assert!((tied.metric.hs - 9.0).abs() < 1e-9);
    }

    #[test]
    fn internal_laser_wins_detail_pro_but_not_panel_pro() {
        let (strong, _) = laser_option("laser-a", 100, 12.0, 16.0, -4.0);
        let (weak, _) = laser_option("laser-b", 200, 8.0, 10.0, -3.0);
        let mut internal = strong;
        internal.visible = false;
        let start = DpState {
            cost: 0,
            metric: ExactPart::neutral_panel(1),
            detail_pro: None,
            panel_pro: None,
            choices: Vec::new(),
        };

        let state = dp_advance(&dp_advance(&start, &weak, 200), &internal, 300);
        // Detail ranks every installed part, so the internal laser takes the
        // pro slot and the visible laser keeps only its con penalty.
        assert_eq!(state.detail_pro.as_ref().unwrap().score, 12.0);
        assert!((state.metric.hs - 9.0).abs() < 1e-9);
        // The panel never sees internal parts, so the visible laser keeps the
        // panel pro slot.
        assert_eq!(state.panel_pro.as_ref().unwrap().score, 8.0);
        assert!((state.metric.panel[0] - 8.0).abs() < 1e-9);
    }

    #[test]
    fn metric_hash_distinguishes_multiplicative_dims() {
        let mut a = ExactPart::neutral();
        let mut b = ExactPart::neutral();
        b.ch = 0.5;
        assert_ne!(metric_hash(&a), metric_hash(&b));
        assert_eq!(metric_hash(&a), metric_hash(&a.clone()));
        a.panel = vec![1.0, 2.0];
        b.panel = vec![1.0, 3.0];
        assert_ne!(metric_hash(&a), metric_hash(&b));
        let mut c = ExactPart::neutral();
        c.ck = 0.25;
        assert_ne!(metric_hash(&a), metric_hash(&c));
    }

    #[test]
    fn dedup_requires_identical_tuning_envelopes() {
        let m = ExactPart::neutral();
        assert!(metric_eq(&m, &m.clone()));
        for set in [
            |v: &mut ExactPart| v.eh = 5.0,
            |v: &mut ExactPart| v.ev = 5.0,
            |v: &mut ExactPart| v.ek = 5.0,
            |v: &mut ExactPart| v.eopen = 1.0,
        ] {
            let mut with_headroom = ExactPart::neutral();
            set(&mut with_headroom);
            assert!(!metric_eq(&m, &with_headroom));
        }
        let mut tuned_a = ExactPart::neutral();
        tuned_a.eh = 1.0;
        tuned_a.e_front = Some(Rc::new(cap_front(vec![[1.0, 0.0, 0.0, 0.0]])));
        let mut tuned_b = ExactPart::neutral();
        tuned_b.eh = 1.0;
        tuned_b.e_front = Some(Rc::new(cap_front(vec![[0.0, 0.0, 0.0, 1.0]])));
        assert_ne!(metric_hash(&tuned_a), metric_hash(&tuned_b));
        assert!(!metric_eq(&tuned_a, &tuned_b));
        let mut tuned_c = ExactPart::neutral();
        tuned_c.eh = 1.0;
        tuned_c.e_front = tuned_a.e_front.clone();
        assert_eq!(metric_hash(&tuned_a), metric_hash(&tuned_c));
        assert!(metric_eq(&tuned_a, &tuned_c));
    }

    #[test]
    fn metric_bounds_combine() {
        let mut a = MetricBound::neutral();
        a.max_d = 3.0;
        a.min_c_h = 0.8;
        let mut b = MetricBound::neutral();
        b.max_d = 4.0;
        b.min_c_h = 0.9;
        a.add_a(&b);
        assert!((a.max_d - 7.0).abs() < 1e-9);
        assert!((a.min_c_h - 0.72).abs() < 1e-9);
    }

    #[test]
    fn seed_aim_weight_tracks_constraint_direction() {
        assert_eq!(
            seed_aim_weight(&[Constraint::new("handling-aim-time", None, Some(400.0))]),
            1.0
        );
        assert_eq!(
            seed_aim_weight(&[Constraint::new("handling-aim-time", Some(300.0), None)]),
            -1.0
        );
        assert_eq!(
            seed_aim_weight(&[Constraint::new(
                "handling-aim-time",
                Some(300.0),
                Some(400.0)
            )]),
            0.0
        );
        assert_eq!(seed_aim_weight(&[]), 0.0);
    }

    #[test]
    fn seed_ranking_prefers_constraint_gain_then_price() {
        let ranked = rank_seed_candidates(vec![
            ("cheap".to_string(), 1.0, 10),
            ("strong".to_string(), 5.0, 90),
            ("strong-cheap".to_string(), 5.0, 20),
        ]);
        assert_eq!(ranked, vec!["strong-cheap", "strong", "cheap"]);
    }

    #[test]
    fn seed_metric_rank_prefers_recoil_gain() {
        let weak = ExactPart {
            ah: 0.1,
            ..ExactPart::neutral()
        };
        let strong = ExactPart {
            d: 10.0,
            ah: -0.2,
            ch: 0.9,
            eh: 5.0,
            ..ExactPart::neutral()
        };
        assert!(seed_metric_rank(&strong, &[], 0.0) > seed_metric_rank(&weak, &[], 0.0));
    }

    #[test]
    fn seed_metric_rank_respects_aim_direction() {
        let lower_ads = ExactPart {
            adsm: -0.2,
            eopen: 10.0,
            ..ExactPart::neutral()
        };
        let higher_ads = ExactPart {
            adsm: 0.2,
            eopen: 0.0,
            ..ExactPart::neutral()
        };
        assert!(seed_metric_rank(&lower_ads, &[], 1.0) > seed_metric_rank(&higher_ads, &[], 1.0));
        assert!(seed_metric_rank(&higher_ads, &[], -1.0) > seed_metric_rank(&lower_ads, &[], -1.0));
    }

    #[test]
    fn seed_metric_rank_respects_panel_min() {
        let higher_panel = ExactPart {
            panel: vec![60.0],
            ..ExactPart::neutral()
        };
        let lower_panel = ExactPart {
            panel: vec![40.0],
            ..ExactPart::neutral()
        };
        assert!(
            seed_metric_rank(&higher_panel, &[0], 0.0) > seed_metric_rank(&lower_panel, &[0], 0.0)
        );
    }

    fn offer() -> Offer {
        Offer {
            e_h: 0.0,
            e_v: 0.0,
            e_k: 0.0,
            e_open: 0.0,
            e_move: 0.0,
            e_move_runtime: 0.0,
            e_breath: 0.0,
            e_move_stability: 0.0,
            values: Vec::new(),
            deviation: 0.0,
        }
    }

    #[test]
    fn dp_abort_stops_a_frontier_that_exceeds_the_speculative_limit() {
        let mut dims = DpDims::default();
        dims.d = true;
        // Every state has a distinct, higher `d` at a higher cost, so none of
        // them is dominated and the frontier keeps growing.
        let states: Vec<DpState> = (0..8).map(|i| part(i as f64, i, vec![])).collect();
        let (out, trim) = dp_pareto_capped(
            states,
            &dims,
            &[],
            AimDir::Ignore,
            usize::MAX,
            None,
            false,
            Some(3),
            None,
        );
        assert!(trim.aborted);
        assert!(!trim.capped);
        assert!(!trim.deadline);
        assert_eq!(out.len(), 3);
    }

    #[test]
    fn dp_abort_waits_for_the_grace_period() {
        let mut dims = DpDims::default();
        dims.d = true;
        let states: Vec<DpState> = (0..8).map(|i| part(i as f64, i, vec![])).collect();
        let not_before = now_ms() + 60_000;
        let (out, trim) = dp_pareto_capped(
            states,
            &dims,
            &[],
            AimDir::Ignore,
            usize::MAX,
            None,
            false,
            Some(3),
            Some(not_before),
        );
        assert!(!trim.aborted);
        assert_eq!(out.len(), 8);
    }

    #[test]
    fn dp_cap_is_not_reported_as_deadline() {
        let mut dims = DpDims::default();
        dims.d = true;
        let states = vec![part(5.0, 5, vec![]), part(20.0, 10, vec![])];
        let (out, trim) = dp_pareto_capped(
            states,
            &dims,
            &[],
            AimDir::Ignore,
            1,
            None,
            false,
            None,
            None,
        );
        assert_eq!(out.len(), 1);
        assert!(trim.capped);
        assert!(!trim.deadline);
    }

    #[test]
    fn dp_deadline_is_not_reported_as_cap() {
        let mut dims = DpDims::default();
        dims.d = true;
        let states = vec![part(5.0, 5, vec![])];
        let (out, trim) = dp_pareto_capped(
            states,
            &dims,
            &[],
            AimDir::Ignore,
            usize::MAX,
            Some(0),
            false,
            None,
            None,
        );
        assert!(out.is_empty());
        assert!(!trim.capped);
        assert!(trim.deadline);
    }

    #[test]
    fn tune_cap_is_not_reported_as_deadline() {
        let offers = vec![offer(), offer()];
        let (out, trim) = pareto_capped(offers, &TuneDims::from_constraints(&[]), 1, None);
        assert_eq!(out.len(), 1);
        assert!(trim.capped);
        assert!(!trim.deadline);
    }

    #[test]
    fn tune_dims_maps_sprint_delay_like_aim_time() {
        let band = [Constraint::new(
            "handling-sprint-fire-delay",
            Some(146.64),
            Some(222.09),
        )];
        assert_eq!(TuneDims::from_constraints(&band).open, TuneDir::Equal);
        let capped = [Constraint::new("handling-sprint-fire-delay", None, Some(222.09))];
        assert_eq!(TuneDims::from_constraints(&capped).open, TuneDir::Higher);
        let floor = [Constraint::new(
            "handling-sprint-fire-delay",
            Some(146.64),
            None,
        )];
        assert_eq!(TuneDims::from_constraints(&floor).open, TuneDir::Lower);
    }

    #[test]
    fn tune_dims_maps_detail_bands_to_equal() {
        for id in [
            "handling-aim-time",
            "handling-aim-move-speed",
            "stability-breath-range",
            "stability-breath-duration",
            "stability-movement-camera",
        ] {
            let dims = TuneDims::from_constraints(&[Constraint::new(id, Some(1.0), Some(2.0))]);
            let mapped = match id {
                "handling-aim-time" => dims.open,
                "handling-aim-move-speed" => dims.move_effect,
                "stability-breath-range" | "stability-breath-duration" => dims.breath,
                _ => dims.move_stability,
            };
            assert_eq!(mapped, TuneDir::Equal, "{id}");
        }
    }

    #[test]
    fn pareto_keeps_distinct_open_sums_for_a_band() {
        let dims = TuneDims::from_constraints(&[Constraint::new(
            "handling-sprint-fire-delay",
            Some(146.64),
            Some(222.09),
        )]);
        let mut low = offer();
        low.e_open = 10.0;
        let mut high = offer();
        high.e_open = 20.0;
        let (out, _) = pareto_capped(vec![low, high], &dims, 4096, None);
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn pareto_keeps_the_highest_open_for_a_max_only_bound() {
        let dims = TuneDims::from_constraints(&[Constraint::new(
            "handling-sprint-fire-delay",
            None,
            Some(222.09),
        )]);
        let mut low = offer();
        low.e_open = 10.0;
        let mut high = offer();
        high.e_open = 20.0;
        let (out, _) = pareto_capped(vec![low, high], &dims, 4096, None);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].e_open, 20.0);
    }

    #[test]
    fn pareto_equal_bucket_still_prunes_dominated_offers() {
        let dims = TuneDims::from_constraints(&[
            Constraint::new("handling-sprint-fire-delay", Some(146.64), Some(222.09)),
            Constraint::new("recoil-single-horizontal", Some(1.0), None),
        ]);
        let mut weak = offer();
        weak.e_open = 10.0;
        weak.e_h = 2.0;
        let mut strong = offer();
        strong.e_open = 10.0;
        strong.e_h = 1.0;
        let (out, _) = pareto_capped(vec![weak, strong], &dims, 4096, None);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].e_h, 1.0);
    }

    fn visible_slot(
        path: &str,
        parent: Option<&str>,
        depth: usize,
        selected: Option<&str>,
        internal: Option<&str>,
    ) -> crate::assembly::VisibleSlot {
        crate::assembly::VisibleSlot {
            path_key: path.to_string(),
            parent_slot_path_key: parent.map(str::to_string),
            host_part_id: "host".to_string(),
            depth,
            slot: crate::assembly::SlotView {
                id: "slot".to_string(),
                name: "slot".to_string(),
                virtual_candidate_ids: Vec::new(),
            },
            candidate_ids: Vec::new(),
            selected_part_id: selected.map(str::to_string),
            internal_part_id: internal.map(str::to_string),
            internal_is_default: false,
        }
    }

    fn internal_part(
        path: &str,
        parent: Option<&str>,
        depth: usize,
        part: &str,
    ) -> crate::assembly::InternalPart {
        crate::assembly::InternalPart {
            slot_path_key: path.to_string(),
            parent_slot_path_key: parent.map(str::to_string),
            host_part_id: "host".to_string(),
            depth,
            slot: crate::assembly::SlotView {
                id: "slot".to_string(),
                name: "slot".to_string(),
                virtual_candidate_ids: Vec::new(),
            },
            part_id: part.to_string(),
            source: "default".to_string(),
        }
    }

    fn decided_set(paths: &[&str]) -> FxHashSet<String> {
        paths.iter().map(|path| (*path).to_string()).collect()
    }

    #[test]
    fn bound_roots_count_an_undecided_subtree_once() {
        let visible = vec![
            visible_slot("w/slot:1:A", None, 0, None, None),
            visible_slot(
                "w/slot:1:A/part:P/slot:2:B",
                Some("w/slot:1:A"),
                1,
                Some("Q"),
                None,
            ),
        ];
        let internal = vec![internal_part(
            "w/slot:1:A/part:P/slot:3:H",
            Some("w/slot:1:A"),
            1,
            "R",
        )];
        let decided = decided_set(&["w/slot:1:A/part:P/slot:2:B"]);
        let roots = bound_roots(&visible, &internal, &decided, |slot| {
            slot.path_key == "w/slot:1:A"
        });
        assert_eq!(roots, vec![BoundRoot::AssumedVisible(0)]);
    }

    #[test]
    fn bound_roots_keep_own_bounds_under_decided_parents() {
        let visible = vec![
            visible_slot("w/slot:1:A", None, 0, Some("P"), None),
            visible_slot(
                "w/slot:1:A/part:P/slot:2:B",
                Some("w/slot:1:A"),
                1,
                Some("Q"),
                None,
            ),
        ];
        let internal = vec![
            internal_part("w/slot:1:A/part:P/slot:3:H", Some("w/slot:1:A"), 1, "R"),
            internal_part(
                "w/slot:1:A/part:P/slot:3:H/part:R/slot:4:K",
                Some("w/slot:1:A/part:P/slot:3:H"),
                2,
                "S",
            ),
        ];
        let decided = decided_set(&["w/slot:1:A", "w/slot:1:A/part:P/slot:2:B"]);
        let roots = bound_roots(&visible, &internal, &decided, |_| true);
        assert_eq!(
            roots,
            vec![
                BoundRoot::DecidedVisible(0),
                BoundRoot::DecidedVisible(1),
                BoundRoot::OptionalInternal(0),
                BoundRoot::OptionalInternal(1),
            ]
        );
    }

    #[test]
    fn bound_roots_dedup_the_internal_record_of_a_visible_slot() {
        let visible = vec![visible_slot("w/slot:1:A", None, 0, None, Some("P"))];
        let internal = vec![internal_part("w/slot:1:A", None, 0, "P")];
        let decided = decided_set(&["w/slot:1:A"]);
        let roots = bound_roots(&visible, &internal, &decided, |_| true);
        assert_eq!(roots, vec![BoundRoot::DecidedVisible(0)]);
    }

    #[test]
    fn bound_roots_do_not_cover_when_no_recursive_bound_exists() {
        let visible = vec![
            visible_slot("w/slot:1:A", None, 0, None, None),
            visible_slot(
                "w/slot:1:A/part:P/slot:2:B",
                Some("w/slot:1:A"),
                1,
                Some("Q"),
                None,
            ),
        ];
        let decided = decided_set(&["w/slot:1:A/part:P/slot:2:B"]);
        let roots = bound_roots(&visible, &[], &decided, |_| false);
        assert_eq!(roots, vec![BoundRoot::DecidedVisible(1)]);
    }

    #[test]
    fn reach_targets_rejects_jointly_unattainable_targets() {
        let increments = vec![
            vec![(10, 0, 0, 0), (0, 0, 0, 10)],
            vec![(0, 0, 0, 10), (10, 0, 0, 0)],
        ];
        assert_eq!(
            reach_targets(&increments, [Some(20), None, None, Some(20)], 1000),
            ReachResult::Infeasible
        );
        assert_eq!(
            reach_targets(&increments, [Some(20), None, None, None], 1000),
            ReachResult::Found(vec![0, 1])
        );
        assert_eq!(
            reach_targets(&increments, [None, None, None, Some(20)], 1000),
            ReachResult::Found(vec![1, 0])
        );
    }

    #[test]
    fn reach_targets_is_infeasible_when_a_channel_max_is_short() {
        let increments = vec![vec![(5, 0, 0, 0)]];
        assert_eq!(
            reach_targets(&increments, [Some(6), None, None, None], 1000),
            ReachResult::Infeasible
        );
        assert_eq!(
            reach_targets(&increments, [Some(5), None, None, None], 1000),
            ReachResult::Found(vec![0])
        );
    }

    #[test]
    fn reach_targets_reports_budget_exhaustion_separately() {
        let increments = vec![vec![(1, 0, 0, 0)], vec![(1, 0, 0, 0)]];
        assert_eq!(
            reach_targets(&increments, [Some(2), None, None, None], 1),
            ReachResult::Budget
        );
    }

    #[test]
    fn recoil_maxima_keep_the_tightest_bound_per_axis() {
        let constraints = vec![
            Constraint::new("recoil-continuous-horizontal", None, Some(0.8)),
            Constraint::new("recoil-single-horizontal", None, Some(0.6)),
            Constraint::new("recoil-continuous-vertical", None, Some(0.4)),
            Constraint::new("recoil-fire-stability", None, Some(0.9)),
            Constraint::new("handling-aim-time", None, Some(400.0)),
            Constraint::new("recoil-continuous-vertical", Some(0.1), None),
        ];
        let (h, v, k) = recoil_maxima(&constraints);
        assert_eq!(h, Some(0.6));
        assert_eq!(v, Some(0.4));
        assert_eq!(k, Some(0.9));
        assert_eq!(aim_time_max(&constraints), Some(400.0));
    }

    #[test]
    fn optimistic_recoil_margin_reports_slack() {
        let control = curve(&[(0.0, 1.0), (100.0, 0.5)]);
        let gunkick = curve(&[(0.0, 1.0), (100.0, 0.5)]);
        let untuned = state_metric(0.0, 0.2, 1.0, 0.0);
        let tuned = state_metric(0.0, 0.2, 1.0, 80.0);
        let m_untuned = optimistic_recoil_margin(
            &untuned,
            Some(0.5),
            None,
            None,
            Some(&control),
            Some(&gunkick),
            1.0,
            &AimScaleFloor::identity(),
        );
        let m_tuned = optimistic_recoil_margin(
            &tuned,
            Some(0.5),
            None,
            None,
            Some(&control),
            Some(&gunkick),
            1.0,
            &AimScaleFloor::identity(),
        );
        assert!(m_tuned > m_untuned);
        assert!(m_tuned > 0.0);
        assert!(m_untuned < 0.0);
        assert!(optimistic_recoil_margin(
            &tuned,
            None,
            None,
            None,
            Some(&control),
            Some(&gunkick),
            1.0,
            &AimScaleFloor::identity(),
        )
        .is_infinite());
    }

    #[test]
    fn incumbent_eval_order_prefers_optimistically_feasible_then_cheapest() {
        let beam = vec![
            part(0.0, 10, vec![]),
            part(0.0, 5, vec![]),
            part(0.0, 20, vec![]),
        ];
        assert_eq!(
            incumbent_eval_order(&beam, &[0.1, -0.2, 0.3]),
            vec![0, 2, 1]
        );
        assert_eq!(
            incumbent_eval_order(&beam, &[-0.1, -0.2, -0.3]),
            vec![1, 0, 2]
        );
    }
}

fn now_ms() -> u64 {
    #[cfg(target_arch = "wasm32")]
    {
        #[link(wasm_import_module = "env")]
        extern "C" {
            fn dws_now_ms() -> f64;
        }
        unsafe { dws_now_ms() as u64 }
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        use std::sync::OnceLock;
        use std::time::Instant;
        static START: OnceLock<Instant> = OnceLock::new();
        START.get_or_init(Instant::now).elapsed().as_millis() as u64
    }
}
