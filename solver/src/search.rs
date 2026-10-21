use crate::assembly::{Assembler, VisibleSlot};
use crate::metrics::{
    eval_curve_points, quantize, tune_channels, tuning_available, tune_offer, tune_offer_values,
    DetailMetrics, DetailSums, Evaluator, HANDLING_ATTRS, HIPFIRE_ATTRS, STABILITY_ATTRS,
    TuningInstance, TuningState, DETAIL_METRICS,
};
use crate::model::{Curve, CurveKey, Part, WeaponPayload};
use rustc_hash::{FxHashMap, FxHashSet};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;

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
    relevance: Relevance,
    offer_cache: FxHashMap<(String, String), Vec<Offer>>,
    slot_bounds: FxHashMap<(String, String), MetricBound>,
    structural_parts: FxHashSet<String>,
    dp_ok: bool,
    dp_dims: DpDims,
    tune_dims: TuneDims,
    aim_dp: AimDir,
    panel_attrs: Vec<String>,
    panel_dirs: Vec<PanelDir>,
    seed_panel_attrs: Vec<usize>,
    dp_metric_cache: FxHashMap<(String, String), ExactPart>,
    control_curve: Option<Curve>,
    gunkick_curve: Option<Curve>,
    control_min: Option<f64>,
    gunkick_min: Option<f64>,
    gunkick0: f64,
    weapon_price: i64,
    seed: bool,
    cost_cap: i64,
    run_found: bool,
    deadline_at: Option<u64>,
    best: Option<Solution>,
    alternatives: Vec<Solution>,
    nodes: u64,
    leaves: u64,
    truncated: bool,
    stop: bool,
    best_cost: i64,
    panel_base: [f64; 5],
}

