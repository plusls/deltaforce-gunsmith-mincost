use crate::assembly::{Assembly, Assembler};
use crate::model::{
    AimRecoil, CurveKey, FlexibleId, Modifier, Part, TuningOption,
    WeaponPayload,
};
use rustc_hash::{FxHashMap, FxHashSet};

pub type TuningState = FxHashMap<String, f64>;

#[derive(Debug, Clone, Default)]
pub struct RecoilResult {
    pub d: f64,
    pub f_h: f64,
    pub f_v: f64,
    pub s: f64,
    pub continuous_h: f64,
    pub continuous_v: f64,
    pub single_h: f64,
    pub single_v: f64,
    pub fire_stability: f64,
    pub tuning_effects: FxHashMap<String, f64>,
    pub tuning_runtime: FxHashMap<String, f64>,
}

#[derive(Debug, Clone, Default)]
pub struct BuildMetrics {
    pub recoil: RecoilResult,
    pub panel: FxHashMap<String, f64>,
    pub attachment_price: i64,
    pub total_price: i64,
    pub missing_price_count: usize,
}

pub struct Evaluator<'a> {
    pub payload: &'a WeaponPayload,
    pub weapon_id: &'a str,
    pub mode: &'a str,
    pub laser_enabled: bool,
    pub bipod_deployed: bool,
    pub prices: &'a FxHashMap<String, i64>,
}

impl<'a> Evaluator<'a> {
    pub fn tuning_key(slot_path: &str, part_id: &str, tune_id: &str) -> String {
        format!("{}\0{}\0{}", slot_path, part_id, tune_id)
    }

    pub fn evaluate(
        &self,
        assembler: &Assembler,
        assembly: &Assembly,
        tuning_state: &TuningState,
    ) -> Option<BuildMetrics> {
        let parts = self.installed_parts(assembly);
        let recoil = self.recoil(assembler, &parts, assembly, tuning_state)?;
        let panel = self.panel(&parts);
        let (attachment_price, total_price, missing) = self.price(assembly);
        Some(BuildMetrics {
            recoil,
            panel,
            attachment_price,
            total_price,
            missing_price_count: missing,
        })
    }

    pub fn installed_parts<'b>(&'b self, assembly: &'b Assembly) -> Vec<&'a Part> {
        let mut out = Vec::new();
        for slot in &assembly.visible_slots {
            if let Some(id) = &slot.selected_part_id {
                if let Some(part) = self.payload.parts.get(id) {
                    out.push(part);
                }
            }
        }
        for item in &assembly.internal_parts {
            if let Some(part) = self.payload.parts.get(&item.part_id) {
                out.push(part);
            }
        }
        out
    }

