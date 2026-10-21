use crate::assembly::{Assembler, VisibleSlot};
use crate::metrics::{
    eval_curve_points, quantize, tuning_available, tune_offer, tune_offer_values, Evaluator,
    TuningInstance, TuningState,
};
use crate::model::{Curve, Part, WeaponPayload};
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
    pub tuning: FxHashMap<String, f64>,
    pub constraints: Vec<ConstraintResult>,
    pub installed_parts: Vec<InstalledPart>,
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
    dp_truncated: bool,
    dp_ok: bool,
    panel_attrs: Vec<String>,
    dp_metric_cache: FxHashMap<(String, String), ExactPart>,
    control_curve: Option<Curve>,
    gunkick_curve: Option<Curve>,
    gunkick0: f64,
    weapon_price: i64,
    seed: bool,
    cost_cap: i64,
    run_found: bool,
    best: Option<Solution>,
    alternatives: Vec<Solution>,
    nodes: u64,
    leaves: u64,
    truncated: bool,
    stop: bool,
    best_cost: i64,
}

#[derive(Debug, Clone, Default)]
struct Relevance {
    slots: FxHashSet<(String, String)>,
}

#[derive(Debug, Clone)]
struct Offer {
    e_h: f64,
    e_v: f64,
    e_k: f64,
    values: Vec<(String, f64)>,
    deviation: f64,
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
            .filter(|c| c.id.chars().all(|ch| ch.is_ascii_digit()))
            .map(|c| c.id.clone())
            .collect();
        panel_attrs.sort();
        panel_attrs.dedup();
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
            dp_truncated: false,
            dp_ok: options.constraints.iter().all(|c| c.min.is_none()),
            panel_attrs,
            dp_metric_cache: FxHashMap::default(),
            control_curve,
            gunkick_curve,
            gunkick0,
            weapon_price,
            seed: false,
            cost_cap: i64::MAX,
            run_found: false,
            best: None,
            alternatives: Vec::new(),
            nodes: 0,
            leaves: 0,
            truncated: false,
            stop: false,
            best_cost: i64::MAX,
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
            if self.best.is_some() || (fixed_cap.is_some() && seed_found && !truncated) {
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
            if !self.slot_actions(slot, selections).is_empty() {
                next = Some(idx);
                break;
            }
        }

        let Some(idx) = next else {
            self.dp_leaf(selections, decided, &assembly);
            return;
        };

        if !self.seed {
            if self.cannot_satisfy(selections, &assembly, decided) {
                return;
            }
            if self.cost_cap < i64::MAX
                && self.cost_lower_bound(selections, &assembly, decided) + self.weapon_price
                    > self.cost_cap
            {
                return;
            }
            if self.best_cost < i64::MAX
                && self.cost_lower_bound(selections, &assembly, decided) + self.weapon_price
                    > self.best_cost
            {
                return;
            }
        }

