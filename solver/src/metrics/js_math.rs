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
pub(crate) fn js_round(value: f64) -> f64 {
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

pub(crate) fn round2(value: f64) -> f64 {
    js_round((value + f64::EPSILON) * 100.0) / 100.0
}

pub(crate) fn fround(value: f64) -> f64 {
    value as f32 as f64
}

pub(crate) fn ms_floor(value: f64) -> f64 {
    (fround(value) * 1000.0).floor()
}

pub(crate) fn clamp_factor(value: f64) -> f64 {
    value.clamp(0.25, 2.5)
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