    pub fn tunable_instances(
        &self,
        assembly: &Assembly,
    ) -> Vec<TuningInstance<'a>> {
        let mut out = Vec::new();
        for slot in &assembly.visible_slots {
            if let Some(id) = &slot.selected_part_id {
                if let Some(part) = self.payload.parts.get(id) {
                    if tuning_available(&slot.slot.name, part) && !part.tuning_options.is_empty() {
                        out.push(TuningInstance {
                            slot_path: slot.path_key.clone(),
                            slot_name: slot.slot.name.clone(),
                            part,
                        });
                    }
                }
            }
        }
        for item in &assembly.internal_parts {
            if let Some(part) = self.payload.parts.get(&item.part_id) {
                if tuning_available(&item.slot.name, part) && !part.tuning_options.is_empty() {
                    out.push(TuningInstance {
                        slot_path: item.slot_path_key.clone(),
                        slot_name: item.slot.name.clone(),
                        part,
                    });
                }
            }
        }
        out.sort_by(|a, b| {
            a.slot_path
                .cmp(&b.slot_path)
                .then(a.part.id.cmp(&b.part.id))
        });
        out
    }

    pub fn resolve_tuning(
        &self,
        assembler: &Assembler,
        instances: &[TuningInstance<'a>],
        tuning_state: &TuningState,
    ) -> TuningState {
        let mut out = TuningState::default();
        for inst in instances {
            for tune in &inst.part.tuning_options {
                let key = Self::tuning_key(&inst.slot_path, &inst.part.id, &tune.tune_id);
                let raw = tuning_state.get(&key).copied().unwrap_or_else(|| {
                    assembler
                        .default_tune(&inst.slot_path, &inst.part.id, &tune.tune_id)
                        .unwrap_or(tune.default_value)
                });
                out.insert(key, quantize(raw, tune.min_value, tune.max_value, tune.step_value));
            }
        }
        out
    }

    pub fn recoil(
        &self,
        assembler: &Assembler,
        parts: &[&Part],
        assembly: &Assembly,
        tuning_state: &TuningState,
    ) -> Option<RecoilResult> {
        let profile = self.payload.numeric_profiles.get(self.weapon_id)?;
        let mode = self.mode;

        let laser_index = laser_pro_index(parts, mode);

        let mut d = 0.0;
        let mut horizontal_a = 0.0;
        let mut vertical_a = 0.0;
        let mut horizontal_c = 1.0;
        let mut vertical_c = 1.0;
        let mut spring_c = 1.0;
        let mut gun_sway_a = 0.0;

        for (idx, part) in parts.iter().enumerate() {
            let laser_part = has_laser(part, mode);
            for m in part.main_mods.get(mode).map(|v| v.as_slice()).unwrap_or(&[]) {
                if !is_addend(m) {
                    continue;
                }
                if !modifier_included(m, laser_part, self.laser_enabled, idx == laser_index) {
                    continue;
                }
                if normalize_attr(m.attribute_id.as_deref()) == "10005" {
                    d += m.delta.map(|v| v.0).unwrap_or(0.0);
                }
            }

            if let Some(rr) = part.raw_recoil.get(mode) {
                if let Some(h) = rr.horizontal {
                    if h.is_finite() {
                        horizontal_a += h;
                    }
                }
                if let Some(v) = rr.vertical {
                    if v.is_finite() {
                        vertical_a += v;
                    }
                }
            }

            let bipod_like = is_bipod_like(part);
            let mut has_explicit_spring_c = false;
            for m in part.raw_mods.get(mode).map(|v| v.as_slice()).unwrap_or(&[]) {
                if m.modifier_type != "Mult_C" {
                    continue;
                }
                let Some(p) = m.param1.map(|v| v.0) else { continue };
                if !p.is_finite() {
                    continue;
                }
                if bipod_like && !self.bipod_deployed && is_recoil_target(&m.target_path) {
                    continue;
                }
                match m.target_path.as_str() {
                    "GRecoil_H" => horizontal_c *= p,
                    "GRecoil_V" => vertical_c *= p,
                    "GGunkickSpring" => {
                        spring_c *= p;
                        has_explicit_spring_c = true;
                    }
                    _ => {}
                }
            }
            if (!bipod_like || self.bipod_deployed) && !has_explicit_spring_c {
                if let Some(k) = part.raw_recoil.get(mode).and_then(|r| r.gunkick_spring_mult_c) {
                    if k.is_finite() {
                        spring_c *= 1.0 + k;
                    }
                }
            }
            if let Some(v) = part.raw_gun_sway.get(mode).copied().flatten() {
                if v.is_finite() {
                    gun_sway_a += v;
                }
            }
        }
        let _ = gun_sway_a;

        let instances = self.tunable_instances(assembly);
        let resolved = self.resolve_tuning(assembler, &instances, tuning_state);
        let (effects, runtime) = self.tuning_effects(&instances, &resolved);

        let t_h = runtime.get("GRecoil_H").copied().unwrap_or(0.0);
        let t_v = runtime.get("GRecoil_V").copied().unwrap_or(0.0);
        let t_k = runtime.get("GGunkickSpring").copied().unwrap_or(0.0);

        let curves = &profile.linked_tables.recoil_curves;
        let control = if self.mode == "warfare" {
            curves.recoil_control_mp.as_ref().or(curves.recoil_control.as_ref())
        } else {
            curves.recoil_control.as_ref()
        }?;
        let gunkick = if self.mode == "warfare" {
            curves.gunkick_mp.as_ref().or(curves.gunkick.as_ref())
        } else {
            curves.gunkick.as_ref()
        }?;

        let h = control.eval(d)?;
        let p = gunkick.eval(d)?;
        let p0 = gunkick.eval(0.0)?;
        if p0 == 0.0 {
            return None;
        }

        let f_h = h * (1.0 + horizontal_a + t_h) * horizontal_c;
        let f_v = h * (1.0 + vertical_a + t_v) * vertical_c;
        let s = p * (1.0 + t_k) * spring_c / p0;

        let base_aim = &profile.linked_tables.aim_recoil;
        let mut aim: &AimRecoil = base_aim;
        for part in parts {
            if let Some(over) = part.aim_recoil_profiles.get(mode) {
                aim = over;
            }
        }

        let ratio = |base: Option<f64>, mult: f64| -> f64 { scale_ratio(base, mult) };
        let cont_h = ratio(
            base_aim.continue_shoot.horizontal_scale,
            f_h * scale_factor(base_aim.continue_shoot.horizontal_scale, aim.continue_shoot.horizontal_scale),
        );
        let cont_v = ratio(
            base_aim.continue_shoot.vertical_scale,
            f_v * scale_factor(base_aim.continue_shoot.vertical_scale, aim.continue_shoot.vertical_scale),
        );
        let single_h = ratio(
            base_aim.single_or_burst.horizontal_scale,
            f_h * scale_factor(base_aim.single_or_burst.horizontal_scale, aim.single_or_burst.horizontal_scale),
        );
        let single_v = ratio(
            base_aim.single_or_burst.vertical_scale,
            f_v * scale_factor(base_aim.single_or_burst.vertical_scale, aim.single_or_burst.vertical_scale),
        );
        let fire_stability = ratio(Some(p0), s);

        Some(RecoilResult {
            d,
            f_h,
            f_v,
            s,
            continuous_h: cont_h,
            continuous_v: cont_v,
            single_h,
            single_v,
            fire_stability,
            tuning_effects: effects,
            tuning_runtime: runtime,
        })
    }

    pub fn tuning_effects(
        &self,
        instances: &[TuningInstance<'a>],
        resolved: &TuningState,
    ) -> (FxHashMap<String, f64>, FxHashMap<String, f64>) {
        let mut effects: FxHashMap<String, f64> = FxHashMap::default();
        let mut runtime: FxHashMap<String, f64> = FxHashMap::default();
        for inst in instances {
            for tune in &inst.part.tuning_options {
                let key = Self::tuning_key(&inst.slot_path, &inst.part.id, &tune.tune_id);
                let value = resolved.get(&key).copied().unwrap_or(tune.default_value);
                for effect in &tune.display_effects {
                    let Some(effect_value) = eval_curve_points(&effect.curve_points, value) else {
                        continue;
                    };
                    *effects
                        .entry(effect.attribute_name_snapshot.clone())
                        .or_insert(0.0) += effect_value;
                    apply_runtime_targets(&mut runtime, tune, &effect.attribute_name_snapshot, effect_value);
                }
            }
        }
        (effects, runtime)
    }

    pub fn panel(&self, parts: &[&Part]) -> FxHashMap<String, f64> {
        let mut out = FxHashMap::default();
        let Some(stats) = self.payload.weapon_stats.get(self.weapon_id) else {
            return out;
        };
        let Some(mode_stats) = stats.by_mode.get(self.mode) else {
            return out;
        };
        for id in ["10005", "10006", "10007", "10008"] {
            let base = mode_stats
                .display
                .get(id)
                .copied()
                .or_else(|| mode_stats.base.get(id).copied())
                .unwrap_or(0.0);
            out.insert(id.to_string(), base);
        }
        let laser_index = laser_pro_index(parts, self.mode);
        for (idx, part) in parts.iter().enumerate() {
            let laser_part = has_laser(part, self.mode);
            for m in part
                .display_mods
                .get(self.mode)
                .map(|v| v.as_slice())
                .unwrap_or(&[])
            {
                if !is_addend(m) {
                    continue;
                }
                if !modifier_included(m, laser_part, self.laser_enabled, idx == laser_index) {
                    continue;
                }
                let attr = normalize_attr(m.attribute_id.as_deref());
                if let Some(v) = out.get_mut(attr) {
                    *v += m.delta.map(|x| x.0).unwrap_or(0.0);
                }
            }
        }
        // 10001 uses Initial / Addend / Mult_A
        if let Some(base) = mode_stats
            .display
            .get("10001")
            .copied()
            .or_else(|| mode_stats.base.get("10001").copied())
        {
            let mut initial: Option<f64> = None;
            let mut addend = 0.0;
            let mut mult_a = 0.0;
            for (idx, part) in parts.iter().enumerate() {
                let laser_part = has_laser(part, self.mode);
                for m in part
                    .display_mods
                    .get(self.mode)
                    .map(|v| v.as_slice())
                    .unwrap_or(&[])
                {
                    if !modifier_included(m, laser_part, self.laser_enabled, idx == laser_index) {
                        continue;
                    }
                    if normalize_attr(m.attribute_id.as_deref()) != "10001" {
                        continue;
                    }
                    match m.modifier_type.as_str() {
                        "Initial" => {
                            if let Some(p) = m.param1 {
                                initial = Some(p.0);
                            }
                        }
                        "Addend" => {
                            if let Some(d) = m.delta {
                                addend += d.0;
                            }
                        }
                        "Mult_A" => {
                            if let Some(p) = m.param1 {
                                mult_a += p.0;
                            }
                        }
                        _ => {}
                    }
                }
            }
            let v = ((initial.unwrap_or(base)) + addend) * (1.0 + mult_a);
            out.insert("10001".to_string(), v.round());
        }
        out
    }

    pub fn price(&self, assembly: &Assembly) -> (i64, i64, usize) {
        let mut attachment = 0i64;
        let mut missing = 0usize;
        for slot in &assembly.visible_slots {
            let Some(id) = &slot.selected_part_id else { continue };
            let Some(part) = self.payload.parts.get(id) else { continue };
            if part.is_virtual {
                continue;
            }
            match self.prices.get(part.price_id()) {
                Some(p) => attachment += *p,
                None => missing += 1,
            }
        }
        for item in &assembly.internal_parts {
            let Some(part) = self.payload.parts.get(&item.part_id) else { continue };
            if part.is_virtual {
                continue;
            }
            match self.prices.get(part.price_id()) {
                Some(p) => attachment += *p,
                None => missing += 1,
            }
        }
        let weapon_price = self
            .payload
            .parts
            .get(self.weapon_id)
            .and_then(|p| self.prices.get(p.price_id()))
            .copied()
            .unwrap_or(0);
        (attachment, attachment + weapon_price, missing)
    }
}

#[derive(Debug, Clone)]
pub struct TuningInstance<'a> {
    pub slot_path: String,
    pub slot_name: String,
    pub part: &'a Part,
}