        let slot = assembly.visible_slots[idx].clone();
        let path = slot.path_key.clone();
        for action in self.slot_actions(&slot, selections) {
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
            if remove_structural && remove.as_deref() != unset.as_deref() {
                forced.push(Action::Remove);
            }
            can_defer = !unset_structural;
        } else if unset.is_none() {
            forced.push(Action::Remove);
        }
        parts.sort_by(|a, b| {
            let pa = self.part_price(a);
            let pb = self.part_price(b);
            pa.cmp(&pb).then(a.cmp(b))
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
        let has_max = self.options.constraints.iter().any(|c| c.max.is_some());
        if !has_max {
            return false;
        }
        if self.control_curve.is_none() || self.gunkick_curve.is_none() || self.gunkick0 == 0.0 {
            return false;
        }
        let mut decided_bound = MetricBound::neutral();
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
            if !decided.contains(&item.slot_path_key) {
                continue;
            }
            if !seen.insert((item.slot_path_key.as_str(), item.part_id.as_str())) {
                continue;
            }
            if let Some(part) = self.payload.parts.get(&item.part_id) {
                decided_bound.add_a(&part_own_bound(part, &self.options.mode));
            }
        }
        let mut bound = decided_bound;
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
        let d_lo = decided_bound.max_d;
        let d_hi = bound.max_d;
        let cr_lo = self.control_curve.as_ref().and_then(|c| c.eval(d_lo));
        let cr_hi = self.control_curve.as_ref().and_then(|c| c.eval(d_hi));
        let cr = match (cr_lo, cr_hi) {
            (Some(a), Some(b)) => a.min(b),
            (Some(a), None) => a,
            (None, Some(b)) => b,
            (None, None) => return false,
        };
        let gk_lo = self.gunkick_curve.as_ref().and_then(|c| c.eval(d_lo));
        let gk_hi = self.gunkick_curve.as_ref().and_then(|c| c.eval(d_hi));
        let gk = match (gk_lo, gk_hi) {
            (Some(a), Some(b)) => a.min(b),
            (Some(a), None) => a,
            (None, Some(b)) => b,
            (None, None) => return false,
        };
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

        let mut base = ExactPart::neutral_panel(self.panel_attrs.len());
        let mut base_cost = 0i64;
        let mut base_parts: Vec<(&Part, &str)> = Vec::new();
        let mut seen_paths: FxHashSet<(&str, &str)> = FxHashSet::default();
        for slot in &assembly.visible_slots {
            if leaf_paths.contains(&slot.path_key) {
                continue;
            }
            let Some(id) = slot
                .selected_part_id
                .as_ref()
                .or(slot.internal_part_id.as_ref())
            else {
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
        for item in &assembly.internal_parts {
            if leaf_paths.contains(&item.slot_path_key) {
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
            let m = exact_part_metric(part, slot_name, &self.options.mode, self.options.bipod_deployed, is_laser, include_pro, &self.panel_attrs);
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

        let mut states: Vec<DpState> = vec![DpState {
            cost: base_cost,
            metric: base,
            choices: Vec::new(),
        }];
        for opts in &options_per_slot {
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
                    let mut metric = st.metric.clone();
                    metric.add(&opt.metric);
                    let mut choices = st.choices.clone();
                    choices.push(opt.choice.clone());
                    next.push(DpState { cost, metric, choices });
                }
            }
            next = dp_pareto(next);
            if next.len() > 200_000 {
                next.sort_by_key(|s| s.cost);
                next.truncate(200_000);
                self.dp_truncated = true;
                self.truncated = true;
            }
            states = next;
            if states.is_empty() {
                return;
            }
        }

        states.sort_by_key(|s| s.cost);
        for st in states {
            let total = st.cost + self.weapon_price;
            if self.best_cost < i64::MAX && total >= self.best_cost {
                continue;
            }
            let mut trial = selections.clone();
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
            for slot in &assembly.visible_slots {
                if !decided.contains(&slot.path_key) {
                    trial.entry(slot.path_key.clone()).or_insert(None);
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
        if self.leaves.is_multiple_of(4096) && self.budget_exceeded() {
            return;
        }
        let assembly = self.assembler.resolve(selections);
        let installed: Vec<Part> = self
            .evaluator
            .installed_parts(&assembly)
            .into_iter()
            .cloned()
            .collect();
        let Some(rec) = self.recoil_base(&installed) else {
            return;
        };
        let (attachment_price, total_price, _) = self.evaluator.price(&assembly);
        if total_price >= self.best_cost || total_price > self.cost_cap {
            return;
        }

        let instances = self.evaluator.tunable_instances(&assembly);
        let req = match self.tune_requirements(&rec) {
            Some(r) => r,
            None => return,
        };
        let Some((tuning_state, chosen)) = self.pick_tuning(&instances, &req) else {
            return;
        };

        let Some(metrics) = self.evaluator.evaluate(&self.assembler, &assembly, &tuning_state) else {
            return;
        };

        let mut constraint_results = Vec::new();
        let mut ok = true;
        for c in &self.options.constraints {
            let value = match metric_value(c.id.as_str(), &metrics) {
                Some(v) => v,
                None => continue,
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
                a_h: rec.a_h,
                a_v: rec.a_v,
                c_h: rec.c_h,
                c_v: rec.c_v,
                c_k: rec.c_k,
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
            tuning,
            constraints: constraint_results,
            installed_parts,
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
            let Some(thr) = c.max else { continue };
            match c.id.as_str() {
                "recoil-continuous-horizontal" => {
                    apply_linear_req(&mut req.lower_h, &mut req.upper_h, &mut req.impossible, b.cr * b.c_h, 1.0 + b.a_h, thr);
                }
                "recoil-continuous-vertical" => {
                    apply_linear_req(&mut req.lower_v, &mut req.upper_v, &mut req.impossible, b.cr * b.c_v, 1.0 + b.a_v, thr);
                }
                "recoil-single-horizontal" => {
                    apply_linear_req(&mut req.lower_h, &mut req.upper_h, &mut req.impossible, b.cr * b.c_h, 1.0 + b.a_h, thr);
                }
                "recoil-single-vertical" => {
                    apply_linear_req(&mut req.lower_v, &mut req.upper_v, &mut req.impossible, b.cr * b.c_v, 1.0 + b.a_v, thr);
                }
                "recoil-fire-stability" => {
                    apply_linear_req(&mut req.lower_k, &mut req.upper_k, &mut req.impossible, b.gk_ratio * b.c_k, 1.0, thr);
                }
                _ => {}
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
    ) -> Option<(TuningState, Vec<(String, f64)>)> {
        let mut frontier: Vec<Offer> = vec![Offer {
            e_h: 0.0,
            e_v: 0.0,
            e_k: 0.0,
            values: Vec::new(),
            deviation: 0.0,
        }];
        for inst in instances {
            let mut group: Vec<Offer> = vec![Offer {
                e_h: 0.0,
                e_v: 0.0,
                e_k: 0.0,
                values: Vec::new(),
                deviation: 0.0,
            }];
            for tune in &inst.part.tuning_options {
                if !tuning_available(&inst.slot_name, inst.part) {
                    continue;
                }
                let offers = {
                    let key_base =
                        Evaluator::tuning_key(&inst.slot_path, &inst.part.id, &tune.tune_id);
                    if let Some(v) = self.options.locked_tuning.get(&key_base).copied() {
                        let v = quantize(v, tune.min_value, tune.max_value, tune.step_value);
                        let (e_h, e_v, e_k) = tune_offer(tune, v);
                        vec![LocalOffer { e_h, e_v, e_k, value: v }]
                    } else {
                        self.tune_offers(&inst.part.id, tune)
                    }
                };
                let key_base = Evaluator::tuning_key(&inst.slot_path, &inst.part.id, &tune.tune_id);
                let default = tune.default_value;
                let mut next = Vec::new();
                for a in &group {
                    for b in &offers {
                        let mut values = a.values.clone();
                        values.push((key_base.clone(), b.value));
                        next.push(Offer {
                            e_h: a.e_h + b.e_h,
                            e_v: a.e_v + b.e_v,
                            e_k: a.e_k + b.e_k,
                            values,
                            deviation: a.deviation + (b.value - default).abs(),
                        });
                    }
                }
                group = pareto(next);
            }
            if group.len() == 1
                && group[0].e_h == 0.0
                && group[0].e_v == 0.0
                && group[0].e_k == 0.0
                && group[0].values.is_empty()
            {
                continue;
            }
            let mut next = Vec::new();
            for a in &frontier {
                for b in &group {
                    let mut values = a.values.clone();
                    values.extend(b.values.iter().cloned());
                    next.push(Offer {
                        e_h: a.e_h + b.e_h,
                        e_v: a.e_v + b.e_v,
                        e_k: a.e_k + b.e_k,
                        values,
                        deviation: a.deviation + b.deviation,
                    });
                }
            }
            frontier = pareto(next);
            if frontier.len() > 4096 {
                frontier.truncate(4096);
            }
        }

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
                    value: o.values.first().map(|v| v.1).unwrap_or(tune.default_value),
                })
                .collect();
        }
        let values = tune_offer_values(tune);
        let mut offers: Vec<LocalOffer> = values
            .into_iter()
            .map(|v| {
                let (e_h, e_v, e_k) = tune_offer(tune, v);
                LocalOffer { e_h, e_v, e_k, value: v }
            })
            .collect();
        offers.sort_by(|a, b| {
            b.e_h
                .partial_cmp(&a.e_h)
                .unwrap_or(Ordering::Equal)
                .then(b.e_v.partial_cmp(&a.e_v).unwrap_or(Ordering::Equal))
                .then(b.e_k.partial_cmp(&a.e_k).unwrap_or(Ordering::Equal))
                .then(
                    (a.value - tune.default_value)
                        .abs()
                        .partial_cmp(&(b.value - tune.default_value).abs())
                        .unwrap_or(Ordering::Equal),
                )
        });
        let mut pruned: Vec<LocalOffer> = Vec::new();
        for o in offers {
            if pruned.iter().any(|p| {
                p.e_h >= o.e_h - 1e-9 && p.e_v >= o.e_v - 1e-9 && p.e_k >= o.e_k - 1e-9
            }) {
                continue;
            }
            pruned.retain(|p| !(o.e_h >= p.e_h - 1e-9 && o.e_v >= p.e_v - 1e-9 && o.e_k >= p.e_k - 1e-9));
            pruned.push(o);
        }
        pruned.sort_by(|a, b| {
            b.e_h
                .partial_cmp(&a.e_h)
                .unwrap_or(Ordering::Equal)
                .then(b.e_v.partial_cmp(&a.e_v).unwrap_or(Ordering::Equal))
                .then(b.e_k.partial_cmp(&a.e_k).unwrap_or(Ordering::Equal))
                .then(
                    (a.value - tune.default_value)
                        .abs()
                        .partial_cmp(&(b.value - tune.default_value).abs())
                        .unwrap_or(Ordering::Equal),
                )
        });
        let key_base = String::new();
        let _ = key_base;
        let cached: Vec<Offer> = pruned
            .iter()
            .map(|o| Offer {
                e_h: o.e_h,
                e_v: o.e_v,
                e_k: o.e_k,
                values: vec![(String::new(), o.value)],
                deviation: (o.value - tune.default_value).abs(),
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
    if !crate::metrics::tuning_available(slot_name, part) {
        return m;
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
            m.eh += max_h;
        }
        if max_v.is_finite() {
            m.ev += max_v;
        }
        if max_k.is_finite() {
            m.ek += max_k;
        }
    }
    m
}

fn dp_dominates(a: &DpState, b: &DpState) -> bool {
    let x = &a.metric;
    let y = &b.metric;
    a.cost <= b.cost
        && x.d >= y.d - 1e-9
        && x.ah <= y.ah + 1e-9
        && x.av <= y.av + 1e-9
        && x.ch <= y.ch + 1e-9
        && x.cv <= y.cv + 1e-9
        && x.ck <= y.ck + 1e-9
        && x.eh >= y.eh - 1e-9
        && x.ev >= y.ev - 1e-9
        && x.ek >= y.ek - 1e-9
        && x.panel.len() == y.panel.len()
        && x.panel
            .iter()
            .zip(y.panel.iter())
            .all(|(a, b)| *a <= *b + 1e-9)
}

fn dp_pareto(mut states: Vec<DpState>) -> Vec<DpState> {
    states.sort_by(|a, b| {
        a.cost
            .cmp(&b.cost)
            .then(b.metric.score().partial_cmp(&a.metric.score()).unwrap_or(Ordering::Equal))
    });
    let mut out: Vec<DpState> = Vec::new();
    for st in states {
        if out.iter().any(|o| dp_dominates(o, &st)) {
            continue;
        }
        out.retain(|o| !dp_dominates(&st, o));
        out.push(st);
    }
    out
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
    }
}

fn part_own_bound(part: &Part, mode: &str) -> MetricBound {
    let mut b = MetricBound::neutral();
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

fn pareto(mut offers: Vec<Offer>) -> Vec<Offer> {
    offers.sort_by(|a, b| {
        b.e_h
            .partial_cmp(&a.e_h)
            .unwrap_or(Ordering::Equal)
            .then(b.e_v.partial_cmp(&a.e_v).unwrap_or(Ordering::Equal))
            .then(b.e_k.partial_cmp(&a.e_k).unwrap_or(Ordering::Equal))
            .then(a.deviation.partial_cmp(&b.deviation).unwrap_or(Ordering::Equal))
    });
    let mut out: Vec<Offer> = Vec::new();
    for o in offers {
        if out.iter().any(|p| {
            p.e_h >= o.e_h - 1e-9 && p.e_v >= o.e_v - 1e-9 && p.e_k >= o.e_k - 1e-9
        }) {
            continue;
        }
        out.retain(|p| {
            !(o.e_h >= p.e_h - 1e-9 && o.e_v >= p.e_v - 1e-9 && o.e_k >= p.e_k - 1e-9)
        });
        out.push(o);
    }
    out
}

fn apply_linear_req(
    lower: &mut f64,
    upper: &mut f64,
    impossible: &mut bool,
    k: f64,
    base: f64,
    threshold: f64,
) {
    if !k.is_finite() || !threshold.is_finite() {
        *impossible = true;
        return;
    }
    if k == 0.0 {
        if 0.0 > threshold {
            *impossible = true;
        }
        return;
    }
    let bound = 100.0 * (base - threshold / k);
    if k > 0.0 {
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
        other => metrics.panel.get(other).copied(),
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
            }
        }
        for tune in &part.tuning_options {
            for effect in &tune.display_effects {
                if effect.attribute_name_snapshot == "%额外后坐力控制"
                    || effect.attribute_name_snapshot == "%开火时武器稳定性"
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
        let out = dp_pareto(vec![dominated, pricey_strong, cheap_weak]);
        assert_eq!(out.len(), 2);
        assert!(out.iter().any(|s| s.cost == 5));
        assert!(out.iter().any(|s| s.cost == 10));
    }

    #[test]
    fn pareto_uses_panel_values() {
        let lower = part(10.0, 10, vec![40.0]);
        let higher = part(10.0, 10, vec![50.0]);
        let out = dp_pareto(vec![higher, lower]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].metric.panel[0], 40.0);
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
