use crate::assembly::{Assembler, Assembly};
use crate::model::{
    AimRecoil, CurveKey, FlexibleId, Modifier, NumericProfile, Part, TuningOption, WeaponPayload,
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
    pub detail: DetailMetrics,
    pub attachment_price: i64,
    pub total_price: i64,
    pub missing_price_count: usize,
}

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct DetailMetrics {
    pub handling_score: f64,
    pub stability_score: f64,
    pub hipfire_score: f64,
    pub aim_time: Option<f64>,
    pub sprint_fire_delay: Option<f64>,
    pub aim_move_speed: Option<f64>,
    pub reload_time: Option<f64>,
    pub empty_reload_time: Option<f64>,
    pub holster_time: Option<f64>,
    pub switch_time: Option<f64>,
    pub breath_range: Option<f64>,
    pub breath_duration: Option<f64>,
    pub movement_camera: Option<f64>,
    pub fire_camera: Option<f64>,
    pub hit_sway: Option<f64>,
    pub hold_breath_recoil: Option<f64>,
    pub hipfire_standing: Option<f64>,
    pub hipfire_maximum: Option<f64>,
}

pub const DETAIL_METRICS: [&str; 15] = [
    "handling-aim-time",
    "handling-sprint-fire-delay",
    "handling-aim-move-speed",
    "handling-reload-time",
    "handling-empty-reload-time",
    "handling-holster-time",
    "handling-switch-time",
    "stability-breath-range",
    "stability-breath-duration",
    "stability-movement-camera",
    "stability-fire-camera",
    "stability-hit-sway",
    "stability-hold-breath-recoil",
    "hipfire-standing-spread",
    "hipfire-maximum-spread",
];