pub fn tuning_available(slot_name: &str, part: &Part) -> bool {
    part.tuning_socket_availability
        .as_ref()
        .and_then(|m| m.get(slot_name))
        .copied()
        .unwrap_or(slot_name != "StoKit_point")
}

pub fn quantize(value: f64, min: f64, max: f64, step: f64) -> f64 {
    let clamped = value.clamp(min, max);
    if step <= 0.0 {
        return clamped;
    }
    let n = ((clamped - min) / step).round();
    let raw = min + n * step;
    let clamped = raw.clamp(min, max);
    let decimals = decimals(min).max(decimals(max)).max(decimals(step));
    round_to(clamped, decimals)
}

fn decimals(value: f64) -> u32 {
    if !value.is_finite() {
        return 0;
    }
    let mut v = value.abs();
    let mut digits = 0u32;
    while digits < 15 && (v - v.round()).abs() > 1e-9 * v.abs().max(1.0) {
        v *= 10.0;
        digits += 1;
    }
    digits
}

fn round_to(value: f64, decimals: u32) -> f64 {
    let factor = 10f64.powi(decimals.min(15) as i32);
    (value * factor).round() / factor
}

fn normalize_attr(attr: Option<&str>) -> &str {
    match attr {
        Some("100103") => "10001",
        Some(v) => v,
        None => "",
    }
}

