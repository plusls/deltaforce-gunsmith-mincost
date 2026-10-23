use gunsmith_mincost::assembly::{Assembler, Assembly};
use gunsmith_mincost::metrics::{quantize, Evaluator, TuningState};
use gunsmith_mincost::model::{Map, WeaponPayload};
use gunsmith_mincost::search::Constraint;
use rustc_hash::FxHashMap;
use serde_json::{json, Value};
use std::time::Instant;

struct Spec {
    payload: Value,
    request: Value,
    max_assemblies: usize,
    max_evals: usize,
    max_values_per_tune: usize,
    dump: bool,
    solution: Option<Value>,
    eval_selections: Vec<Value>,
}

fn parse_spec(path: &str) -> Spec {
    let text = std::fs::read_to_string(path).unwrap();
    let v: Value = serde_json::from_str(&text).unwrap();
    Spec {
        payload: v["payload"].clone(),
        request: v["request"].clone(),
        max_assemblies: v["maxAssemblies"].as_u64().unwrap_or(2_000_000) as usize,
        max_evals: v["maxEvaluations"].as_u64().unwrap_or(20_000_000) as usize,
        max_values_per_tune: v["maxValuesPerTune"].as_u64().unwrap_or(128) as usize,
        dump: v["dump"].as_bool().unwrap_or(false),
        solution: v.get("solution").cloned(),
        eval_selections: v["evalSelections"].as_array().cloned().unwrap_or_default(),
    }
}

fn metric_value(id: &str, m: &gunsmith_mincost::metrics::BuildMetrics) -> Option<f64> {
    match id {
        "recoil-continuous-horizontal" => Some(m.recoil.continuous_h),
        "recoil-continuous-vertical" => Some(m.recoil.continuous_v),
        "recoil-single-horizontal" => Some(m.recoil.single_h),
        "recoil-single-vertical" => Some(m.recoil.single_v),
        "recoil-fire-stability" => Some(m.recoil.fire_stability),
        other => m
            .detail
            .value(other)
            .or_else(|| m.panel.get(other).copied()),
    }
}

fn check(
    constraints: &[Constraint],
    m: &gunsmith_mincost::metrics::BuildMetrics,
) -> (bool, Vec<Value>) {
    let mut ok = true;
    let mut out = Vec::new();
    for c in constraints {
        let Some(value) = metric_value(&c.id, m) else {
            return (false, out);
        };
        let ok_this = c.min.map(|v| value >= v - 1e-9).unwrap_or(true)
            && c.max.map(|v| value <= v + 1e-9).unwrap_or(true);
        ok &= ok_this;
        out.push(json!({"id": c.id, "value": value, "ok": ok_this}));
    }
    (ok, out)
}

fn tune_values(tune: &gunsmith_mincost::model::TuningOption, cap: usize) -> Option<Vec<f64>> {
    let mut vals = vec![tune.min_value, tune.max_value, tune.default_value];
    for e in &tune.display_effects {
        for k in &e.curve_points {
            vals.push(k.time);
        }
    }
    if tune.step_value > 0.0 {
        let n = ((tune.max_value - tune.min_value) / tune.step_value).floor();
        if n < 0.0 {
            return Some(vec![]);
        }
        let count = n as usize + 1;
        if count > cap * 4 {
            return None;
        }
        for i in 0..count {
            vals.push(tune.min_value + i as f64 * tune.step_value);
        }
    }
    let mut out: Vec<f64> = vals
        .into_iter()
        .map(|v| quantize(v, tune.min_value, tune.max_value, tune.step_value))
        .collect();
    out.sort_by(|a, b| a.partial_cmp(b).unwrap());
    out.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
    if out.len() > cap {
        return None;
    }
    Some(out)
}

struct Bf<'a> {
    assembler: Assembler<'a>,
    evaluator: Evaluator<'a>,
    constraints: Vec<Constraint>,
    locks: Map<String, Option<String>>,
    locked_tuning: FxHashMap<String, f64>,
    max_assemblies: usize,
    max_evals: usize,
    max_values_per_tune: usize,
    assemblies: usize,
    evals: usize,
    feasible: usize,
    skipped_tuning: usize,
    lock_rejected: usize,
    evals_truncated: bool,
    asm_truncated: bool,
    best_cost: Option<i64>,
    best: Option<Value>,
    rows: Vec<Value>,
}

