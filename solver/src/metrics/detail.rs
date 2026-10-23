use crate::model::{Modifier, NumericProfile, Part};
use rustc_hash::{FxHashMap, FxHashSet};

use super::js_math::{clamp_factor, ms_floor, round2};
use super::part_rules::{
    has_laser, is_bipod_like, laser_pro_index, modifier_included, normalize_attr,
};
use super::{DetailMetrics, DetailSums, Evaluator, HANDLING_ATTRS, HIPFIRE_ATTRS, STABILITY_ATTRS};

impl<'a> Evaluator<'a> {
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

    /// Sprint-to-fire delay for a handling addend and an open speed effect.
    /// The DP tracks both inputs, so it can evaluate the same formula for
    /// partial builds and prune on it.
    pub fn sprint_fire_delay(&self, handling_add: f64, open_speed: f64) -> Option<f64> {
        let profile = self.payload.numeric_profiles.get(self.weapon_id)?;
        let stats = self.payload.weapon_stats.get(self.weapon_id)?;
        let mode = self.mode;
        let base_stat = |id: &str| stats.base_stat(mode, id);
        let ops_base_10006 = stats.base_stat("operations", "10006");
        let tuning_scale = clamp_factor(1.0 / (1.0 + open_speed / 100.0));
        let handling_score = base_stat("10006").unwrap_or(0.0) + handling_add;
        let baseline = ops_base_10006.or_else(|| base_stat("10006")).unwrap_or(0.0);
        let handling_delta = handling_score - baseline;
        let sprint_speed = jnum(&profile.detail.raw_weapon_attribute, "SprintToFireSpeed");
        let curves = &profile.linked_tables.handling_curves;
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
        sprint_ms.or_else(|| {
            sprint_speed.map(|base| {
                ms_floor(
                    base * clamp_factor((1.0 - handling_add * 0.015) / (1.0 + open_speed / 100.0)),
                )
            })
        })
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
        let handling_score = base_stat("10006").unwrap_or(0.0) + handling;
        let baseline = ops_base_10006.or_else(|| base_stat("10006")).unwrap_or(0.0);
        let handling_delta = handling_score - baseline;

        let curves = &profile.linked_tables.handling_curves;
        out.aim_time = self.aim_time(handling, ads_time_mult, open_speed);

        out.sprint_fire_delay = self.sprint_fire_delay(handling, open_speed);

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::WeaponPayload;
    use rustc_hash::FxHashMap;

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
}