fn is_addend(m: &Modifier) -> bool {
    m.condition_id.is_none()
        && m.modifier_type == "Addend"
        && m.delta.map(|v| v.0.is_finite()).unwrap_or(false)
}

fn modifier_included(
    m: &Modifier,
    laser_part: bool,
    laser_enabled: bool,
    is_selected_laser: bool,
) -> bool {
    if !laser_part {
        return true;
    }
    if m.polarity.as_deref() == Some("con") {
        return true;
    }
    laser_enabled && is_selected_laser
}

pub fn has_laser(part: &Part, mode: &str) -> bool {
    part.ui_effects
        .get(mode)
        .map(|list| list.iter().any(|e| condition_is(&e.condition_id, "10038")))
        .unwrap_or(false)
}

pub fn has_bipod_ui(part: &Part, mode: &str) -> bool {
    part.ui_effects
        .get(mode)
        .map(|list| list.iter().any(|e| condition_is(&e.condition_id, "10047")))
        .unwrap_or(false)
}

fn condition_is(id: &Option<FlexibleId>, value: &str) -> bool {
    id.as_ref().map(|v| v.is(value)).unwrap_or(false)
}

fn laser_pro_index(parts: &[&Part], mode: &str) -> usize {
    let mut best = usize::MAX;
    let mut best_score = f64::NEG_INFINITY;
    for (idx, part) in parts.iter().enumerate() {
        if !has_laser(part, mode) {
            continue;
        }
        let score: f64 = part
            .display_mods
            .get(mode)
            .map(|list| {
                list.iter()
                    .filter(|m| is_addend(m) && m.polarity.as_deref() == Some("pro"))
                    .map(|m| m.delta.map(|v| v.0.abs()).unwrap_or(0.0))
                    .sum()
            })
            .unwrap_or(0.0);
        if score > best_score {
            best = idx;
            best_score = score;
        }
    }
    best
}