impl<'a> Bf<'a> {
    fn dfs(&mut self, sel: Map<String, Option<String>>, decided: rustc_hash::FxHashSet<String>) {
        if self.assemblies >= self.max_assemblies {
            self.asm_truncated = true;
            return;
        }
        let asm = self.assembler.resolve(&sel);
        let next = asm
            .visible_slots
            .iter()
            .find(|s| !decided.contains(&s.path_key))
            .cloned();
        match next {
            None => {
                self.assemblies += 1;
                self.eval_assembly(&sel, &asm);
            }
            Some(slot) => {
                let mut opts: Vec<Option<Option<String>>> = Vec::new();
                if let Some(lock) = self.locks.get(&slot.path_key) {
                    opts.push(Some(lock.clone()));
                } else {
                    if slot.internal_is_default || slot.internal_part_id.is_some() {
                        opts.push(None);
                    }
                    opts.push(Some(None));
                    for c in &slot.candidate_ids {
                        opts.push(Some(Some(c.clone())));
                    }
                }
                for opt in opts {
                    let mut s2 = sel.clone();
                    match opt {
                        None => {}
                        Some(v) => {
                            s2.insert(slot.path_key.clone(), v);
                        }
                    }
                    let mut d2 = decided.clone();
                    d2.insert(slot.path_key.clone());
                    self.dfs(s2, d2);
                }
            }
        }
    }

    /// Verifies the resolved assembly still honours every effective lock,
    /// including the implicit slot locks derived from lockedTuningKeys. The
    /// primitive is shared with the solver so the oracle cannot drift from it.
    fn locks_ok(&self, asm: &Assembly) -> bool {
        gunsmith_mincost::assembly::locks_honoured(&self.locks, asm)
    }

    fn lock_report(&self, asm: &Assembly) -> Value {
        let mut valid = true;
        let mut checked = Vec::new();
        let mut paths: Vec<&String> = self.locks.keys().collect();
        paths.sort();
        for path in paths {
            let expected = self.locks.get(path).cloned().flatten();
            let slot_found = asm.visible_slots.iter().any(|s| &s.path_key == path)
                || asm.internal_parts.iter().any(|p| &p.slot_path_key == path);
            let actual = gunsmith_mincost::assembly::installed_part_id(asm, path);
            let ok = gunsmith_mincost::assembly::lock_honoured(asm, path, expected.as_deref());
            valid &= ok;
            checked.push(json!({
                "path": path,
                "expected": expected,
                "actual": actual,
                "slotFound": slot_found,
                "ok": ok,
            }));
        }
        json!({"valid": valid, "checked": checked})
    }

    fn eval_assembly(&mut self, sel: &Map<String, Option<String>>, asm: &Assembly) {
        if !self.locks_ok(asm) {
            self.lock_rejected += 1;
            return;
        }
        let instances = self.evaluator.tunable_instances(asm);
        let mut groups: Vec<(String, Vec<f64>)> = Vec::new();
        let mut skip = false;
        for inst in &instances {
            if !gunsmith_mincost::metrics::tuning_available(&inst.slot_name, inst.part) {
                continue;
            }
            for tune in &inst.part.tuning_options {
                let key = Evaluator::tuning_key(&inst.slot_path, &inst.part.id, &tune.tune_id);
                let vals = if let Some(v) = self.locked_tuning.get(&key) {
                    vec![quantize(
                        *v,
                        tune.min_value,
                        tune.max_value,
                        tune.step_value,
                    )]
                } else {
                    match tune_values(tune, self.max_values_per_tune) {
                        Some(v) => v,
                        None => {
                            skip = true;
                            break;
                        }
                    }
                };
                if !vals.is_empty() {
                    groups.push((key, vals));
                }
            }
            if skip {
                break;
            }
        }
        if skip {
            self.skipped_tuning += 1;
            return;
        }
        let mut state = TuningState::default();
        self.enumerate(sel, asm, &groups, 0, &mut state);
    }

