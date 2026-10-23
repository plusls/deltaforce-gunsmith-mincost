use crate::assembly::{Assembler, Assembly};
use crate::model::{AimRecoil, Part, TuningOption};
use rustc_hash::{FxHashMap, FxHashSet};

use super::js_math::{round2, scale_factor, scale_ratio};
use super::part_rules::{
    has_laser, is_addend, is_bipod_like, is_recoil_target, laser_pro_index, modifier_included,
    normalize_attr,
};
use super::tuning::eval_curve_points;
use super::{Evaluator, RecoilResult, TuningInstance, TuningState};

impl<'a> Evaluator<'a> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assembly::Assembler;
    use crate::metrics::test_support::price_assembly;
    use crate::model::WeaponPayload;
    use rustc_hash::FxHashMap;

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
}