fn is_recoil_target(target: &str) -> bool {
    target.starts_with("GGunkick")
        || target.starts_with("GRecoil_")
        || target.ends_with("BreathFactor")
}

fn is_bipod_like(part: &Part) -> bool {
    // uiEffects condition 10047 is only meaningful per mode; raw fallback catches virtual bipods.
    let mut has_ui = false;
    for list in part.ui_effects.values() {
        if list.iter().any(|e| condition_is(&e.condition_id, "10047")) {
            has_ui = true;
            break;
        }
    }
    if has_ui || !part.is_virtual {
        return has_ui;
    }
    let mut targets = FxHashSet::default();
    for list in part.raw_mods.values() {
        for m in list {
            if m.modifier_type == "Mult_C" && is_recoil_target(&m.target_path) {
                targets.insert(m.target_path.as_str());
            }
        }
    }
    let has_gunkick = targets.iter().any(|t| t.starts_with("GGunkick"));
    let has_h = targets.contains("GRecoil_H");
    let has_v = targets.contains("GRecoil_V");
    let has_breath = targets.iter().any(|t| t.ends_with("StandBreathFactor"));
    has_gunkick && has_h && has_v && has_breath
}

fn scale_factor(base: Option<f64>, current: Option<f64>) -> f64 {
    match (base, current) {
        (Some(b), Some(c)) => {
            if b == 0.0 {
                if c == 0.0 { 1.0 } else { 0.0 }
            } else {
                c / b
            }
        }
        _ => 1.0,
    }
}

fn round2(value: f64) -> f64 {
    ((value + f64::EPSILON) * 100.0).round() / 100.0
}

fn scale_ratio(base: Option<f64>, multiplier: f64) -> f64 {
    let Some(base) = base else { return multiplier };
    let b = round2(base);
    if b == 0.0 {
        return multiplier;
    }
    let current = round2(b * multiplier);
    round2(current / b)
}

