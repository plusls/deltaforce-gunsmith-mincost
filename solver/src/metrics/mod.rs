mod detail;
mod js_math;
mod panel;
mod part_rules;
mod price;
mod recoil;
mod tuning;

use crate::assembly::{Assembler, Assembly};
use crate::model::{Part, WeaponPayload};
use rustc_hash::FxHashMap;

pub use detail::stability_metric_at;
pub(crate) use js_math::{scale_factor, scale_ratio};
pub use part_rules::{has_bipod_ui, has_laser, laser_pro_score};
pub use tuning::{
    eval_curve_points, quantize, tune_channels, tune_offer, tune_offer_values, tuning_available,
};

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
}

#[derive(Debug, Clone)]
pub struct TuningInstance<'a> {
    pub slot_path: String,
    pub slot_name: String,
    pub part: &'a Part,
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
pub(crate) mod test_support {
    use super::*;

    pub(crate) fn price_payload() -> WeaponPayload {
        serde_json::from_value(serde_json::json!({
            "parts": {
                "w1": { "id": "w1", "name": "Gun" },
                "p1": { "id": "p1", "name": "P1" }
            }
        }))
        .unwrap()
    }

    pub(crate) fn price_assembly(part_id: &str) -> Assembly {
        Assembly {
            visible_slots: vec![crate::assembly::VisibleSlot {
                path_key: "weapon:w1/slot:1:Muzzle_point".to_string(),
                parent_slot_path_key: None,
                host_part_id: "w1".to_string(),
                depth: 1,
                slot: std::rc::Rc::new(crate::assembly::SlotView {
                    id: "1".to_string(),
                    name: "Muzzle_point".to_string(),
                    virtual_candidate_ids: Vec::new(),
                }),
                candidate_ids: std::rc::Rc::new(vec![part_id.to_string()]),
                selected_part_id: Some(part_id.to_string()),
                internal_part_id: None,
                internal_is_default: false,
            }],
            ..Default::default()
        }
    }
}
