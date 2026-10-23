use crate::assembly::Assembly;

use super::Evaluator;

impl<'a> Evaluator<'a> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics::test_support::{price_assembly, price_payload};
    use rustc_hash::FxHashMap;

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
}
