use rustc_hash::FxHashMap;
use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use std::fmt;

pub type Map<K, V> = FxHashMap<K, V>;

#[derive(Debug, Clone, Deserialize)]
pub struct WeaponPayload {
    #[serde(rename = "dataVersion", default)]
    pub data_version: String,
    #[serde(default)]
    pub parts: Map<String, Part>,
    #[serde(rename = "slotsByHost", default)]
    pub slots_by_host: Map<String, Vec<SlotDef>>,
    #[serde(rename = "defaultPresetsByWeapon", default)]
    pub default_presets: Map<String, Preset>,
    #[serde(rename = "virtualRulesByWeapon", default)]
    pub virtual_rules: Map<String, Vec<VirtualRule>>,
    #[serde(rename = "candidateOverridesByWeapon", default)]
    pub candidate_overrides: Map<String, Vec<CandidateOverride>>,
    #[serde(rename = "weaponStatsById", default)]
    pub weapon_stats: Map<String, WeaponStats>,
    #[serde(rename = "weaponNumericProfilesById", default)]
    pub numeric_profiles: Map<String, NumericProfile>,
    #[serde(rename = "weaponId", default)]
    pub weapon_id: Option<String>,
    #[serde(rename = "weaponIds", default)]
    pub weapon_ids: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Part {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(rename = "officialObjectId", default)]
    pub official_object_id: Option<String>,
    #[serde(rename = "isVirtual", default)]
    pub is_virtual: bool,
    #[serde(default)]
    pub hidden: Option<bool>,
    #[serde(rename = "mainAttributeModifiersByMode", default)]
    pub main_mods: Map<String, Vec<Modifier>>,
    #[serde(rename = "modifiersByMode", default)]
    pub display_mods: Map<String, Vec<Modifier>>,
    #[serde(rename = "baselineRecoveryModifiersByWeaponMode", default)]
    pub display_baseline_mods: Map<String, Map<String, Vec<Modifier>>>,
    #[serde(rename = "mainAttributeBaselineRecoveryModifiersByWeaponMode", default)]
    pub main_baseline_mods: Map<String, Map<String, Vec<Modifier>>>,
    #[serde(rename = "rawModifiersByMode", default)]
    pub raw_mods: Map<String, Vec<Modifier>>,
    #[serde(rename = "rawRecoilMultipliersByMode", default)]
    pub raw_recoil: Map<String, RawRecoil>,
    #[serde(rename = "rawGunSwayBeHitScaleMultAByMode", default)]
    pub raw_gun_sway: Map<String, Option<f64>>,
    #[serde(rename = "runtimeModifiersByMode", default)]
    pub runtime_mods: Map<String, Vec<RuntimeModifier>>,
    #[serde(rename = "tuningOptions", default)]
    pub tuning_options: Vec<TuningOption>,
    #[serde(rename = "tuningSocketAvailability", default)]
    pub tuning_socket_availability: Option<Map<String, bool>>,
    #[serde(rename = "extraOccupySocketIds", default)]
    pub extra_occupy_socket_ids: Vec<String>,
    #[serde(rename = "aimRecoilProfilesByMode", default)]
    pub aim_recoil_profiles: Map<String, AimRecoil>,
    #[serde(rename = "uiEffectsByMode", default)]
    pub ui_effects: Map<String, Vec<UiEffect>>,
    #[serde(rename = "forbidBindings", default)]
    pub forbid_bindings: Vec<ForbidBinding>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Modifier {
    #[serde(rename = "attributeId", default)]
    pub attribute_id: Option<String>,
    #[serde(rename = "conditionId", default)]
    pub condition_id: Option<FlexibleId>,
    #[serde(default)]
    pub delta: Option<Num>,
    #[serde(rename = "modifierType", default)]
    pub modifier_type: String,
    #[serde(default)]
    pub param1: Option<Num>,
    #[serde(default)]
    pub polarity: Option<String>,
    #[serde(rename = "targetPath", default)]
    pub target_path: String,
    #[serde(default)]
    pub name: String,
    #[serde(rename = "functionRuleKey", default)]
    pub function_rule_key: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct RawRecoil {
    #[serde(default)]
    pub horizontal: Option<f64>,
    #[serde(default)]
    pub vertical: Option<f64>,
    #[serde(rename = "gunkickSpringMultC", default)]
    pub gunkick_spring_mult_c: Option<f64>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct RuntimeModifier {
    #[serde(default)]
    pub target: String,
    #[serde(rename = "modifierType", default)]
    pub modifier_type: String,
    #[serde(default)]
    pub param1: Option<Num>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct UiEffect {
    #[serde(rename = "attributeId", default)]
    pub attribute_id: Option<String>,
    #[serde(rename = "conditionId", default)]
    pub condition_id: Option<FlexibleId>,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub polarity: Option<String>,
    #[serde(default)]
    pub value: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct ForbidBinding {
    #[serde(rename = "weaponId", default)]
    pub weapon_id: String,
    #[serde(rename = "requiresFoldedState", default)]
    pub requires_folded_state: bool,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct TuningOption {
    #[serde(rename = "tune_id", default)]
    pub tune_id: String,
    #[serde(default)]
    pub tune_index: i64,
    #[serde(default)]
    pub min_value: f64,
    #[serde(default)]
    pub max_value: f64,
    #[serde(default)]
    pub step_value: f64,
    #[serde(default)]
    pub default_value: f64,
    #[serde(default)]
    pub display_effects: Vec<TuningEffect>,
    #[serde(default)]
    pub runtime_targets: Vec<RuntimeTarget>,
    #[serde(default)]
    pub slider_type: String,
    #[serde(default)]
    pub labels: Value,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct TuningEffect {
    #[serde(default)]
    pub attribute_name_snapshot: String,
    #[serde(default)]
    pub curve_points: Vec<CurveKey>,
    #[serde(default)]
    pub effect_index: i64,
    #[serde(default)]
    pub is_debuff: bool,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct RuntimeTarget {
    #[serde(default)]
    pub effect_index: i64,
    #[serde(default)]
    pub modifier_type: String,
    #[serde(default)]
    pub runtime_target: String,
    #[serde(default)]
    pub target_kind: String,
    #[serde(default)]
    pub explicit_curve_asset: String,
    #[serde(default)]
    pub main_attribute_id: String,
    #[serde(default)]
    pub main_attribute_name_snapshot: String,
    #[serde(default)]
    pub function_type: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct SlotDef {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub name: String,
    #[serde(rename = "type", default)]
    pub slot_type: String,
    #[serde(rename = "candidateIds", default)]
    pub candidate_ids: Vec<String>,
    #[serde(rename = "virtualCandidateIds", default)]
    pub virtual_candidate_ids: Vec<String>,
    #[serde(rename = "defaultHidden", default)]
    pub default_hidden: bool,
    #[serde(rename = "hideForPlayer", default)]
    pub hide_for_player: bool,
    #[serde(rename = "allCandidatesVirtual", default)]
    pub all_candidates_virtual: bool,
    #[serde(rename = "parentLabelOptions", default)]
    pub parent_label_options: Option<Value>,
    #[serde(rename = "parentLabels", default)]
    pub parent_labels: Option<Value>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct Preset {
    #[serde(default)]
    pub nodes: Vec<PresetNode>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct PresetNode {
    #[serde(rename = "isRoot", default)]
    pub is_root: bool,
    #[serde(rename = "isVirtual", default)]
    pub is_virtual: bool,
    #[serde(rename = "parentSocketId", default)]
    pub parent_socket_id: Option<String>,
    #[serde(rename = "partId", default)]
    pub part_id: String,
    #[serde(rename = "pathKey", default)]
    pub path_key: Option<String>,
    #[serde(rename = "tunePairs", default)]
    pub tune_pairs: Vec<TunePair>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct TunePair {
    #[serde(default)]
    pub tune_id: String,
    #[serde(default)]
    pub tune_value: f64,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct VirtualRule {
    #[serde(rename = "socketId", default)]
    pub socket_id: String,
    #[serde(rename = "virtualPartId", default)]
    pub virtual_part_id: String,
    #[serde(rename = "conditionType", default)]
    pub condition_type: String,
    #[serde(rename = "conditionItemIds", default)]
    pub condition_item_ids: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct CandidateOverride {
    #[serde(default)]
    pub decision: String,
    #[serde(rename = "hostPartId", default)]
    pub host_part_id: Option<String>,
    #[serde(rename = "socketId", default)]
    pub socket_id: Option<String>,
    #[serde(rename = "partId", default)]
    pub part_id: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct WeaponStats {
    #[serde(rename = "byMode", default)]
    pub by_mode: Map<String, ModeStats>,
    #[serde(flatten)]
    pub extra: Map<String, Num>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatChannel {
    Base,
    Display,
}

impl WeaponStats {
    /// Mirrors the site's `st(stats, mode, channel)`: look up the mode entry,
    /// use its `base`/`display` object when the entry is structured, use the
    /// flat object itself when it has neither, and fall back to the weapon's
    /// top-level stats when the mode entry is missing.
    pub fn stat(&self, mode: &str, channel: StatChannel, id: &str) -> Option<f64> {
        let value = match self.by_mode.get(mode) {
            None => self.extra.get(id),
            Some(stats) if stats.base.is_none() && stats.display.is_none() => stats.extra.get(id),
            Some(stats) => {
                let (primary, secondary) = match channel {
                    StatChannel::Base => (&stats.base, &stats.display),
                    StatChannel::Display => (&stats.display, &stats.base),
                };
                primary
                    .as_ref()
                    .or(secondary.as_ref())
                    .and_then(|values| values.get(id))
            }
        };
        value.map(|n| n.0)
    }

    pub fn base_stat(&self, mode: &str, id: &str) -> Option<f64> {
        self.stat(mode, StatChannel::Base, id)
    }

    pub fn display_stat(&self, mode: &str, id: &str) -> Option<f64> {
        self.stat(mode, StatChannel::Display, id)
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct ModeStats {
    #[serde(default)]
    pub base: Option<Map<String, Num>>,
    #[serde(default)]
    pub display: Option<Map<String, Num>>,
    /// Flat mode stats (`{"10001": 27, ...}`) for modes that carry no
    /// `base`/`display` split.
    #[serde(flatten)]
    pub extra: Map<String, Num>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct NumericProfile {
    #[serde(rename = "linkedTables", default)]
    pub linked_tables: LinkedTables,
    #[serde(default)]
    pub detail: DetailProfile,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct LinkedTables {
    #[serde(rename = "aimRecoil", default)]
    pub aim_recoil: AimRecoil,
    #[serde(rename = "recoilCurves", default)]
    pub recoil_curves: RecoilCurves,
    #[serde(rename = "handlingCurves", default)]
    pub handling_curves: HandlingCurves,
    #[serde(rename = "stabilityCurves", default)]
    pub stability_curves: StabilityCurves,
    #[serde(rename = "waistSpread", default)]
    pub waist_spread: Option<WaistSpread>,
    #[serde(default)]
    pub aiming: Value,
    #[serde(rename = "movementByMode", default)]
    pub movement_by_mode: Map<String, Value>,
    #[serde(rename = "holdBreath", default)]
    pub hold_breath: Value,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct HandlingCurves {
    #[serde(rename = "adsTime", default)]
    pub ads_time: Option<Curve>,
    #[serde(rename = "sprintToFire", default)]
    pub sprint_to_fire: Option<Curve>,
    #[serde(rename = "sprintToFireMP", default)]
    pub sprint_to_fire_mp: Option<Curve>,
    #[serde(rename = "adsStrafeSpeed", default)]
    pub ads_strafe_speed: Option<Curve>,
    #[serde(rename = "adsStrafeSpeedMP", default)]
    pub ads_strafe_speed_mp: Option<Curve>,
    #[serde(rename = "hipSpread", default)]
    pub hip_spread: Option<Curve>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct StabilityCurves {
    #[serde(rename = "breathScale", default)]
    pub breath_scale: Option<Curve>,
    #[serde(rename = "breathTime", default)]
    pub breath_time: Option<Curve>,
    #[serde(rename = "fireCameraStability", default)]
    pub fire_camera_stability: Option<Curve>,
    #[serde(rename = "movementCameraStability", default)]
    pub movement_camera_stability: Option<Curve>,
    #[serde(rename = "holdBreathRecoil", default)]
    pub hold_breath_recoil: Option<Curve>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct WaistSpread {
    #[serde(rename = "AreaSize", default)]
    pub area_size: Vec<Num>,
    #[serde(rename = "SpreadMax", default)]
    pub spread_max: SpreadMax,
    #[serde(rename = "LuaDisplaySpreadFactor", default)]
    pub lua_display_spread_factor: Option<Num>,
    #[serde(rename = "LuaDisplaySpreadUIFactor", default)]
    pub lua_display_spread_ui_factor: Option<Num>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct SpreadMax {
    #[serde(rename = "X", default)]
    pub x: Option<Num>,
    #[serde(rename = "Y", default)]
    pub y: Option<Num>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct AimRecoil {
    #[serde(rename = "ContinueShootRecoil", default)]
    pub continue_shoot: RecoilScale,
    #[serde(rename = "SingleOrBurstShootRecoil", default)]
    pub single_or_burst: RecoilScale,
    #[serde(rename = "RecoilShakePitchWithCurve", default)]
    pub shake_pitch: Value,
    #[serde(rename = "RecoilShakeYawWithCurve", default)]
    pub shake_yaw: Value,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct RecoilScale {
    #[serde(rename = "HorizontalScale", default)]
    pub horizontal_scale: Option<f64>,
    #[serde(rename = "VerticalScale", default)]
    pub vertical_scale: Option<f64>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct RecoilCurves {
    #[serde(rename = "recoilControl", default)]
    pub recoil_control: Option<Curve>,
    #[serde(rename = "gunkick", default)]
    pub gunkick: Option<Curve>,
    #[serde(rename = "recoilControlMP", default)]
    pub recoil_control_mp: Option<Curve>,
    #[serde(rename = "gunkickMP", default)]
    pub gunkick_mp: Option<Curve>,
}

#[derive(Debug, Clone, Default)]
pub struct Curve {
    pub keys: Vec<CurveKey>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct CurveKey {
    #[serde(rename = "Time", alias = "time", default, deserialize_with = "de_num")]
    pub time: f64,
    #[serde(rename = "Value", alias = "value", default, deserialize_with = "de_num")]
    pub value: f64,
    #[serde(rename = "InterpMode", alias = "interpolation", default)]
    pub interp_mode: String,
    #[serde(
        rename = "LeaveTangent",
        alias = "leave_tangent",
        default,
        deserialize_with = "de_num"
    )]
    pub leave_tangent: f64,
    #[serde(
        rename = "ArriveTangent",
        alias = "arrive_tangent",
        default,
        deserialize_with = "de_num"
    )]
    pub arrive_tangent: f64,
    #[serde(rename = "key_index", default)]
    pub key_index: i64,
}

impl<'de> Deserialize<'de> for Curve {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Repr {
            WithKeys {
                #[serde(default)]
                keys: Vec<CurveKey>,
            },
            Arr(Vec<CurveKey>),
        }
        let repr = Repr::deserialize(deserializer)?;
        let mut keys = match repr {
            Repr::WithKeys { keys } => keys,
            Repr::Arr(keys) => keys,
        };
        // The site sorts curve keys by Time only (stable), so equal-time keys
        // keep the payload order; key_index is not a tie-break here.
        keys.sort_by(|a, b| {
            a.time
                .partial_cmp(&b.time)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        Ok(Curve { keys })
    }
}

impl Curve {
    pub fn eval(&self, x: f64) -> Option<f64> {
        if self.keys.is_empty() || !x.is_finite() {
            return None;
        }
        let first = &self.keys[0];
        if x <= first.time {
            return Some(first.value);
        }
        let last = &self.keys[self.keys.len() - 1];
        if x >= last.time {
            return Some(last.value);
        }
        for i in 0..self.keys.len() - 1 {
            let a = &self.keys[i];
            let b = &self.keys[i + 1];
            if x > b.time {
                continue;
            }
            if b.time == a.time {
                return Some(b.value);
            }
            let t = (x - a.time) / (b.time - a.time);
            // The site only special-cases Constant and Linear; cubic is the
            // fallback for every other (or missing) InterpMode.
            return match a.interp_mode.as_str() {
                "RCIM_Constant" => Some(a.value),
                "RCIM_Linear" => Some(a.value + (b.value - a.value) * t),
                _ => {
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
            }
        }
        Some(last.value)
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct DetailProfile {
    #[serde(rename = "raw_weapon_attribute", default)]
    pub raw_weapon_attribute: Value,
    #[serde(rename = "raw_supporting_values", default)]
    pub raw_supporting_values: Value,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, PartialOrd, Serialize)]
pub struct Num(pub f64);

impl Num {
    pub fn get(self) -> f64 {
        self.0
    }
}

impl<'de> Deserialize<'de> for Num {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Num;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("number or numeric string")
            }
            fn visit_f64<E>(self, v: f64) -> Result<Num, E> {
                Ok(Num(v))
            }
            fn visit_i64<E>(self, v: i64) -> Result<Num, E> {
                Ok(Num(v as f64))
            }
            fn visit_u64<E>(self, v: u64) -> Result<Num, E> {
                Ok(Num(v as f64))
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<Num, E> {
                v.trim().parse::<f64>().map(Num).map_err(E::custom)
            }
            fn visit_string<E: de::Error>(self, v: String) -> Result<Num, E> {
                self.visit_str(&v)
            }
        }
        deserializer.deserialize_any(V)
    }
}

fn de_num<'de, D>(deserializer: D) -> Result<f64, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(Num::deserialize(deserializer)?.0)
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(untagged)]
pub enum FlexibleId {
    Int(i64),
    Str(String),
    #[default]
    Null,
}

impl FlexibleId {
    pub fn as_string(&self) -> String {
        match self {
            FlexibleId::Int(v) => v.to_string(),
            FlexibleId::Str(v) => v.clone(),
            FlexibleId::Null => String::new(),
        }
    }
    pub fn is(&self, value: &str) -> bool {
        self.as_string() == value
    }
}

impl Part {
    pub fn is_virtual(&self) -> bool {
        self.is_virtual
    }
    pub fn is_hidden(&self) -> bool {
        self.hidden.unwrap_or(false)
    }
    pub fn price_id(&self) -> &str {
        self.official_object_id.as_deref().unwrap_or(&self.id)
    }

    /// Display modifiers that apply to `weapon_id` in `mode`: the part's own
    /// list followed by the weapon's baseline recovery list. The site appends
    /// the baseline list after the normal one, so callers that care about
    /// ordering (the last `Initial` for 10001 wins) see the same sequence.
    pub fn display_mods_for<'b>(
        &'b self,
        weapon_id: &str,
        mode: &str,
    ) -> impl Iterator<Item = &'b Modifier> + 'b {
        let normal = self
            .display_mods
            .get(mode)
            .map(|v| v.as_slice())
            .unwrap_or(&[]);
        let baseline = self
            .display_baseline_mods
            .get(weapon_id)
            .and_then(|m| m.get(mode))
            .map(|v| v.as_slice())
            .unwrap_or(&[]);
        normal.iter().chain(baseline.iter())
    }

    /// Main attribute modifiers that apply to `weapon_id` in `mode`, with the
    /// same normal-then-baseline order as [`Part::display_mods_for`].
    pub fn main_mods_for<'b>(
        &'b self,
        weapon_id: &str,
        mode: &str,
    ) -> impl Iterator<Item = &'b Modifier> + 'b {
        let normal = self
            .main_mods
            .get(mode)
            .map(|v| v.as_slice())
            .unwrap_or(&[]);
        let baseline = self
            .main_baseline_mods
            .get(weapon_id)
            .and_then(|m| m.get(mode))
            .map(|v| v.as_slice())
            .unwrap_or(&[]);
        normal.iter().chain(baseline.iter())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn curve_linear_interpolates() {
        let curve = Curve {
            keys: vec![key(0.0, 1.0), key(10.0, 0.5)],
        };
        assert!((curve.eval(0.0).unwrap() - 1.0).abs() < 1e-9);
        assert!((curve.eval(5.0).unwrap() - 0.75).abs() < 1e-9);
        assert!((curve.eval(10.0).unwrap() - 0.5).abs() < 1e-9);
    }

    #[test]
    fn curve_clamps_outside_range() {
        let curve = Curve {
            keys: vec![key(0.0, 1.0), key(10.0, 0.5)],
        };
        assert!((curve.eval(-5.0).unwrap() - 1.0).abs() < 1e-9);
        assert!((curve.eval(15.0).unwrap() - 0.5).abs() < 1e-9);
    }

    #[test]
    fn curve_constant_segments() {
        let mut first = key(0.0, 1.0);
        first.interp_mode = "RCIM_Constant".to_string();
        let curve = Curve {
            keys: vec![first, key(10.0, 0.5)],
        };
        assert!((curve.eval(9.9).unwrap() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn curve_equal_time_keys_keep_input_order() {
        // The site's curve normalization only sorts by Time, and Rust's sort
        // is stable, so equal-time keys retain the payload order instead of
        // being reordered by key_index.
        let curve: Curve = serde_json::from_value(serde_json::json!({
            "keys": [
                { "Time": 0, "Value": 10, "key_index": 1 },
                { "Time": 0, "Value": 20, "key_index": 0 },
                { "Time": 10, "Value": 30, "key_index": 2 }
            ]
        }))
        .unwrap();
        assert_eq!(curve.eval(0.0), Some(10.0));
        assert_eq!(curve.eval(-1.0), Some(10.0));
        assert_eq!(curve.eval(5.0), Some(25.0));
    }

    #[test]
    fn curve_unknown_or_missing_interp_mode_is_cubic() {
        // The site only special-cases Constant and Linear; anything else
        // (including a missing InterpMode) uses the cubic hermite formula.
        for mode in [Some("RCIM_Bogus"), None] {
            let mut first = key(0.0, 10.0);
            first.leave_tangent = 0.0;
            first.interp_mode = mode.map(str::to_string).unwrap_or_default();
            let curve = Curve {
                keys: vec![first, key(10.0, 30.0)],
            };
            assert!(
                (curve.eval(2.5).unwrap() - 13.125).abs() < 1e-9,
                "mode {mode:?}"
            );
        }
    }

    #[test]
    fn curve_linear_mode_stays_linear() {
        let curve = Curve {
            keys: vec![key(0.0, 10.0), key(10.0, 30.0)],
        };
        assert!((curve.eval(2.5).unwrap() - 15.0).abs() < 1e-9);
    }
}