    fn enumerate(
        &mut self,
        sel: &Map<String, Option<String>>,
        asm: &Assembly,
        groups: &[(String, Vec<f64>)],
        idx: usize,
        state: &mut TuningState,
    ) {
        if idx == groups.len() {
            if self.evals >= self.max_evals {
                self.evals_truncated = true;
                return;
            }
            self.evals += 1;
            let Some(metrics) = self.evaluator.evaluate(&self.assembler, asm, state) else {
                return;
            };
            let (ok, results) = check(&self.constraints, &metrics);
            if !ok {
                return;
            }
            self.feasible += 1;
            let (_, total, _) = self.evaluator.price(asm);
            if self.best_cost.map(|b| total < b).unwrap_or(true) {
                self.best_cost = Some(total);
                let mut selections = serde_json::Map::new();
                for (k, v) in sel {
                    selections.insert(k.clone(), json!(v));
                }
                let mut tuning = serde_json::Map::new();
                for (k, v) in state.iter() {
                    tuning.insert(k.clone(), json!(v));
                }
                self.best = Some(json!({
                    "totalPrice": total,
                    "selections": selections,
                    "tuning": tuning,
                    "constraints": results,
                    "lockCheck": self.lock_report(asm),
                    "panel": metrics.panel,
                    "recoil": {
                        "continuousH": metrics.recoil.continuous_h,
                        "continuousV": metrics.recoil.continuous_v,
                        "singleH": metrics.recoil.single_h,
                        "singleV": metrics.recoil.single_v,
                        "fireStability": metrics.recoil.fire_stability,
                    },
                }));
            }
            return;
        }
        let (key, vals) = &groups[idx];
        for v in vals {
            state.insert(key.clone(), *v);
            self.enumerate(sel, asm, groups, idx + 1, state);
            if self.evals_truncated {
                return;
            }
        }
        state.remove(key);
    }

