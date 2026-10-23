use crate::model::{FlexibleId, Modifier, Part};
use rustc_hash::FxHashSet;

pub(crate) fn normalize_attr(attr: Option<&str>) -> &str {
    match attr {
        Some("100103") => "10001",
        Some(v) => v,
        None => "",
    }
}

pub(crate) fn is_addend(m: &Modifier) -> bool {
    m.condition_id.is_none()
        && m.modifier_type == "Addend"
        && m.delta.map(|v| v.0.is_finite()).unwrap_or(false)
}

pub(crate) fn modifier_included(
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

pub(crate) fn laser_pro_index(parts: &[&Part], mode: &str) -> usize {
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

pub(crate) fn is_recoil_target(target: &str) -> bool {
    target.starts_with("GGunkick")
        || target.starts_with("GRecoil_")
        || target.ends_with("BreathFactor")
}

pub(crate) fn is_bipod_like(part: &Part) -> bool {
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