#[derive(Debug, Clone, Default)]
struct Relevance {
    slots: FxHashSet<(String, String)>,
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
    values: Vec<(String, f64)>,
    deviation: f64,
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
                "handling-aim-time" => {
                    open_high |= max && !min;
                    open_low |= min && !max;
                }
                "handling-aim-move-speed" => {
                    move_low |= max && !min;
                    move_high |= min && !max;
                }
                "stability-breath-range" => {
                    breath_high |= max && !min;
                    breath_low |= min && !max;
                }
                "stability-breath-duration" => {
                    breath_high |= min && !max;
                    breath_low |= max && !min;
                }
                "stability-movement-camera" => {
                    stability_high |= max && !min;
                    stability_low |= min && !max;
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
        let assembler = Assembler::new(payload, weapon_id);
        let evaluator = Evaluator {
            payload,
            weapon_id,
            mode: &options.mode,
            laser_enabled: options.laser_enabled,
            bipod_deployed: options.bipod_deployed,
            prices,
        };
        let relevance = compute_relevance(payload, weapon_id, options);
        let slot_bounds = compute_bounds(payload, weapon_id, &options.mode);
        let (control_curve, gunkick_curve, gunkick0) = {
            let profile = payload.numeric_profiles.get(weapon_id);
            let curves = profile.map(|p| &p.linked_tables.recoil_curves);
            let control = curves.and_then(|c| {
                if options.mode == "warfare" {
                    c.recoil_control_mp.as_ref().or(c.recoil_control.as_ref())
                } else {
                    c.recoil_control.as_ref()
                }
            }).cloned();
            let gunkick = curves.and_then(|c| {
                if options.mode == "warfare" {
                    c.gunkick_mp.as_ref().or(c.gunkick.as_ref())
                } else {
                    c.gunkick.as_ref()
                }
            }).cloned();
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
                || crate::metrics::has_laser(part, &options.mode)
                || !part.aim_recoil_profiles.is_empty();
            if structural {
                structural_parts.insert(id.clone());
            }
        }
        let mut panel_attrs: Vec<String> = options
            .constraints
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
                for c in &options.constraints {
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
                options
                    .constraints
                    .iter()
                    .any(|c| &c.id == *attr && c.min.is_some())
            })
            .map(|(idx, _)| idx)
            .collect();
        let mut dp_ok = options.constraints.iter().all(|c| {
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
        let dp_dims = DpDims::from_constraints(&options.constraints);
        if dp_dims.d && !curves_nonincreasing {
            dp_ok = false;
        }
        let mut panel_base = [0.0f64; 5];
        if let Some(stats) = payload
            .weapon_stats
            .get(weapon_id)
            .and_then(|s| s.by_mode.get(&options.mode))
        {
            for (idx, attr) in PANEL_ATTRS.iter().enumerate() {
                panel_base[idx] = stats
                    .display
                    .get(*attr)
                    .copied()
                    .or_else(|| stats.base.get(*attr).copied())
                    .unwrap_or(0.0);
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
            slot_bounds,
            structural_parts,
            dp_ok,
            dp_dims,
            tune_dims: TuneDims::from_constraints(&options.constraints),
            aim_dp: aim_direction(&options.constraints),
            panel_attrs,
            panel_dirs,
            seed_panel_attrs,
            dp_metric_cache: FxHashMap::default(),
            control_curve,
            gunkick_curve,
            control_min,
            gunkick_min,
            gunkick0,
            weapon_price,
            seed: false,
            cost_cap: i64::MAX,
            run_found: false,
            deadline_at: None,
            best: None,
            alternatives: Vec::new(),
            nodes: 0,
            leaves: 0,
            truncated: false,
            stop: false,
            best_cost: i64::MAX,
            panel_base,
        }
    }

    pub fn solve(mut self) -> SolveResult {
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
        let mut total_nodes = 0u64;
        let mut total_leaves = 0u64;
        if self.options.first_solution {
            self.seed = true;
            self.dfs(&mut selections, &mut decided, &mut deferred);
            self.seed = false;
            let found = self.best.is_some();
            let message = if found {
                "first feasible build found"
            } else if self.truncated {
                "budget exhausted; no build found"
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
            if (self.best.is_some() || (fixed_cap.is_some() && seed_found)) && !self.truncated {
                reset(&mut self, &mut selections, &mut decided);
                self.dfs(&mut selections, &mut decided, &mut deferred);
                total_nodes += self.nodes;
                total_leaves += self.leaves;
                truncated |= self.truncated;
            }
        } else {
            self.cost_cap = fixed_cap.unwrap_or(i64::MAX);
            self.dfs(&mut selections, &mut decided, &mut deferred);
            total_nodes += self.nodes;
            total_leaves += self.leaves;
            truncated = self.truncated;
        }
        let optimal = !truncated && self.best.is_some();
        let message = if truncated {
            "budget exhausted; result is the best found so far".to_string()
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
        self.nodes += 1;

        let assembly = self.assembler.resolve(selections);

        let mut next: Option<usize> = None;
        for (idx, slot) in assembly.visible_slots.iter().enumerate() {
            if decided.contains(&slot.path_key) || deferred.contains(&slot.path_key) {
                continue;
            }
            if !self
                .relevance
                .slots
                .contains(&(slot.host_part_id.clone(), slot.slot.id.clone()))
            {
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
        &self,
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
        if self.dp_ok {
            parts.retain(|id| self.structural_parts.contains(id));
        }
        let unset = slot.internal_part_id.clone();
        let mut forced: Vec<Action> = Vec::new();
        let mut can_defer = false;
        if self.dp_ok {
            let unset_structural = unset
                .as_ref()
                .map(|id| self.structural_parts.contains(id))
                .unwrap_or(false);
            let remove = if slot.internal_is_default {
                self.effective_none_part(selections, path)
            } else {
                unset.clone()
            };
            let remove_structural = remove
                .as_ref()
                .map(|id| self.structural_parts.contains(id))
                .unwrap_or(false);
            if unset_structural {
                forced.push(Action::Unset);
            }
            if (remove_structural || unset_structural)
                && remove.as_deref() != unset.as_deref()
            {
                forced.push(Action::Remove);
            }
            can_defer = !unset_structural;
        } else if unset.is_some() {
            forced.push(Action::Unset);
            if slot.internal_is_default {
                forced.push(Action::Remove);
            }
        } else {
            forced.push(Action::Remove);
        }
        parts.sort_by(|a, b| {
            if self.seed && !self.seed_panel_attrs.is_empty() {
                let sa = self.seed_panel_score(a);
                let sb = self.seed_panel_score(b);
                sb.partial_cmp(&sa)
                    .unwrap_or(Ordering::Equal)
                    .then_with(|| self.part_price(a).cmp(&self.part_price(b)))
                    .then_with(|| a.cmp(b))
            } else {
                let pa = self.part_price(a);
                let pb = self.part_price(b);
                pa.cmp(&pb).then(a.cmp(b))
            }
        });
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

    fn cannot_satisfy(
        &self,
        _selections: &FxHashMap<String, Option<String>>,
        assembly: &crate::assembly::Assembly,
        decided: &FxHashSet<String>,
    ) -> bool {
        let has_panel = self
            .options
            .constraints
            .iter()
            .any(|c| is_panel_attr(&c.id) && c.id != "10001");
        let has_recoil_max = self.options.constraints.iter().any(|c| {
            c.max.is_some() && is_recoil_metric(&c.id)
        });
        if !has_panel && !has_recoil_max {
            return false;
        }
        let mut decided_bound = MetricBound::neutral();
        let mut assumed_bound = MetricBound::neutral();
        let mut optional_paths: FxHashSet<&str> = FxHashSet::default();
        let mut seen: FxHashSet<(&str, &str)> = FxHashSet::default();
        for slot in &assembly.visible_slots {
            if !decided.contains(&slot.path_key) {
                continue;
            }
            let Some(id) = slot
                .selected_part_id
                .as_ref()
                .or(slot.internal_part_id.as_ref())
            else {
                continue;
            };
            if !seen.insert((slot.path_key.as_str(), id.as_str())) {
                continue;
            }
            if let Some(part) = self.payload.parts.get(id) {
                decided_bound.add_a(&part_own_bound(part, &self.options.mode));
            }
        }
        for item in &assembly.internal_parts {
            if !seen.insert((item.slot_path_key.as_str(), item.part_id.as_str())) {
                continue;
            }
            let Some(part) = self.payload.parts.get(&item.part_id) else {
                continue;
            };
            if decided.contains(&item.slot_path_key) {
                decided_bound.add_a(&part_own_bound(part, &self.options.mode));
                continue;
            }
            let parent_active = match &item.parent_slot_path_key {
                Some(parent) => {
                    decided.contains(parent) || optional_paths.contains(parent.as_str())
                }
                None => true,
            };
            if parent_active {
                assumed_bound.add_optional(&part_own_bound(part, &self.options.mode));
                optional_paths.insert(item.slot_path_key.as_str());
            }
        }
        let mut bound = decided_bound;
        bound.add_a(&assumed_bound);
        for slot in &assembly.visible_slots {
            if decided.contains(&slot.path_key) {
                continue;
            }
            if !self
                .relevance
                .slots
                .contains(&(slot.host_part_id.clone(), slot.slot.id.clone()))
            {
                continue;
            }
            if let Some(b) = self
                .slot_bounds
                .get(&(slot.host_part_id.clone(), slot.slot.id.clone()))
            {
                bound.add_a(b);
            }
        }
        for c in &self.options.constraints {
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
        let f_h = cr * (1.0 + bound.min_a_h + bound.min_t_h) * bound.min_c_h;
        let f_v = cr * (1.0 + bound.min_a_v + bound.min_t_v) * bound.min_c_v;
        let s = gk / self.gunkick0 * (1.0 + bound.min_t_k) * bound.min_c_k;
        for c in &self.options.constraints {
            let Some(max) = c.max else { continue };
            let value = match c.id.as_str() {
                "recoil-continuous-horizontal" => f_h,
                "recoil-continuous-vertical" => f_v,
                "recoil-fire-stability" => s,
                _ => continue,
            };
            if value > max + 1e-9 {
                return true;
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
            let Some(id) = slot
                .selected_part_id
                .as_ref()
                .or(slot.internal_part_id.as_ref())
            else {
                continue;
            };
            if !seen.insert((slot.path_key.as_str(), id.as_str())) {
                continue;
            }
            if let Some(part) = self.payload.parts.get(id) {
                cost += self.dp_part_price(part);
            }
        }
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
        cost
    }

    fn part_price(&self, id: &str) -> i64 {
        self.payload
            .parts
            .get(id)
            .map(|p| self.evaluator.prices.get(p.price_id()).copied().unwrap_or(0))
            .unwrap_or(0)
    }

    fn seed_panel_score(&self, id: &str) -> f64 {
        let Some(part) = self.payload.parts.get(id) else {
            return 0.0;
        };
        let Some(list) = part.display_mods.get(&self.options.mode) else {
            return 0.0;
        };
        let mut score = 0.0;
        for m in list {
            if m.condition_id.is_some() || m.modifier_type != "Addend" {
                continue;
            }
            let Some(idx) = panel_attr_index(normalize_attr(m.attribute_id.as_deref())) else {
                continue;
            };
            if !self.seed_panel_attrs.contains(&idx) {
                continue;
            }
            if let Some(delta) = m.delta.map(|v| v.0).filter(|v| v.is_finite()) {
                score += delta;
            }
        }
        score
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
            if !self
                .relevance
                .slots
                .contains(&(slot.host_part_id.clone(), slot.slot.id.clone()))
            {
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
        let leaf_paths: FxHashSet<String> =
            leaf_slots.iter().map(|s| s.path_key.clone()).collect();
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
        let mut base_parts: Vec<(&Part, &str)> = Vec::new();
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
                base_parts.push((part, &slot.slot.name));
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
                base_parts.push((part, &item.slot.name));
                base_cost += self.dp_part_price(part);
            }
        }
        let only_parts: Vec<&Part> = base_parts.iter().map(|(p, _)| *p).collect();
        let laser_index = self.laser_index(&only_parts);
        for (idx, (part, slot_name)) in base_parts.iter().enumerate() {
            let is_laser = crate::metrics::has_laser(part, &self.options.mode);
            let include_pro = is_laser && self.options.laser_enabled && Some(idx) == laser_index;
            let m = exact_part_metric(
                part,
                slot_name,
                &self.options.mode,
                self.options.bipod_deployed,
                is_laser,
                include_pro,
                &self.panel_attrs,
            );
            base.add(&m);
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
                opts.push(self.dp_option_for_part(part, &slot.slot.name, SlotChoice::Unset, &opts));
            } else {
                opts.push(DpOption {
                    cost: 0,
                    metric: ExactPart::neutral_panel(self.panel_attrs.len()),
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
                if let Some(part) = remove_part.as_ref().and_then(|id| self.payload.parts.get(id)) {
                    opts.push(self.dp_option_for_part(part, &slot.slot.name, SlotChoice::Remove, &opts));
                } else {
                    opts.push(DpOption {
                        cost: 0,
                        metric: ExactPart::neutral_panel(self.panel_attrs.len()),
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
                        opts.push(self.dp_option_for_part(part, &slot.slot.name, choice, &opts));
                    }
                }
            }
            opts.sort_by(|a, b| {
                a.cost
                    .cmp(&b.cost)
                    .then(b.metric.score().partial_cmp(&a.metric.score()).unwrap_or(Ordering::Equal))
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

        let mut states: Vec<DpState> = vec![DpState {
            cost: base_cost,
            metric: base,
            choices: Vec::new(),
        }];
        for (slot_idx, opts) in options_per_slot.iter().enumerate() {
            let rest_cost = suffix_min[slot_idx + 1];
            let mut next: Vec<DpState> = Vec::with_capacity(states.len().saturating_mul(opts.len()).min(200_000));
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
                    if total + rest_cost > self.cost_cap
                        && self.cost_cap < i64::MAX
                    {
                        continue;
                    }
                    if self.best_cost < i64::MAX && total + rest_cost >= self.best_cost {
                        continue;
                    }
                    let mut metric = st.metric.clone();
                    metric.add(&opt.metric);
                    let mut choices = st.choices.clone();
                    choices.push(opt.choice.clone());
                    next.push(DpState { cost, metric, choices });
                }
            }
            let (pruned, cut) = dp_pareto_capped(
                next,
                &self.dp_dims,
                &self.panel_dirs,
                self.aim_dp,
                200_000,
                self.deadline_at,
            );
            self.truncated |= cut;
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

        states.sort_by_key(|s| s.cost);
        let aim_max = self
            .options
            .constraints
            .iter()
            .find(|c| c.id == "handling-aim-time" && c.min.is_none())
            .and_then(|c| c.max);
        for st in states {
            if self.budget_exceeded() {
                return;
            }
            if let Some(max) = aim_max {
                if let Some(best) = self.evaluator.aim_time(
                    st.metric.hs,
                    st.metric.adsm,
                    st.metric.eopen.max(0.0),
                ) {
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

    fn dp_part_price(&self, part: &Part) -> i64 {
        if part.is_virtual {
            0
        } else {
            self.evaluator.prices.get(part.price_id()).copied().unwrap_or(0)
        }
    }

    fn dp_option_for_part(
        &mut self,
        part: &Part,
        slot_name: &str,
        choice: SlotChoice,
        existing: &[DpOption],
    ) -> DpOption {
        let is_laser = crate::metrics::has_laser(part, &self.options.mode);
        let include_pro = is_laser && self.options.laser_enabled;
        let key = (part.id.clone(), slot_name.to_string());
        let metric = if let Some(m) = self.dp_metric_cache.get(&key) {
            m.clone()
        } else {
            let m = exact_part_metric(
                part,
                slot_name,
                &self.options.mode,
                self.options.bipod_deployed,
                is_laser,
                include_pro,
                &self.panel_attrs,
            );
            self.dp_metric_cache.insert(key, m.clone());
            m
        };
        let cost = self.dp_part_price(part);
        let _ = existing;
        DpOption { cost, metric, choice }
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

    fn laser_index(&self, parts: &[&Part]) -> Option<usize> {
        let mut best: Option<(usize, f64)> = None;
        for (idx, part) in parts.iter().enumerate() {
            if !crate::metrics::has_laser(part, &self.options.mode) {
                continue;
            }
            let score: f64 = part
                .display_mods
                .get(&self.options.mode)
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
            if best.map(|(_, s)| score > s).unwrap_or(true) {
                best = Some((idx, score));
            }
        }
        best.map(|(idx, _)| idx)
    }

    fn leaf(&mut self, selections: &FxHashMap<String, Option<String>>) {
        if self.stop {
            return;
        }
        self.leaves += 1;
        if self.budget_exceeded() {
            return;
        }
        let assembly = self.assembler.resolve(selections);
        let installed: Vec<Part> = self
            .evaluator
            .installed_parts(&assembly)
            .into_iter()
            .cloned()
            .collect();
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
        let installed_refs: Vec<&Part> = installed.iter().collect();
        let Some((tuning_state, chosen)) =
            self.pick_tuning(&instances, &req, &installed_refs)
        else {
            return;
        };

        let Some(metrics) = self.evaluator.evaluate(&self.assembler, &assembly, &tuning_state) else {
            return;
        };

        let mut constraint_results = Vec::new();
        let mut ok = true;
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
        if !ok {
            return;
        }

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
                t_h: metrics.recoil.tuning_runtime.get("GRecoil_H").copied().unwrap_or(0.0),
                t_v: metrics.recoil.tuning_runtime.get("GRecoil_V").copied().unwrap_or(0.0),
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
            let prev = self.best.replace(solution.clone());
            if let Some(prev) = prev {
                self.push_alternative(prev);
            }
        } else {
            self.best = Some(solution.clone());
        }
        self.push_alternative(solution);
        self.run_found = true;
        if self.seed {
            self.stop = true;
        }
    }

    fn push_alternative(&mut self, solution: Solution) {
        if self.options.max_results == 0 {
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

    fn recoil_base(&self, parts: &[Part]) -> Option<BaseRecoil> {
        let profile = self.payload.numeric_profiles.get(self.evaluator.weapon_id)?;
        let mode = &self.options.mode;
        let mut d = 0.0;
        let mut a_h = 0.0;
        let mut a_v = 0.0;
        let mut c_h = 1.0;
        let mut c_v = 1.0;
        let mut c_k = 1.0;
        let mut laser_parts: Vec<(usize, f64)> = Vec::new();
        for (idx, part) in parts.iter().enumerate() {
            if crate::metrics::has_laser(part, mode) {
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
            let laser_part = crate::metrics::has_laser(part, mode);
            let selected_laser = laser_idx == Some(idx);
            for m in part.main_mods.get(mode).map(|v| v.as_slice()).unwrap_or(&[]) {
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
                let Some(p) = m.param1.map(|v| v.0) else { continue };
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
                if let Some(k) = part.raw_recoil.get(mode).and_then(|r| r.gunkick_spring_mult_c) {
                    if k.is_finite() {
                        c_k *= 1.0 + k;
                    }
                }
            }
            let _ = (explicit_c_h, explicit_c_v);
        }

        let curves = &profile.linked_tables.recoil_curves;
        let control = if mode == "warfare" {
            curves.recoil_control_mp.as_ref().or(curves.recoil_control.as_ref())
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
        Some(BaseRecoil {
            a_h,
            a_v,
            c_h,
            c_v,
            c_k,
            cr,
            gk_ratio: gk / gk0,
        })
    }

    fn tune_requirements(&self, b: &BaseRecoil) -> Option<TuneReq> {
        let mut req = TuneReq {
            lower_h: f64::NEG_INFINITY,
            upper_h: f64::INFINITY,
            lower_v: f64::NEG_INFINITY,
            upper_v: f64::INFINITY,
            lower_k: f64::NEG_INFINITY,
            upper_k: f64::INFINITY,
            impossible: false,
        };
        for c in &self.options.constraints {
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
            let (k, base) = match c.id.as_str() {
                "recoil-continuous-horizontal" | "recoil-single-horizontal" => {
                    (b.cr * b.c_h, 1.0 + b.a_h)
                }
                "recoil-continuous-vertical" | "recoil-single-vertical" => {
                    (b.cr * b.c_v, 1.0 + b.a_v)
                }
                _ => (b.gk_ratio * b.c_k, 1.0),
            };
            if let Some(thr) = c.max {
                apply_linear_req(lower, upper, &mut req.impossible, k, base, thr, true);
            }
            if let Some(thr) = c.min {
                apply_linear_req(lower, upper, &mut req.impossible, k, base, thr, false);
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
    ) -> Option<(TuningState, Vec<(String, f64)>)> {
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
                let default = tune.default_value;
                let mut next = Vec::new();
                for a in &group {
                    for b in &offers {
                        if next.len() >= TUNE_NEXT_LIMIT {
                            break;
                        }
                        let mut values = a.values.clone();
                        values.push((key_base.clone(), b.value));
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
                    if next.len() >= TUNE_NEXT_LIMIT {
                        self.truncated = true;
                        break;
                    }
                }
                let (pruned, cut) =
                    pareto_capped(next, &self.tune_dims, 4096, self.deadline_at);
                self.truncated |= cut;
                group = pruned;
            }
            if group.len() == 1 && group[0].values.is_empty() {
                continue;
            }
            let mut next = Vec::new();
            for a in &frontier {
                for b in &group {
                    if next.len() >= TUNE_NEXT_LIMIT {
                        break;
                    }
                    let mut values = a.values.clone();
                    values.extend(b.values.iter().cloned());
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
                if next.len() >= TUNE_NEXT_LIMIT {
                    self.truncated = true;
                    break;
                }
            }
            let (pruned, cut) =
                pareto_capped(next, &self.tune_dims, 4096, self.deadline_at);
            self.truncated |= cut;
            frontier = pruned;
        }

        let detail_constraints: Vec<&Constraint> = self
            .options
            .constraints
            .iter()
            .filter(|c| !is_panel_attr(&c.id) && !is_recoil_metric(&c.id))
            .collect();
        let mut best: Option<&Offer> = None;
        let mut best_score = f64::NEG_INFINITY;
        for o in &frontier {
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
                let detail = self.evaluator.detail_with(parts, &sums);
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
        for (key, value) in &best.values {
            state.insert(key.clone(), *value);
        }
        Some((state, best.values.clone()))
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
                    value: o.values.first().map(|v| v.1).unwrap_or(tune.default_value),
                })
                .collect();
        }
        let values = tune_offer_values(tune);
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
                .then(deviation(a.value).partial_cmp(&deviation(b.value)).unwrap_or(Ordering::Equal))
        });
        let dims = self.tune_dims;
        let mut pruned: Vec<LocalOffer> = Vec::new();
        for o in offers {
            if pruned.iter().any(|p| local_offer_dominates(p, &o, &dims)) {
                continue;
            }
            pruned.retain(|p| !local_offer_dominates(&o, p, &dims));
            pruned.push(o);
        }
        pruned.sort_by(|a, b| {
            b.e_h
                .partial_cmp(&a.e_h)
                .unwrap_or(Ordering::Equal)
                .then(b.e_v.partial_cmp(&a.e_v).unwrap_or(Ordering::Equal))
                .then(b.e_k.partial_cmp(&a.e_k).unwrap_or(Ordering::Equal))
                .then(deviation(a.value).partial_cmp(&deviation(b.value)).unwrap_or(Ordering::Equal))
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
                values: vec![(String::new(), o.value)],
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
                        price: self.evaluator.prices.get(part.price_id()).copied().unwrap_or(0),
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
                    price: self.evaluator.prices.get(part.price_id()).copied().unwrap_or(0),
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
        if self.panel.len() < other.panel.len() {
            self.panel.resize(other.panel.len(), 0.0);
        }
        for (i, v) in other.panel.iter().enumerate() {
            self.panel[i] += *v;
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

#[derive(Debug, Clone)]
struct DpOption {
    cost: i64,
    metric: ExactPart,
    choice: SlotChoice,
}

#[derive(Debug, Clone)]
struct DpState {
    cost: i64,
    metric: ExactPart,
    choices: Vec<SlotChoice>,
}

fn exact_part_metric(
    part: &Part,
    slot_name: &str,
    mode: &str,
    bipod_deployed: bool,
    is_laser: bool,
    include_laser_pro: bool,
    panel_attrs: &[String],
) -> ExactPart {
    let mut m = ExactPart::neutral_panel(panel_attrs.len());
    if !panel_attrs.is_empty() {
        if let Some(list) = part.display_mods.get(mode) {
            for modif in list {
                if modif.condition_id.is_some() || modif.modifier_type != "Addend" {
                    continue;
                }
                if is_laser
                    && modif.polarity.as_deref() != Some("con")
                    && !include_laser_pro
                {
                    continue;
                }
                let attr = normalize_attr(modif.attribute_id.as_deref());
                if let Some(pos) = panel_attrs.iter().position(|a| *a == attr) {
                    m.panel[pos] += modif.delta.map(|v| v.0).unwrap_or(0.0);
                }
            }
        }
    }
    if let Some(list) = part.main_mods.get(mode) {
        for modif in list {
            if modif.condition_id.is_some() || modif.modifier_type != "Addend" {
                continue;
            }
            if is_laser
                && modif.polarity.as_deref() != Some("con")
                && !include_laser_pro
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
            let Some(p) = modif.param1.map(|v| v.0) else { continue };
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
        if let Some(k) = part.raw_recoil.get(mode).and_then(|r| r.gunkick_spring_mult_c) {
            if k.is_finite() {
                m.ck *= 1.0 + k;
            }
        }
    }
    if !crate::metrics::tuning_available(slot_name, part) {
        return m;
    }
    for tune in &part.tuning_options {
        let mut max_h = f64::NEG_INFINITY;
        let mut max_v = f64::NEG_INFINITY;
        let mut max_k = f64::NEG_INFINITY;
        let mut max_open = f64::NEG_INFINITY;
        for v in tune_offer_values(tune) {
            let (h, vv, k) = tune_offer(tune, v);
            max_h = max_h.max(h);
            max_v = max_v.max(vv);
            max_k = max_k.max(k);
            max_open = max_open.max(tune_channels(tune, v).open);
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
    }
    m
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
    if dims.eh && x.eh < y.eh - 1e-9 {
        return false;
    }
    if dims.ev && x.ev < y.ev - 1e-9 {
        return false;
    }
    if dims.ek && x.ek < y.ek - 1e-9 {
        return false;
    }
    if dims.hs {
        match aim_dir {
            AimDir::MoreHandling => {
                if x.hs < y.hs - 1e-9
                    || x.adsm > y.adsm + 1e-9
                    || x.eopen < y.eopen - 1e-9
                {
                    return false;
                }
            }
            AimDir::LessHandling => {
                if x.hs > y.hs + 1e-9
                    || x.adsm < y.adsm - 1e-9
                    || x.eopen > y.eopen + 1e-9
                {
                    return false;
                }
            }
            AimDir::Equal => {
                if (x.hs - y.hs).abs() > 1e-9
                    || (x.adsm - y.adsm).abs() > 1e-9
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
    dp_pareto_capped(states, dims, panel_dirs, aim_dir, usize::MAX, None).0
}

fn dp_pareto_capped(
    mut states: Vec<DpState>,
    dims: &DpDims,
    panel_dirs: &[PanelDir],
    aim_dir: AimDir,
    limit: usize,
    deadline_at: Option<u64>,
) -> (Vec<DpState>, bool) {
    states.sort_by(|a, b| {
        a.cost
            .cmp(&b.cost)
            .then(b.metric.score().partial_cmp(&a.metric.score()).unwrap_or(Ordering::Equal))
    });
    let mut truncated = false;
    let mut out: Vec<DpState> = Vec::new();
    for (idx, st) in states.into_iter().enumerate() {
        if out.len() >= limit {
            truncated = true;
            break;
        }
        if idx & 1023 == 0 && deadline_at.is_some_and(|d| now_ms() >= d) {
            truncated = true;
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
    (out, truncated)
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

    fn add_optional(&mut self, other: &MetricBound) {
        self.max_d += other.max_d.max(0.0);
        self.min_a_h += other.min_a_h.min(0.0);
        self.min_a_v += other.min_a_v.min(0.0);
        self.min_c_h *= other.min_c_h.min(1.0);
        self.min_c_v *= other.min_c_v.min(1.0);
        self.min_c_k *= other.min_c_k.min(1.0);
        self.min_t_h += other.min_t_h.min(0.0);
        self.min_t_v += other.min_t_v.min(0.0);
        self.min_t_k += other.min_t_k.min(0.0);
        for idx in 0..5 {
            self.panel_max[idx] += other.panel_max[idx].max(0.0);
            self.panel_min[idx] += other.panel_min[idx].min(0.0);
        }
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

fn part_own_bound(part: &Part, mode: &str) -> MetricBound {
    let mut b = MetricBound::neutral();
    if let Some(list) = part.display_mods.get(mode) {
        for m in list {
            if m.condition_id.is_some() || m.modifier_type != "Addend" {
                continue;
            }
            let Some(idx) = panel_attr_index(normalize_attr(m.attribute_id.as_deref())) else {
                continue;
            };
            let Some(delta) = m.delta.map(|v| v.0).filter(|v| v.is_finite()) else {
                continue;
            };
            b.panel_max[idx] += delta;
            b.panel_min[idx] += delta;
        }
    }
    if let Some(list) = part.main_mods.get(mode) {
        for m in list {
            if m.condition_id.is_none()
                && m.modifier_type == "Addend"
                && normalize_attr(m.attribute_id.as_deref()) == "10005"
            {
                b.max_d += m.delta.map(|v| v.0).unwrap_or(0.0);
            }
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
            let Some(p) = m.param1.map(|v| v.0) else { continue };
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
        if let Some(k) = part.raw_recoil.get(mode).and_then(|r| r.gunkick_spring_mult_c) {
            if k.is_finite() {
                b.min_c_k *= 1.0 + k;
            }
        }
    }
    for tune in &part.tuning_options {
        let mut max_h = f64::NEG_INFINITY;
        let mut max_v = f64::NEG_INFINITY;
        let mut max_k = f64::NEG_INFINITY;
        for v in tune_offer_values(tune) {
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
) -> FxHashMap<(String, String), MetricBound> {
    let mut part_cache: FxHashMap<String, MetricBound> = FxHashMap::default();
    let mut slot_cache: FxHashMap<(String, String), MetricBound> = FxHashMap::default();
    let mut visited: FxHashSet<String> = FxHashSet::default();
    let rules = payload.virtual_rules.get(weapon_id).cloned().unwrap_or_default();
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
                mode,
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
    mode: &str,
    id: &str,
    slot_rule_parts: &FxHashMap<String, Vec<String>>,
    part_cache: &mut FxHashMap<String, MetricBound>,
    slot_cache: &mut FxHashMap<(String, String), MetricBound>,
    visited: &mut FxHashSet<String>,
) -> MetricBound {
    if let Some(v) = part_cache.get(id) {
        return *v;
    }
    let Some(part) = payload.parts.get(id) else {
        return MetricBound::neutral();
    };
    let mut b = part_own_bound(part, mode);
    if visited.contains(id) {
        return b;
    }
    visited.insert(id.to_string());
    if let Some(slots) = payload.slots_by_host.get(id) {
        for slot in slots {
            let child = slot_bound(
                payload,
                mode,
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
    part_cache.insert(id.to_string(), b);
    b
}

#[allow(clippy::too_many_arguments)]
fn slot_bound(
    payload: &WeaponPayload,
    mode: &str,
    host: &str,
    slot: &crate::model::SlotDef,
    slot_rule_parts: &FxHashMap<String, Vec<String>>,
    part_cache: &mut FxHashMap<String, MetricBound>,
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
            mode,
            id,
            slot_rule_parts,
            part_cache,
            slot_cache,
            visited,
        );
        best.best_of(&b);
    }
    for id in &slot.virtual_candidate_ids {
        let b = part_bound(
            payload,
            mode,
            id,
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
                mode,
                id,
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
}

fn cubic_coeffs(a: &CurveKey, b: &CurveKey) -> (f64, f64, f64) {
    let dt = b.time - a.time;
    let c3 =
        2.0 * a.value + dt * a.leave_tangent - 2.0 * b.value + dt * b.arrive_tangent;
    let c2 = -3.0 * a.value - 2.0 * dt * a.leave_tangent + 3.0 * b.value
        - dt * b.arrive_tangent;
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

const TUNE_NEXT_LIMIT: usize = 16_384;

fn pareto_capped(
    mut offers: Vec<Offer>,
    dims: &TuneDims,
    limit: usize,
    deadline_at: Option<u64>,
) -> (Vec<Offer>, bool) {
    let channel = |dir: TuneDir, a: f64, b: f64| match dir {
        TuneDir::Higher => b.partial_cmp(&a),
        TuneDir::Lower | TuneDir::Equal => a.partial_cmp(&b),
        TuneDir::Ignore => Some(Ordering::Equal),
    }
    .unwrap_or(Ordering::Equal);
    offers.sort_by(|a, b| {
        channel(dims.open, a.e_open, b.e_open)
            .then(channel(dims.move_effect, a.e_move, b.e_move))
            .then(channel(dims.breath, a.e_breath, b.e_breath))
            .then(channel(dims.move_stability, a.e_move_stability, b.e_move_stability))
            .then(b.e_h.partial_cmp(&a.e_h).unwrap_or(Ordering::Equal))
            .then(b.e_v.partial_cmp(&a.e_v).unwrap_or(Ordering::Equal))
            .then(b.e_k.partial_cmp(&a.e_k).unwrap_or(Ordering::Equal))
            .then(a.deviation.partial_cmp(&b.deviation).unwrap_or(Ordering::Equal))
    });
    let mut truncated = false;
    let mut out: Vec<Offer> = Vec::new();
    for (idx, o) in offers.into_iter().enumerate() {
        if out.len() >= limit {
            truncated = true;
            break;
        }
        if idx & 1023 == 0 && deadline_at.is_some_and(|d| now_ms() >= d) {
            truncated = true;
            break;
        }
        if out.iter().any(|p| offer_dominates(p, &o, dims)) {
            continue;
        }
        out.retain(|p| !offer_dominates(&o, p, dims));
        out.push(o);
    }
    (out, truncated)
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

fn apply_linear_req(
    lower: &mut f64,
    upper: &mut f64,
    impossible: &mut bool,
    k: f64,
    base: f64,
    threshold: f64,
    is_max: bool,
) {
    if !k.is_finite() || !threshold.is_finite() {
        *impossible = true;
        return;
    }
    if k == 0.0 {
        if (is_max && 0.0 > threshold) || (!is_max && 0.0 < threshold) {
            *impossible = true;
        }
        return;
    }
    let bound = 100.0 * (base - threshold / k);
    if (k > 0.0) == is_max {
        *lower = lower.max(bound);
    } else {
        *upper = upper.min(bound);
    }
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
        if list
            .iter()
            .any(|e| e.condition_id.as_ref().map(|v| v.is("10047")).unwrap_or(false))
        {
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

fn compute_relevance(
    payload: &WeaponPayload,
    weapon_id: &str,
    options: &SolveOptions,
) -> Relevance {
    let mut parts: FxHashSet<String> = FxHashSet::default();
    let mut direct: FxHashSet<String> = FxHashSet::default();
    let mut attribute_constraints: FxHashSet<String> = FxHashSet::default();
    for c in &options.constraints {
        if c.id.chars().all(|ch| ch.is_ascii_digit()) {
            attribute_constraints.insert(c.id.clone());
        }
    }
    let detail_ids: FxHashSet<&str> = options
        .constraints
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
        for list in part.main_mods.values() {
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
            for list in part.display_mods.values() {
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
                        || (m.target_path == "GMovement_ADSSpeed"
                            && m.modifier_type == "Addend"))
                {
                    relevant = true;
                }
                if want_reload
                    && m.modifier_type == "Mult_A"
                    && m.target_path == "GReload_Time"
                {
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
            for list in part.display_mods.values() {
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

    let mut slots: FxHashSet<(String, String)> = FxHashSet::default();
    for (host, list) in &payload.slots_by_host {
        for slot in list {
            let mut slot_relevant = slot
                .candidate_ids
                .iter()
                .any(|id| parts.contains(id))
                || slot.virtual_candidate_ids.iter().any(|id| parts.contains(id));
            if !slot_relevant {
                if let Some(rules) = payload.virtual_rules.get(weapon_id) {
                    slot_relevant = rules
                        .iter()
                        .any(|r| r.socket_id == slot.id && parts.contains(&r.virtual_part_id));
                }
            }
            if slot_relevant {
                slots.insert((host.clone(), slot.id.clone()));
            }
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
            choices: Vec::new(),
        }
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
        let out = dp_pareto(vec![dominated, pricey_strong, cheap_weak], &dims, &[], AimDir::Ignore);
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
        let out = dp_pareto(vec![higher, lower], &dims, &[PanelDir::Lower], AimDir::Ignore);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].metric.panel[0], 40.0);
    }

    #[test]
    fn pareto_respects_panel_min_direction() {
        let lower = part(10.0, 10, vec![40.0]);
        let higher = part(10.0, 10, vec![50.0]);
        let out = dp_pareto(vec![lower, higher], &DpDims::default(), &[PanelDir::Higher], AimDir::Ignore);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].metric.panel[0], 50.0);
    }

    #[test]
    fn pareto_ignores_unconstrained_dimensions() {
        let cheap_weak = part(5.0, 5, vec![]);
        let pricey_strong = part(20.0, 10, vec![]);
        let out = dp_pareto(vec![pricey_strong, cheap_weak], &DpDims::default(), &[], AimDir::Ignore);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].cost, 5);
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