pub fn eval_curve_points(points: &[CurveKey], x: f64) -> Option<f64> {
    if points.is_empty() || !x.is_finite() {
        return None;
    }
    let mut keys: Vec<&CurveKey> = points.iter().collect();
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
        return match a.interp_mode.as_str() {
            "RCIM_Constant" => Some(a.value),
            "RCIM_Cubic" => {
                let dt = b.time - a.time;
                let t2 = t * t;
                let t3 = t2 * t;
                Some(
                    (2.0 * t3 - 3.0 * t2 + 1.0) * a.value
                        + (t3 - 2.0 * t2 + t) * dt * a.leave_tangent
                        + (-2.0 * t3 + 3.0 * t2) * b.value
                        + (t3 - t2) * dt * b.arrive_tangent,
                )
            }
            _ => Some(a.value + (b.value - a.value) * t),
        };
    }
    Some(last.value)
}

fn apply_runtime_targets(
    runtime: &mut FxHashMap<String, f64>,
    tune: &TuningOption,
    name: &str,
    value: f64,
) {
    let (targets, sign): (&[&str], f64) = match name {
        "%额外后坐力控制" => (&["GRecoil_H", "GRecoil_V"], -1.0),
        "%开火时武器稳定性" => (&["GGunkickSpring"], -1.0),
        "%瞄准时移动速度" => (&["GMovement_ADSSpeed"], 1.0),
        _ => return,
    };
    let allowed: Option<FxHashSet<&str>> = if tune.runtime_targets.is_empty() {
        None
    } else {
        Some(
            tune.runtime_targets
                .iter()
                .map(|t| t.runtime_target.as_str())
                .collect(),
        )
    };
    let delta = sign * value / 100.0;
    for target in targets {
        if let Some(allowed) = &allowed {
            if !allowed.contains(target) {
                continue;
            }
        }
        *runtime.entry((*target).to_string()).or_insert(0.0) += delta;
    }
}

pub fn tune_offer(tune: &TuningOption, value: f64) -> (f64, f64, f64) {
    let mut e_h = 0.0;
    let mut e_v = 0.0;
    let mut e_k = 0.0;
    for effect in &tune.display_effects {
        let Some(v) = eval_curve_points(&effect.curve_points, value) else {
            continue;
        };
        match effect.attribute_name_snapshot.as_str() {
            "%额外后坐力控制" => {
                if runtime_target_allowed(tune, "GRecoil_H") {
                    e_h += v;
                }
                if runtime_target_allowed(tune, "GRecoil_V") {
                    e_v += v;
                }
            }
            "%开火时武器稳定性" => {
                if runtime_target_allowed(tune, "GGunkickSpring") {
                    e_k += v;
                }
            }
            _ => {}
        }
    }
    (e_h, e_v, e_k)
}

pub fn tune_offer_values(tune: &TuningOption) -> Vec<f64> {
    let mut values = vec![
        tune.min_value,
        tune.max_value,
        tune.default_value,
    ];
    for effect in &tune.display_effects {
        for key in &effect.curve_points {
            values.push(key.time);
        }
    }
    let step = tune.step_value;
    if step > 0.0 {
        let count = ((tune.max_value - tune.min_value) / step).floor() + 1.0;
        if count <= 128.0 && count > 0.0 {
            let mut v = tune.min_value;
            while v <= tune.max_value + step * 0.5 {
                values.push(v);
                v += step;
            }
        }
    }
    let mut out: Vec<f64> = values
        .into_iter()
        .map(|v| quantize(v, tune.min_value, tune.max_value, tune.step_value))
        .collect();
    out.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    out.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
    out
}

fn runtime_target_allowed(tune: &TuningOption, target: &str) -> bool {
    tune.runtime_targets.is_empty()
        || tune
            .runtime_targets
            .iter()
            .any(|t| t.runtime_target == target)
}

pub fn detail_metric_values(r: &RecoilResult) -> FxHashMap<&'static str, f64> {
    let mut m = FxHashMap::default();
    m.insert("recoil-continuous-vertical", r.continuous_v);
    m.insert("recoil-continuous-horizontal", r.continuous_h);
    m.insert("recoil-single-vertical", r.single_v);
    m.insert("recoil-single-horizontal", r.single_h);
    m.insert("recoil-fire-stability", r.fire_stability);
    m
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::TuningEffect;

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