impl DetailMetrics {
    pub fn value(&self, id: &str) -> Option<f64> {
        match id {
            "handling-aim-time" => self.aim_time,
            "handling-sprint-fire-delay" => self.sprint_fire_delay,
            "handling-aim-move-speed" => self.aim_move_speed,
            "handling-reload-time" => self.reload_time,
            "handling-empty-reload-time" => self.empty_reload_time,
            "handling-holster-time" => self.holster_time,
            "handling-switch-time" => self.switch_time,
            "stability-breath-range" => self.breath_range,
            "stability-breath-duration" => self.breath_duration,
            "stability-movement-camera" => self.movement_camera,
            "stability-fire-camera" => self.fire_camera,
            "stability-hit-sway" => self.hit_sway,
            "stability-hold-breath-recoil" => self.hold_breath_recoil,
            "hipfire-standing-spread" => self.hipfire_standing,
            "hipfire-maximum-spread" => self.hipfire_maximum,
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct DetailSums {
    pub open_speed: f64,
    pub move_speed: f64,
    pub move_runtime: f64,
    pub breath_stability: f64,
    pub move_stability: f64,
}

pub const HANDLING_ATTRS: [&str; 8] = [
    "10006", "10016", "10040", "10081", "10090", "100109", "100125", "100138",
];
pub const STABILITY_ATTRS: [&str; 12] = [
    "10007", "10044", "10056", "10083", "10086", "10087", "10088", "10089", "100101", "100105",
    "100123", "100124",
];
pub const HIPFIRE_ATTRS: [&str; 7] = [
    "10008", "10011", "100104", "100111", "100112", "100116", "100128",
];

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
        let visible_parts = self.visible_selected_parts(assembly);
        let recoil = self.recoil(assembler, &parts, assembly, tuning_state);
        let (effects, runtime) = match &recoil {
            Some(r) => (r.tuning_effects.clone(), r.tuning_runtime.clone()),
            None => {
                let instances = self.tunable_instances(assembly);
                let resolved = self.resolve_tuning(assembler, &instances, tuning_state);
                self.tuning_effects(&instances, &resolved)
            }
        };
        let detail = self.detail(&parts, &effects, &runtime);
        // The site panel only sums `selectedPart` of visible slots; default and
        // virtual parts that the assembler installs internally are excluded.
        let panel = self.panel(&visible_parts);
        let (attachment_price, total_price, missing) = self.price(assembly);
        Some(BuildMetrics {
            recoil: recoil.unwrap_or_default(),
            panel,
            detail,
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

    /// Parts the site treats as explicitly selected in the UI. Default and
    /// virtual parts installed by the assembler live in `internal_parts` and
    /// are not part of this list even when they belong to a visible slot.
    pub fn visible_selected_parts<'b>(&'b self, assembly: &'b Assembly) -> Vec<&'a Part> {
        let mut out = Vec::new();
        for slot in &assembly.visible_slots {
            if let Some(id) = &slot.selected_part_id {
                if let Some(part) = self.payload.parts.get(id) {
                    out.push(part);
                }
            }
        }
        out
    }

    pub fn tunable_instances(&self, assembly: &Assembly) -> Vec<TuningInstance<'a>> {
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
                out.insert(
                    key,
                    quantize(raw, tune.min_value, tune.max_value, tune.step_value),
                );
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

        for (idx, part) in parts.iter().enumerate() {
            let laser_part = has_laser(part);
            for m in part.main_mods_for(self.weapon_id, mode) {
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
                let Some(p) = m.param1.map(|v| v.0) else {
                    continue;
                };
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
                if let Some(k) = part
                    .raw_recoil
                    .get(mode)
                    .and_then(|r| r.gunkick_spring_mult_c)
                {
                    if k.is_finite() {
                        spring_c *= 1.0 + k;
                    }
                }
            }
        }

        let instances = self.tunable_instances(assembly);
        let resolved = self.resolve_tuning(assembler, &instances, tuning_state);
        let (effects, runtime) = self.tuning_effects(&instances, &resolved);

        let t_h = runtime.get("GRecoil_H").copied().unwrap_or(0.0);
        let t_v = runtime.get("GRecoil_V").copied().unwrap_or(0.0);
        let t_k = runtime.get("GGunkickSpring").copied().unwrap_or(0.0);

        let curves = &profile.linked_tables.recoil_curves;
        let control = if self.mode == "warfare" {
            curves
                .recoil_control_mp
                .as_ref()
                .or(curves.recoil_control.as_ref())
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
        // The site's `Ye` returns null when the base scale is missing (or
        // rounds to 0) and `_r` drops the whole recoil object unless all five
        // entries are non-null, so these inputs must be present.
        let base_scales = [
            base_aim.continue_shoot.horizontal_scale,
            base_aim.continue_shoot.vertical_scale,
            base_aim.single_or_burst.horizontal_scale,
            base_aim.single_or_burst.vertical_scale,
        ];
        if base_scales
            .iter()
            .any(|v| v.map_or(true, |v| !v.is_finite() || round2(v) == 0.0))
            || round2(p0) == 0.0
        {
            return None;
        }
        let mut aim: &AimRecoil = base_aim;
        for part in parts {
            if let Some(over) = part.aim_recoil_profiles.get(mode) {
                aim = over;
            }
        }

        let ratio = |base: Option<f64>, mult: f64| -> f64 { scale_ratio(base, mult) };
        let cont_h = ratio(
            base_aim.continue_shoot.horizontal_scale,
            f_h * scale_factor(
                base_aim.continue_shoot.horizontal_scale,
                aim.continue_shoot.horizontal_scale,
            ),
        );
        let cont_v = ratio(
            base_aim.continue_shoot.vertical_scale,
            f_v * scale_factor(
                base_aim.continue_shoot.vertical_scale,
                aim.continue_shoot.vertical_scale,
            ),
        );
        let single_h = ratio(
            base_aim.single_or_burst.horizontal_scale,
            f_h * scale_factor(
                base_aim.single_or_burst.horizontal_scale,
                aim.single_or_burst.horizontal_scale,
            ),
        );
        let single_v = ratio(
            base_aim.single_or_burst.vertical_scale,
            f_v * scale_factor(
                base_aim.single_or_burst.vertical_scale,
                aim.single_or_burst.vertical_scale,
            ),
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
                    apply_runtime_targets(
                        &mut runtime,
                        tune,
                        &effect.attribute_name_snapshot,
                        effect_value,
                    );
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
        for id in ["10005", "10006", "10007", "10008"] {
            let base = stats.display_stat(self.mode, id).unwrap_or(0.0);
            out.insert(id.to_string(), base);
        }
        let laser_index = laser_pro_index(parts, self.mode);
        for (idx, part) in parts.iter().enumerate() {
            let laser_part = has_laser(part);
            for m in part.display_mods_for(self.weapon_id, self.mode) {
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
        if let Some(base) = stats.display_stat(self.mode, "10001") {
            let mut initial: Option<f64> = None;
            let mut addend = 0.0;
            let mut mult_a = 0.0;
            let mut has_modifier = false;
            for (idx, part) in parts.iter().enumerate() {
                let laser_part = has_laser(part);
                // Addends use the normal-then-baseline list (`kt` on the site),
                // but `Initial` and `Mult_A` read only the part's own mode list.
                for m in part.display_mods_for(self.weapon_id, self.mode) {
                    if !is_addend(m) {
                        continue;
                    }
                    if !modifier_included(m, laser_part, self.laser_enabled, idx == laser_index) {
                        continue;
                    }
                    if normalize_attr(m.attribute_id.as_deref()) != "10001" {
                        continue;
                    }
                    has_modifier = true;
                    if let Some(d) = m.delta {
                        addend += d.0;
                    }
                }
                let own = part
                    .display_mods
                    .get(self.mode)
                    .map(|v| v.as_slice())
                    .unwrap_or(&[]);
                for m in own {
                    // The site's `cr`/`or` read the part's own list instead of
                    // the normal-plus-baseline list, but they still apply the
                    // laser inclusion rule.
                    if m.condition_id.is_some()
                        || !modifier_included(m, laser_part, self.laser_enabled, idx == laser_index)
                    {
                        continue;
                    }
                    if normalize_attr(m.attribute_id.as_deref()) != "10001" {
                        continue;
                    }
                    match m.modifier_type.as_str() {
                        "Initial" => {
                            has_modifier = true;
                            if let Some(p) = m.param1.filter(|p| p.0.is_finite()) {
                                initial = Some(p.0);
                            }
                        }
                        "Mult_A" => {
                            has_modifier = true;
                            if let Some(p) = m.param1.filter(|p| p.0.is_finite()) {
                                mult_a += p.0;
                            }
                        }
                        _ => {}
                    }
                }
            }
            // The site's `lr` only rounds when at least one 10001 modifier is
            // present; otherwise the display value is reported unchanged.
            let v = ((initial.unwrap_or(base)) + addend) * (1.0 + mult_a);
            let v = if has_modifier { js_round(v) } else { base };
            out.insert("10001".to_string(), v);
        }
        out
    }

    pub fn detail(
        &self,
        parts: &[&Part],
        effects: &FxHashMap<String, f64>,
        runtime: &FxHashMap<String, f64>,
    ) -> DetailMetrics {
        let sums = DetailSums {
            open_speed: effect_sum(effects, &["开镜速度", "举镜瞄准速度", "举枪瞄准速度"]),
            move_speed: effect_sum(effects, &["移动速度"]),
            move_runtime: runtime.get("GMovement_ADSSpeed").copied().unwrap_or(0.0),
            breath_stability: effect_sum(effects, &["瞄准时呼吸稳定性"]),
            move_stability: effect_sum(effects, &["移动时据枪稳定性"]),
        };
        self.detail_with(parts, &sums)
    }

    pub fn aim_time(&self, handling_add: f64, ads_time_mult: f64, open_speed: f64) -> Option<f64> {
        let profile = self.payload.numeric_profiles.get(self.weapon_id)?;
        let stats = self.payload.weapon_stats.get(self.weapon_id)?;
        let base_handling = stats.base_stat(self.mode, "10006")?;
        let handling_score = base_handling + handling_add;
        let tuning_scale = clamp_factor(1.0 / (1.0 + open_speed / 100.0));
        let curves = &profile.linked_tables.handling_curves;
        if let Some(curve) = curves.ads_time.as_ref() {
            if let Some(v) = curve.eval(handling_score) {
                return Some(ms_floor(v * (1.0 + ads_time_mult)) * tuning_scale);
            }
        }
        let base = jnum(&profile.linked_tables.aiming, "AimingOnAnimTime").or_else(|| {
            profile
                .detail
                .raw_supporting_values
                .get("handling")
                .and_then(|h| jnum(h, "AimingOnAnimTime"))
        })?;
        Some(ms_floor(
            base * clamp_factor((1.0 - handling_add * 0.015) / (1.0 + open_speed / 100.0)),
        ))
    }

    pub fn aim_inputs(&self, parts: &[&Part]) -> (f64, f64) {
        let mode = self.mode;
        let laser_index = laser_pro_index(parts, mode);
        let mut handling = 0.0;
        let mut ads_time_mult = 0.0;
        let mut seen_raw: FxHashSet<(String, String, String, u64, String)> = FxHashSet::default();
        for (idx, part) in parts.iter().enumerate() {
            let laser_part = has_laser(part);
            let include = |m: &Modifier| {
                modifier_included(m, laser_part, self.laser_enabled, idx == laser_index)
            };
            for m in part.main_mods_for(self.weapon_id, mode) {
                if m.condition_id.is_some() || m.modifier_type != "Addend" || !include(m) {
                    continue;
                }
                if HANDLING_ATTRS.contains(&normalize_attr(m.attribute_id.as_deref())) {
                    handling += m.delta.map(|v| v.0).unwrap_or(0.0);
                }
            }
            for m in part.raw_mods.get(mode).map(|v| v.as_slice()).unwrap_or(&[]) {
                let Some(param) = m.param1.map(|v| v.0) else {
                    continue;
                };
                if !param.is_finite() {
                    continue;
                }
                if !(m.target_path == "GAiming_ADSTime" && m.modifier_type == "Mult_A") {
                    continue;
                }
                let key = (
                    part.id.clone(),
                    m.target_path.clone(),
                    m.modifier_type.clone(),
                    param.to_bits(),
                    m.function_rule_key.clone().unwrap_or_default(),
                );
                if !seen_raw.insert(key) {
                    continue;
                }
                ads_time_mult += param;
            }
        }
        (handling, ads_time_mult)
    }

    pub fn detail_with(&self, parts: &[&Part], sums: &DetailSums) -> DetailMetrics {
        let mut out = DetailMetrics::default();
        let Some(profile) = self.payload.numeric_profiles.get(self.weapon_id) else {
            return out;
        };
        let mode = self.mode;
        let laser_index = laser_pro_index(parts, mode);

        let mut handling = 0.0;
        let mut stability = 0.0;
        let mut hipfire = 0.0;
        let mut gun_sway_a = 0.0;
        let mut breath_c = 1.0;
        let mut ads_time_mult = 0.0;
        let mut move_addend = 0.0;
        let mut seen_raw: FxHashSet<(String, String, String, u64, String)> = FxHashSet::default();
        let mut reload = jnum(&profile.detail.raw_weapon_attribute, "ChangeClipTime");
        let mut empty_reload = jnum(
            &profile.detail.raw_weapon_attribute,
            "ChangeClipTimeWhenEmpty",
        );

        for (idx, part) in parts.iter().enumerate() {
            let laser_part = has_laser(part);
            let include = |m: &Modifier| {
                modifier_included(m, laser_part, self.laser_enabled, idx == laser_index)
            };
            for m in part.main_mods_for(self.weapon_id, mode) {
                if m.condition_id.is_some() || m.modifier_type != "Addend" || !include(m) {
                    continue;
                }
                if HANDLING_ATTRS.contains(&normalize_attr(m.attribute_id.as_deref())) {
                    handling += m.delta.map(|v| v.0).unwrap_or(0.0);
                }
            }
            for m in part.display_mods_for(self.weapon_id, mode) {
                if m.condition_id.is_some() || m.modifier_type != "Addend" || !include(m) {
                    continue;
                }
                let attr = normalize_attr(m.attribute_id.as_deref());
                let delta = m.delta.map(|v| v.0).unwrap_or(0.0);
                if STABILITY_ATTRS.contains(&attr) {
                    stability += delta;
                }
                if HIPFIRE_ATTRS.contains(&attr) {
                    hipfire += delta;
                }
            }
            if let Some(v) = part.raw_gun_sway.get(mode).copied().flatten() {
                if v.is_finite() {
                    gun_sway_a += v;
                }
            }
            let bipod_like = is_bipod_like(part);
            for m in part.raw_mods.get(mode).map(|v| v.as_slice()).unwrap_or(&[]) {
                let Some(param) = m.param1.map(|v| v.0) else {
                    continue;
                };
                if !param.is_finite() {
                    continue;
                }
                if m.modifier_type == "Mult_C"
                    && m.target_path.ends_with(".StandBreathFactor")
                    && !(bipod_like && !self.bipod_deployed)
                {
                    breath_c *= param;
                }
                let counted = m.target_path == "GAiming_ADSTime" && m.modifier_type == "Mult_A"
                    || m.target_path == "GMovement_ADSSpeed" && m.modifier_type == "Addend";
                if !counted {
                    continue;
                }
                let key = (
                    part.id.clone(),
                    m.target_path.clone(),
                    m.modifier_type.clone(),
                    param.to_bits(),
                    m.function_rule_key.clone().unwrap_or_default(),
                );
                if !seen_raw.insert(key) {
                    continue;
                }
                if m.target_path == "GAiming_ADSTime" {
                    ads_time_mult += param;
                } else {
                    move_addend += param;
                }
            }
            for m in part
                .runtime_mods
                .get(mode)
                .map(|v| v.as_slice())
                .unwrap_or(&[])
            {
                let Some(param) = m.param1.map(|v| v.0) else {
                    continue;
                };
                if !param.is_finite() {
                    continue;
                }
                let target = if m.target == "ChangeClipTimewhenEmpty" {
                    "ChangeClipTimeWhenEmpty"
                } else {
                    m.target.as_str()
                };
                match target {
                    "GReload_Time" => {
                        if m.modifier_type == "Mult_A" {
                            reload = reload.map(|v| v * (1.0 + param));
                            empty_reload = empty_reload.map(|v| v * (1.0 + param));
                        }
                    }
                    "ChangeClipTime" => {
                        if m.modifier_type == "Initial" {
                            reload = Some(param);
                        } else if m.modifier_type == "Mult_A" {
                            reload = reload.map(|v| v * (1.0 + param));
                        }
                    }
                    "ChangeClipTimeWhenEmpty" => {
                        if m.modifier_type == "Initial" {
                            empty_reload = Some(param);
                        } else if m.modifier_type == "Mult_A" {
                            empty_reload = empty_reload.map(|v| v * (1.0 + param));
                        }
                    }
                    _ => {}
                }
            }
        }
        out.handling_score = handling;
        out.stability_score = stability;
        out.hipfire_score = hipfire;

        let Some(stats) = self.payload.weapon_stats.get(self.weapon_id) else {
            return out;
        };
        let base_stat = |id: &str| stats.base_stat(mode, id);
        let ops_base_10006 = stats.base_stat("operations", "10006");

        let open_speed = sums.open_speed;
        let move_speed = sums.move_speed;
        let breath_stability = sums.breath_stability;
        let move_stability = sums.move_stability;
        let tuning_scale = clamp_factor(1.0 / (1.0 + open_speed / 100.0));
        let handling_score = base_stat("10006").unwrap_or(0.0) + handling;
        let baseline = ops_base_10006.or_else(|| base_stat("10006")).unwrap_or(0.0);
        let handling_delta = handling_score - baseline;

        let curves = &profile.linked_tables.handling_curves;
        out.aim_time = self.aim_time(handling, ads_time_mult, open_speed);

        let sprint_speed = jnum(&profile.detail.raw_weapon_attribute, "SprintToFireSpeed");
        let sprint_curve = if mode == "warfare" {
            curves
                .sprint_to_fire_mp
                .as_ref()
                .or(curves.sprint_to_fire.as_ref())
        } else {
            curves.sprint_to_fire.as_ref()
        };
        let sprint_ms = sprint_speed.and_then(|base| {
            if base < 0.0 {
                return None;
            }
            let scale = if handling_delta == 0.0 {
                Some(1.0)
            } else {
                sprint_curve.and_then(|c| c.eval(handling_delta))
            }?;
            Some(ms_floor(base * scale) * tuning_scale)
        });
        out.sprint_fire_delay = sprint_ms.or_else(|| {
            sprint_speed.map(|base| {
                ms_floor(base * clamp_factor((1.0 - handling * 0.015) / (1.0 + open_speed / 100.0)))
            })
        });

        let movement = profile.linked_tables.movement_by_mode.get(mode);
        let stand_run = movement.and_then(|m| jnum(m, "StandRunSpeed"));
        let stand_aim = movement.and_then(|m| jnum(m, "StandAimSpeedFactor"));
        let base_aim_speed = match (stand_run, stand_aim) {
            (Some(run), Some(factor)) => Some(run * (1.0 + factor) / 100.0),
            _ => None,
        };
        let strafe_curve = if mode == "warfare" {
            curves
                .ads_strafe_speed_mp
                .as_ref()
                .or(curves.ads_strafe_speed.as_ref())
        } else {
            curves.ads_strafe_speed.as_ref()
        };
        let tune_move = sums.move_runtime;
        out.aim_move_speed = match (base_aim_speed, stand_run, strafe_curve) {
            (Some(base), Some(run), Some(curve)) => curve
                .eval(handling_delta)
                .map(|scale| base + run * scale / 100.0 + run * (move_addend + tune_move) / 100.0),
            _ => None,
        };
        if out.aim_move_speed.is_none() {
            out.aim_move_speed = base_aim_speed
                .map(|base| base * clamp_factor(1.0 + handling * 0.015 + move_speed / 100.0));
        }

        out.reload_time = reload;
        out.empty_reload_time = empty_reload;
        out.holster_time = jnum(&profile.detail.raw_weapon_attribute, "EquipTime");
        out.switch_time = jnum(&profile.detail.raw_weapon_attribute, "UnEquipTime");
        out.hit_sway = jnum(&profile.detail.raw_weapon_attribute, "GunSwayBeHitScale")
            .map(|base| round2(base * (1.0 + gun_sway_a)));

        let hold = profile
            .linked_tables
            .hold_breath
            .as_array()
            .and_then(|list| list.first());
        let hold = hold.unwrap_or(&serde_json::Value::Null);
        let breath_config = hold
            .get("WeaponBreathConfig")
            .unwrap_or(&serde_json::Value::Null);
        let breath_factor = jnum(hold, "StandBreathFactor");
        let atten_speed = jnum(breath_config, "AttenSpeed");
        let movement_camera_scale = profile
            .detail
            .raw_supporting_values
            .get("stability")
            .and_then(|s| jnum(s, "GSMovement_CameraScale"));
        let pitch_amp = jnum(
            &profile.linked_tables.aim_recoil.shake_pitch,
            "WholeAmplitude",
        );
        let yaw_amp = jnum(
            &profile.linked_tables.aim_recoil.shake_yaw,
            "WholeAmplitude",
        );
        let recoil_scale_x = hold.get("RecoilScale").and_then(|v| jnum(v, "X"));
        let recoil_scale_y = hold.get("RecoilScale").and_then(|v| jnum(v, "Y"));
        let stability_curves = &profile.linked_tables.stability_curves;
        let stability_ready = [
            breath_factor,
            atten_speed.filter(|v| *v > 0.0),
            movement_camera_scale,
            pitch_amp,
            yaw_amp,
            recoil_scale_x,
            recoil_scale_y,
        ]
        .iter()
        .all(|v| v.is_some());
        if stability_ready {
            let breath_scale = stability_curves
                .breath_scale
                .as_ref()
                .and_then(|c| c.eval(stability));
            let breath_time = stability_curves
                .breath_time
                .as_ref()
                .and_then(|c| c.eval(stability))
                .filter(|v| *v != 0.0);
            let fire_camera = stability_curves
                .fire_camera_stability
                .as_ref()
                .and_then(|c| c.eval(stability));
            let movement_camera = stability_curves
                .movement_camera_stability
                .as_ref()
                .and_then(|c| c.eval(stability));
            let hold_recoil = stability_curves
                .hold_breath_recoil
                .as_ref()
                .and_then(|c| c.eval(stability));
            if let (
                Some(breath_scale),
                Some(breath_time),
                Some(fire_camera),
                Some(movement_camera),
                Some(hold_recoil),
            ) = (
                breath_scale,
                breath_time,
                fire_camera,
                movement_camera,
                hold_recoil,
            ) {
                let breath_inv = 1.0 / (1.0 + breath_stability / 100.0).max(0.1);
                let move_inv = 1.0 / (1.0 + move_stability / 100.0).max(0.1);
                let raw_range = breath_factor.unwrap() * breath_c * 0.4 * breath_scale * breath_inv;
                let raw_duration =
                    100.0 / (atten_speed.unwrap() * breath_time) * (1.0 / breath_inv);
                let movement_ratio = movement_camera * move_inv;
                out.breath_range = Some(raw_range);
                out.breath_duration = Some(raw_duration);
                out.movement_camera = Some(round2(movement_ratio));
                out.fire_camera = Some(round2(fire_camera));
                out.hold_breath_recoil = Some(hold_recoil);
            }
        }

        let waist = &profile.linked_tables.waist_spread;
        let hip_curve = curves.hip_spread.as_ref();
        let hip_multiplier = hip_curve
            .and_then(|c| c.eval(hipfire))
            .or(if hipfire == 0.0 { Some(1.0) } else { None });
        if let (Some(waist), Some(multiplier)) = (waist.as_ref(), hip_multiplier) {
            let area = waist.area_size.first().map(|v| v.0);
            let spread_max = waist.spread_max.x.map(|v| v.0);
            let factor = waist.lua_display_spread_factor.map(|v| v.0);
            let ui_factor = waist.lua_display_spread_ui_factor.map(|v| v.0);
            if let (Some(area), Some(spread_max), Some(_), Some(_)) =
                (area, spread_max, factor, ui_factor)
            {
                out.hipfire_standing = Some(round2(area) * multiplier);
                out.hipfire_maximum = Some(round2(spread_max) * multiplier);
            }
        }
        out
    }

    pub fn price(&self, assembly: &Assembly) -> (i64, i64, usize) {
        let mut attachment = 0i64;
        let mut missing = 0usize;
        // The site's ordinary price helper (`ma`) maps a non-positive or
        // non-finite value to null, while blackhawk prices treat any present
        // value (including 0) as the real price.
        let real_price = |p: i64| self.mode == "blackhawk" || p > 0;
        for slot in &assembly.visible_slots {
            let Some(id) = &slot.selected_part_id else {
                continue;
            };
            let Some(part) = self.payload.parts.get(id) else {
                continue;
            };
            if part.is_virtual {
                continue;
            }
            match self.prices.get(part.price_id()) {
                Some(p) if real_price(*p) => attachment += *p,
                _ => missing += 1,
            }
        }
        // The site's ordinary price walks `installedParts` (visible selected
        // parts only), while blackhawk prices come from `sn()` which also
        // includes the non-virtual internal parts.
        if self.mode == "blackhawk" {
            for item in &assembly.internal_parts {
                let Some(part) = self.payload.parts.get(&item.part_id) else {
                    continue;
                };
                if part.is_virtual {
                    continue;
                }
                match self.prices.get(part.price_id()) {
                    Some(p) if real_price(*p) => attachment += *p,
                    _ => missing += 1,
                }
            }
        }
        let weapon_price = self
            .payload
            .parts
            .get(self.weapon_id)
            .and_then(|p| self.prices.get(p.price_id()))
            .copied()
            .filter(|p| real_price(*p));
        // The site seeds `missing` with 1 when the weapon itself has no price
        // (ordinary `Ri` and blackhawk `sn()` behave the same way).
        if weapon_price.is_none() {
            missing += 1;
        }
        let weapon_price = weapon_price.unwrap_or(0);
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

/// The site's `Ce` scans every mode's `uiEffectsByMode`, so a part that is a
/// laser in any mode is treated as one for the current evaluation.
pub fn has_laser(part: &Part) -> bool {
    part.ui_effects
        .values()
        .any(|list| list.iter().any(|e| condition_is(&e.condition_id, "10038")))
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

/// Selection weight of a laser part: the summed magnitude of its
/// unconditioned `pro` display addends. The evaluator applies the laser
/// modifiers of exactly one laser part, the one with the highest score, so
/// every consumer of the laser rule has to rank parts the same way.
pub fn laser_pro_score(part: &Part, mode: &str) -> f64 {
    part.display_mods
        .get(mode)
        .map(|list| {
            list.iter()
                .filter(|m| is_addend(m) && m.polarity.as_deref() == Some("pro"))
                .map(|m| m.delta.map(|v| v.0.abs()).unwrap_or(0.0))
                .sum()
        })
        .unwrap_or(0.0)
}

fn laser_pro_index(parts: &[&Part], mode: &str) -> usize {
    let mut best = usize::MAX;
    let mut best_score = f64::NEG_INFINITY;
    for (idx, part) in parts.iter().enumerate() {
        if !has_laser(part) {
            continue;
        }
        let score = laser_pro_score(part, mode);
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

pub(crate) fn scale_factor(base: Option<f64>, current: Option<f64>) -> f64 {
    match (base, current) {
        (Some(b), Some(c)) => {
            if b == 0.0 {
                if c == 0.0 {
                    1.0
                } else {
                    0.0
                }
            } else {
                c / b
            }
        }
        _ => 1.0,
    }
}

/// JavaScript `Math.round`: half values round toward +infinity, so
/// `js_round(-0.5)` is `-0` while `f64::round(-0.5)` is `-1`.
fn js_round(value: f64) -> f64 {
    if !value.is_finite() {
        return value;
    }
    let rounded = (value + 0.5).floor();
    if rounded == 0.0 {
        if value.is_sign_negative() {
            -0.0
        } else {
            0.0
        }
    } else {
        rounded
    }
}

fn round2(value: f64) -> f64 {
    js_round((value + f64::EPSILON) * 100.0) / 100.0
}

/// Value of a stability detail metric when every non-`dS` input is neutral:
/// breath scale multiplier 1, breath stability divisor 1 and movement
/// stability divisor 1. Mirrors the corresponding branches of `detail_with`
/// so callers may invert the metric against the stability score.
///
/// Returns `None` when the profile lacks any value that `detail_with` requires
/// before it emits stability metrics at all (all five curves must evaluate).
pub fn stability_metric_at(profile: &NumericProfile, id: &str, stability: f64) -> Option<f64> {
    let hold = profile.linked_tables.hold_breath.as_array()?.first()?;
    let breath_config = hold.get("WeaponBreathConfig")?;
    let breath_factor = jnum(hold, "StandBreathFactor")?;
    let atten_speed = jnum(breath_config, "AttenSpeed").filter(|v| *v > 0.0)?;
    // The raw camera-stability value multiplies by GSWovement_CameraScale, but
    // the reported metric (`display.movementCameraStability` on the site) does
    // not. Require the scale for parity with `detail_with`, then ignore it.
    jnum(
        profile.detail.raw_supporting_values.get("stability")?,
        "GSMovement_CameraScale",
    )?;
    jnum(
        &profile.linked_tables.aim_recoil.shake_pitch,
        "WholeAmplitude",
    )?;
    jnum(
        &profile.linked_tables.aim_recoil.shake_yaw,
        "WholeAmplitude",
    )?;
    hold.get("RecoilScale").and_then(|v| jnum(v, "X"))?;
    hold.get("RecoilScale").and_then(|v| jnum(v, "Y"))?;

    let curves = &profile.linked_tables.stability_curves;
    let breath_scale = curves.breath_scale.as_ref()?.eval(stability)?;
    let breath_time = curves
        .breath_time
        .as_ref()?
        .eval(stability)
        .filter(|v| *v != 0.0)?;
    let fire_camera = curves.fire_camera_stability.as_ref()?.eval(stability)?;
    let movement_camera = curves.movement_camera_stability.as_ref()?.eval(stability)?;
    let hold_recoil = curves.hold_breath_recoil.as_ref()?.eval(stability)?;

    match id {
        "stability-fire-camera" => Some(round2(fire_camera)),
        "stability-movement-camera" => Some(round2(movement_camera)),
        "stability-hold-breath-recoil" => Some(hold_recoil),
        "stability-breath-range" => Some(breath_factor * 0.4 * breath_scale),
        "stability-breath-duration" => Some(100.0 / (atten_speed * breath_time)),
        _ => None,
    }
}

fn fround(value: f64) -> f64 {
    value as f32 as f64
}

fn ms_floor(value: f64) -> f64 {
    (fround(value) * 1000.0).floor()
}

fn clamp_factor(value: f64) -> f64 {
    value.clamp(0.25, 2.5)
}

fn jnum(value: &serde_json::Value, key: &str) -> Option<f64> {
    let v = value.get(key)?;
    match v {
        serde_json::Value::Number(n) => n.as_f64(),
        serde_json::Value::String(s) => s.trim().parse::<f64>().ok(),
        _ => None,
    }
}

fn effect_sum(effects: &FxHashMap<String, f64>, needles: &[&str]) -> f64 {
    effects
        .iter()
        .filter(|(name, _)| needles.iter().any(|needle| name.contains(needle)))
        .map(|(_, value)| *value)
        .sum()
}

pub(crate) fn scale_ratio(base: Option<f64>, multiplier: f64) -> f64 {
    let Some(base) = base else { return multiplier };
    let b = round2(base);
    if b == 0.0 {
        return multiplier;
    }
    let current = round2(b * multiplier);
    round2(current / b)
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

    fn price_payload() -> WeaponPayload {
        serde_json::from_value(serde_json::json!({
            "parts": {
                "w1": { "id": "w1", "name": "Gun" },
                "p1": { "id": "p1", "name": "P1" }
            }
        }))
        .unwrap()
    }

    fn price_assembly(part_id: &str) -> Assembly {
        Assembly {
            visible_slots: vec![crate::assembly::VisibleSlot {
                path_key: "weapon:w1/slot:1:Muzzle_point".to_string(),
                parent_slot_path_key: None,
                host_part_id: "w1".to_string(),
                depth: 1,
                slot: crate::assembly::SlotView {
                    id: "1".to_string(),
                    name: "Muzzle_point".to_string(),
                    virtual_candidate_ids: Vec::new(),
                },
                candidate_ids: vec![part_id.to_string()],
                selected_part_id: Some(part_id.to_string()),
                internal_part_id: None,
                internal_is_default: false,
            }],
            ..Default::default()
        }
    }

    #[test]
    fn ordinary_price_treats_non_positive_as_missing() {
        let payload = price_payload();
        let check = |part_price: i64| {
            let mut prices: FxHashMap<String, i64> = FxHashMap::default();
            prices.insert("w1".to_string(), 1000);
            prices.insert("p1".to_string(), part_price);
            let ev = Evaluator {
                payload: &payload,
                weapon_id: "w1",
                mode: "operations",
                laser_enabled: false,
                bipod_deployed: false,
                prices: &prices,
            };
            ev.price(&price_assembly("p1"))
        };
        assert_eq!(check(0), (0, 1000, 1), "zero must count as missing");
        assert_eq!(check(-5), (0, 1000, 1), "negative must count as missing");
        assert_eq!(check(250), (250, 1250, 0));
    }

    #[test]
    fn blackhawk_price_keeps_zero_as_real() {
        let payload = price_payload();
        let mut prices: FxHashMap<String, i64> = FxHashMap::default();
        prices.insert("w1".to_string(), 1000);
        prices.insert("p1".to_string(), 0);
        let ev = Evaluator {
            payload: &payload,
            weapon_id: "w1",
            mode: "blackhawk",
            laser_enabled: false,
            bipod_deployed: false,
            prices: &prices,
        };
        assert_eq!(ev.price(&price_assembly("p1")), (0, 1000, 0));
    }

    fn recoil_payload_json(aim_recoil: serde_json::Value) -> serde_json::Value {
        serde_json::json!({
            "parts": {
                "w1": { "id": "w1", "name": "Gun" }
            },
            "weaponNumericProfilesById": {
                "w1": {
                    "linkedTables": {
                        "aimRecoil": aim_recoil,
                        "recoilCurves": {
                            "recoilControl": {
                                "keys": [
                                    { "Time": 0, "Value": 1.0 },
                                    { "Time": 100, "Value": 0.5 }
                                ]
                            },
                            "gunkick": {
                                "keys": [
                                    { "Time": 0, "Value": 1.0 },
                                    { "Time": 100, "Value": 0.5 }
                                ]
                            }
                        }
                    }
                }
            }
        })
    }

    fn recoil_payload(aim_recoil: serde_json::Value) -> WeaponPayload {
        serde_json::from_value(recoil_payload_json(aim_recoil)).unwrap()
    }

    fn recoil_of(aim_recoil: serde_json::Value) -> Option<RecoilResult> {
        let payload = recoil_payload(aim_recoil);
        let assembler = Assembler::new(&payload, "w1");
        let prices: FxHashMap<String, i64> = FxHashMap::default();
        let ev = Evaluator {
            payload: &payload,
            weapon_id: "w1",
            mode: "operations",
            laser_enabled: false,
            bipod_deployed: false,
            prices: &prices,
        };
        ev.recoil(
            &assembler,
            &[],
            &Assembly::default(),
            &TuningState::default(),
        )
    }

    #[test]
    fn missing_aim_scale_makes_recoil_unavailable() {
        // The site's `Ye` returns null when the base scale is missing and
        // `_r` drops the whole recoil object unless all five entries are
        // non-null.
        assert!(recoil_of(serde_json::json!({})).is_none());
        assert!(recoil_of(serde_json::json!({
            "ContinueShootRecoil": { "HorizontalScale": 1.0, "VerticalScale": 1.0 },
            "SingleOrBurstShootRecoil": { "HorizontalScale": 1.0 }
        }))
        .is_none());
        // `Ye` also returns null when round2(base) is zero.
        assert!(recoil_of(serde_json::json!({
            "ContinueShootRecoil": { "HorizontalScale": 0.001, "VerticalScale": 1.0 },
            "SingleOrBurstShootRecoil": { "HorizontalScale": 1.0, "VerticalScale": 1.0 }
        }))
        .is_none());
    }

    #[test]
    fn complete_aim_scale_still_yields_recoil() {
        let r = recoil_of(serde_json::json!({
            "ContinueShootRecoil": { "HorizontalScale": 1.0, "VerticalScale": 2.0 },
            "SingleOrBurstShootRecoil": { "HorizontalScale": 4.0, "VerticalScale": 1.0 }
        }))
        .expect("all scales present");
        assert!((r.continuous_h - 1.0).abs() < 1e-9);
        assert!((r.continuous_v - 1.0).abs() < 1e-9);
        assert!((r.single_h - 1.0).abs() < 1e-9);
        assert!((r.single_v - 1.0).abs() < 1e-9);
        assert!((r.fire_stability - 1.0).abs() < 1e-9);
    }

    #[test]
    fn recoil_constraint_is_unavailable_without_aim_scale() {
        let payload = recoil_payload_json(serde_json::json!({}));
        let request = serde_json::json!({
            "weaponId": "w1",
            "mode": "operations",
            "prices": { "w1": 100 },
            "constraints": [
                { "id": "recoil-continuous-horizontal", "max": 0.85 }
            ]
        });
        let out = crate::solve_slices(
            &serde_json::to_vec(&payload).unwrap(),
            &serde_json::to_vec(&request).unwrap(),
        )
        .unwrap();
        let out: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(out["optimal"], false, "no recoil value can satisfy the bound");
    }

    #[test]
    fn round2_uses_js_math_round_semantics() {
        // Math.round rounds .5 toward +infinity, unlike f64::round.
        assert_eq!(round2(-0.2550000000000002), -0.25);
        assert_eq!(round2(-0.26500000000000024), -0.26);
        assert_eq!(round2(-0.2950000000000002), -0.29);
        assert_eq!(round2(0.2550000000000002), 0.26);
        let negative_zero = round2(-0.005);
        assert_eq!(negative_zero, 0.0);
        assert!(negative_zero.is_sign_negative());
    }

    #[test]
    fn hit_sway_rounds_like_js_math_round() {
        let payload: WeaponPayload = serde_json::from_value(serde_json::json!({
            "parts": { "w1": { "id": "w1", "name": "Gun" } },
            "weaponStatsById": {
                "w1": {
                    "byMode": {
                        "operations": {
                            "base": { "10006": 50 },
                            "display": { "10006": 50 }
                        }
                    }
                }
            },
            "weaponNumericProfilesById": {
                "w1": {
                    "detail": {
                        "raw_weapon_attribute": {
                            "GunSwayBeHitScale": -0.2550000000000002
                        }
                    }
                }
            }
        }))
        .unwrap();
        let prices: FxHashMap<String, i64> = FxHashMap::default();
        let ev = Evaluator {
            payload: &payload,
            weapon_id: "w1",
            mode: "operations",
            laser_enabled: false,
            bipod_deployed: false,
            prices: &prices,
        };
        let detail = ev.detail_with(&[], &DetailSums::default());
        assert_eq!(detail.hit_sway, Some(-0.25));
    }

    #[test]
    fn unknown_interp_mode_matches_site_cubic_recoil() {
        for mode in [Some("RCIM_Bogus"), None] {
            let key_mode = mode.unwrap_or("");
            let payload: WeaponPayload = serde_json::from_value(serde_json::json!({
                "parts": {
                    "w1": { "id": "w1", "name": "Gun" },
                    "p1": {
                        "id": "p1",
                        "name": "P1",
                        "mainAttributeModifiersByMode": {
                            "operations": [
                                { "attributeId": "10005", "modifierType": "Addend", "delta": 2.5 }
                            ]
                        }
                    }
                },
                "weaponNumericProfilesById": {
                    "w1": {
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
                            },
                            "recoilCurves": {
                                "recoilControl": {
                                    "keys": [
                                        { "Time": 0, "Value": 10, "InterpMode": key_mode },
                                        { "Time": 10, "Value": 30 }
                                    ]
                                },
                                "gunkick": {
                                    "keys": [
                                        { "Time": 0, "Value": 1 },
                                        { "Time": 10, "Value": 1 }
                                    ]
                                }
                            }
                        }
                    }
                }
            }))
            .unwrap();
            let assembler = Assembler::new(&payload, "w1");
            let prices: FxHashMap<String, i64> = FxHashMap::default();
            let ev = Evaluator {
                payload: &payload,
                weapon_id: "w1",
                mode: "operations",
                laser_enabled: false,
                bipod_deployed: false,
                prices: &prices,
            };
            let parts: Vec<&Part> = vec![payload.parts.get("p1").unwrap()];
            let r = ev
                .recoil(
                    &assembler,
                    &parts,
                    &price_assembly("p1"),
                    &TuningState::default(),
                )
                .expect("recoil");
            assert_eq!(r.continuous_h, 13.13, "mode {mode:?}");
            assert_eq!(r.continuous_v, 13.13, "mode {mode:?}");
        }
    }

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

    fn panel_10001(display: f64, initial: Option<f64>) -> Option<f64> {
        let mut parts = serde_json::json!({
            "w1": { "id": "w1", "name": "Gun" }
        });
        if let Some(initial) = initial {
            parts["p1"] = serde_json::json!({
                "id": "p1",
                "name": "P1",
                "modifiersByMode": {
                    "operations": [
                        {
                            "attributeId": "10001",
                            "modifierType": "Initial",
                            "param1": initial
                        }
                    ]
                }
            });
        }
        let payload: WeaponPayload = serde_json::from_value(serde_json::json!({
            "parts": parts,
            "weaponStatsById": {
                "w1": {
                    "byMode": {
                        "operations": {
                            "base": { "10001": 50 },
                            "display": { "10001": display }
                        }
                    }
                }
            }
        }))
        .unwrap();
        let prices: FxHashMap<String, i64> = FxHashMap::default();
        let ev = Evaluator {
            payload: &payload,
            weapon_id: "w1",
            mode: "operations",
            laser_enabled: false,
            bipod_deployed: false,
            prices: &prices,
        };
        let selected: Vec<&Part> = initial
            .and_then(|_| payload.parts.get("p1"))
            .into_iter()
            .collect();
        ev.panel(&selected).get("10001").copied()
    }

    #[test]
    fn panel_10001_rounds_only_with_a_main_attribute_modifier() {
        // The site only calls `lr` (Math.round) when a 10001 addend/Initial/
        // Mult_A modifier exists; otherwise the display value is kept as-is.
        assert_eq!(panel_10001(50.25, None), Some(50.25));
        // A negative half value follows Math.round, i.e. -0.
        let zero = panel_10001(1.0, Some(-0.5)).unwrap();
        assert_eq!(zero, 0.0);
        assert!(zero.is_sign_negative());
        assert_eq!(panel_10001(1.0, Some(-1.5)), Some(-1.0));
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
