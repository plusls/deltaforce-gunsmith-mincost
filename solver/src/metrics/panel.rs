use crate::model::Part;
use rustc_hash::FxHashMap;

use super::js_math::js_round;
use super::part_rules::{has_laser, is_addend, laser_pro_index, modifier_included, normalize_attr};
use super::Evaluator;

impl<'a> Evaluator<'a> {
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Part, WeaponPayload};

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
}