    fn dump_assembly(&mut self, sel: &Map<String, Option<String>>, asm: &Assembly) {
        let instances = self.evaluator.tunable_instances(asm);
        let mut state = TuningState::default();
        for inst in &instances {
            for tune in &inst.part.tuning_options {
                let key = Evaluator::tuning_key(&inst.slot_path, &inst.part.id, &tune.tune_id);
                let raw = self
                    .assembler
                    .default_tune(&inst.slot_path, &inst.part.id, &tune.tune_id)
                    .unwrap_or(tune.default_value);
                state.insert(
                    key,
                    quantize(raw, tune.min_value, tune.max_value, tune.step_value),
                );
            }
        }
        let Some(metrics) = self.evaluator.evaluate(&self.assembler, asm, &state) else {
            return;
        };
        let (_, total, _) = self.evaluator.price(asm);
        let mut selections = serde_json::Map::new();
        for (k, v) in sel {
            selections.insert(k.clone(), json!(v));
        }
        let mut tuning = serde_json::Map::new();
        for (k, v) in state.iter() {
            tuning.insert(k.clone(), json!(v));
        }
        self.rows.push(json!({
            "selections": selections,
            "tuning": tuning,
            "totalPrice": total,
            "panel": metrics.panel,
            "recoil": {
                "continuousH": metrics.recoil.continuous_h,
                "continuousV": metrics.recoil.continuous_v,
                "singleH": metrics.recoil.single_h,
                "singleV": metrics.recoil.single_v,
                "fireStability": metrics.recoil.fire_stability,
            },
            "detail": metrics.detail,
        }));
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(path) = args.first() else {
        eprintln!("usage: bf <spec.json>");
        std::process::exit(2);
    };
    let spec = parse_spec(path);
    let payload_bytes = serde_json::to_vec(&spec.payload).unwrap();
    let payload: WeaponPayload = serde_json::from_value(spec.payload.clone()).unwrap();
    let weapon_id = spec.request["weaponId"]
        .as_str()
        .map(|s| s.to_string())
        .or_else(|| payload.weapon_id.clone())
        .or_else(|| payload.weapon_ids.first().cloned())
        .expect("weapon id");
    let mode = spec.request["mode"]
        .as_str()
        .unwrap_or("operations")
        .to_string();
    let laser = spec.request["laserEnabled"].as_bool().unwrap_or(false);
    let bipod = spec.request["bipodDeployed"].as_bool().unwrap_or(false);
    let constraints: Vec<Constraint> =
        serde_json::from_value(spec.request["constraints"].clone()).unwrap_or_default();
    let mut locks: Map<String, Option<String>> =
        serde_json::from_value(spec.request["lockedPathKeys"].clone()).unwrap_or_default();
    let locked_tuning: FxHashMap<String, f64> =
        serde_json::from_value(spec.request["lockedTuningKeys"].clone()).unwrap_or_default();
    let excluded: Vec<String> =
        serde_json::from_value(spec.request["excludedPartIds"].clone()).unwrap_or_default();
    let prices_f: FxHashMap<String, f64> =
        serde_json::from_value(spec.request["prices"].clone()).unwrap_or_default();
    let prices: FxHashMap<String, i64> = prices_f
        .iter()
        .map(|(k, v)| (k.clone(), v.round() as i64))
        .collect();

    // Mirror the request preprocessing of solve_request so the oracle sees the
    // same effective locks: a locked tuning key implies a slot lock on its part.
    let locked_tuning_keys =
        match gunsmith_mincost::expand_locked_tuning(&mut locks, &locked_tuning) {
            Ok(keys) => keys,
            Err(e) => {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        };
    let excluded_set: rustc_hash::FxHashSet<String> = excluded.iter().cloned().collect();
    if let Err(e) = gunsmith_mincost::validate_locks(
        &payload,
        &weapon_id,
        &locks,
        &excluded_set,
        &locked_tuning_keys,
    ) {
        eprintln!("error: {e}");
        std::process::exit(1);
    }

    let mut assembler = Assembler::new(&payload, &weapon_id);
    assembler.set_excluded(excluded_set);
    let evaluator = Evaluator {
        payload: &payload,
        weapon_id: &weapon_id,
        mode: &mode,
        laser_enabled: laser,
        bipod_deployed: bipod,
        prices: &prices,
    };

    let started = Instant::now();
    let mut bf = Bf {
        assembler,
        evaluator,
        constraints: constraints.clone(),
        locks,
        locked_tuning,
        max_assemblies: spec.max_assemblies,
        max_evals: spec.max_evals,
        max_values_per_tune: spec.max_values_per_tune,
        assemblies: 0,
        evals: 0,
        feasible: 0,
        skipped_tuning: 0,
        lock_rejected: 0,
        evals_truncated: false,
        asm_truncated: false,
        best_cost: None,
        best: None,
        rows: Vec::new(),
    };
    let mut initial = Map::default();
    for (k, v) in &bf.locks {
        initial.insert(k.clone(), v.clone());
    }
    let mut decided = rustc_hash::FxHashSet::default();
    for k in initial.keys() {
        decided.insert(k.clone());
    }
    if spec.dump {
        bf.dump_dfs(initial, decided);
    } else if !spec.eval_selections.is_empty() {
        for entry in &spec.eval_selections {
            let selections: Map<String, Option<String>> =
                serde_json::from_value(entry["selections"].clone()).unwrap_or_default();
            let tuning_value = &entry["tuning"];
            let tuning: TuningState = if tuning_value.is_null() {
                let asm = bf.assembler.resolve(&selections);
                let mut t = TuningState::default();
                for inst in bf.evaluator.tunable_instances(&asm) {
                    for tune in &inst.part.tuning_options {
                        let key =
                            Evaluator::tuning_key(&inst.slot_path, &inst.part.id, &tune.tune_id);
                        let raw = bf
                            .assembler
                            .default_tune(&inst.slot_path, &inst.part.id, &tune.tune_id)
                            .unwrap_or(tune.default_value);
                        t.insert(
                            key,
                            quantize(raw, tune.min_value, tune.max_value, tune.step_value),
                        );
                    }
                }
                t
            } else {
                serde_json::from_value(tuning_value.clone()).unwrap_or_default()
            };
            bf.eval_given(&selections, &tuning);
        }
    } else {
        bf.dfs(initial, decided);
    }
    let elapsed = started.elapsed().as_secs_f64();

    let solve_started = Instant::now();
    let request_bytes = serde_json::to_vec(&spec.request).unwrap();
    let solved = gunsmith_mincost::solve_slices(&payload_bytes, &request_bytes);
    let solve_elapsed = solve_started.elapsed().as_secs_f64();

    let witness_lock = bf
        .best
        .as_ref()
        .and_then(|w| w.get("lockCheck").cloned())
        .unwrap_or(Value::Null);
    let mut out = json!({
        "brute": {
            "minTotalPrice": bf.best_cost,
            "feasibleCount": bf.feasible,
            "assemblies": bf.assemblies,
            "tuningEvals": bf.evals,
            "skippedTuningGroups": bf.skipped_tuning,
            "lockRejectedAssemblies": bf.lock_rejected,
            "assembliesTruncated": bf.asm_truncated,
            "evalsTruncated": bf.evals_truncated,
            "elapsedS": elapsed,
            "witness": bf.best,
            "witnessLock": witness_lock,
        },
        "solve": match &solved {
            Ok(bytes) => {
                let v: Value = serde_json::from_slice(bytes).unwrap();
                json!({
                    "ok": true,
                    "optimal": v["optimal"],
                    "truncated": v["truncated"],
                    "message": v["message"],
                    "nodes": v["nodes"],
                    "leaves": v["leaves"],
                    "totalPrice": v["best"]["total_price"],
                    "selections": v["best"]["selections"],
                    "tuning": v["best"]["tuning"],
                    "constraints": v["best"]["constraints"],
                    "panel": v["best"]["panel"],
                    "recoil": {
                        "continuousH": v["best"]["recoil"]["continuous_h"],
                        "continuousV": v["best"]["recoil"]["continuous_v"],
                        "singleH": v["best"]["recoil"]["single_h"],
                        "singleV": v["best"]["recoil"]["single_v"],
                        "fireStability": v["best"]["recoil"]["fire_stability"],
                    },
                    "detail": v["best"]["detail"],
                })
            }
            Err(e) => json!({"ok": false, "error": e}),
        },
        "solveElapsedS": solve_elapsed,
        "compare": Value::Null,
    });
    if spec.dump || !spec.eval_selections.is_empty() {
        out["rows"] = json!(bf.rows);
    }
    let recheck = spec.solution.clone().or_else(|| {
        let bytes = solved.as_ref().ok()?;
        let v: Value = serde_json::from_slice(bytes).ok()?;
        if v["best"].is_null() {
            None
        } else {
            Some(json!({
                "selections": v["best"]["selections"].clone(),
                "tuning": v["best"]["tuning"].clone(),
            }))
        }
    });
    if let Some(sol) = recheck {
        let selections: Map<String, Option<String>> =
            serde_json::from_value(sol["selections"].clone()).unwrap_or_default();
        let tuning: TuningState = serde_json::from_value(sol["tuning"].clone()).unwrap_or_default();
        let asm = bf.assembler.resolve(&selections);
        let metrics = bf.evaluator.evaluate(&bf.assembler, &asm, &tuning);
        let (_, total, _) = bf.evaluator.price(&asm);
        out["recheckSolution"] = match metrics {
            Some(m) => {
                let (ok, results) = check(&bf.constraints, &m);
                json!({
                    "feasible": ok,
                    "constraints": results,
                    "totalPrice": total,
                    "panel": m.panel,
                    "recoil": {
                        "continuousH": m.recoil.continuous_h,
                        "continuousV": m.recoil.continuous_v,
                        "singleH": m.recoil.single_h,
                        "singleV": m.recoil.single_v,
                        "fireStability": m.recoil.fire_stability,
                    },
                    "detail": m.detail,
                })
            }
            None => json!({"error": "evaluate returned None"}),
        };
    }
    println!("{}", serde_json::to_string_pretty(&out).unwrap());
}

impl<'a> Bf<'a> {
    fn eval_given(&mut self, sel: &Map<String, Option<String>>, tuning: &TuningState) {
        let asm = self.assembler.resolve(sel);
        let Some(metrics) = self.evaluator.evaluate(&self.assembler, &asm, tuning) else {
            return;
        };
        let (_, total, _) = self.evaluator.price(&asm);
        let mut selections = serde_json::Map::new();
        for (k, v) in sel {
            selections.insert(k.clone(), json!(v));
        }
        let mut tune_out = serde_json::Map::new();
        for (k, v) in tuning.iter() {
            tune_out.insert(k.clone(), json!(v));
        }
        self.rows.push(json!({
            "selections": selections,
            "tuning": tune_out,
            "totalPrice": total,
            "panel": metrics.panel,
            "recoil": {
                "continuousH": metrics.recoil.continuous_h,
                "continuousV": metrics.recoil.continuous_v,
                "singleH": metrics.recoil.single_h,
                "singleV": metrics.recoil.single_v,
                "fireStability": metrics.recoil.fire_stability,
            },
            "detail": metrics.detail,
        }));
    }

    fn dump_dfs(
        &mut self,
        sel: Map<String, Option<String>>,
        decided: rustc_hash::FxHashSet<String>,
    ) {
        if self.assemblies >= self.max_assemblies {
            self.asm_truncated = true;
            return;
        }
        let asm = self.assembler.resolve(&sel);
        let next = asm
            .visible_slots
            .iter()
            .find(|s| !decided.contains(&s.path_key))
            .cloned();
        match next {
            None => {
                self.assemblies += 1;
                self.dump_assembly(&sel, &asm);
            }
            Some(slot) => {
                let mut opts: Vec<Option<Option<String>>> = Vec::new();
                if let Some(lock) = self.locks.get(&slot.path_key) {
                    opts.push(Some(lock.clone()));
                } else {
                    if slot.internal_is_default || slot.internal_part_id.is_some() {
                        opts.push(None);
                    }
                    opts.push(Some(None));
                    for c in &slot.candidate_ids {
                        opts.push(Some(Some(c.clone())));
                    }
                }
                for opt in opts {
                    let mut s2 = sel.clone();
                    match opt {
                        None => {}
                        Some(v) => {
                            s2.insert(slot.path_key.clone(), v);
                        }
                    }
                    let mut d2 = decided.clone();
                    d2.insert(slot.path_key.clone());
                    self.dump_dfs(s2, d2);
                }
            }
        }
    }
}
