/// Convert a linear amplitude ratio to decibels.
///
/// Zero maps to negative infinity. Negative and non-finite inputs are rejected.
#[must_use]
pub fn linear_to_db(linear: f32) -> Option<f32> {
    if linear.is_nan() || linear < 0.0 || linear == f32::INFINITY {
        return None;
    }
    if linear == 0.0 {
        return Some(f32::NEG_INFINITY);
    }
    Some(20.0 * linear.log10())
}

/// Convert decibels to a linear amplitude ratio.
#[must_use]
pub fn db_to_linear(db: f32) -> Option<f32> {
    if db.is_nan() || db == f32::INFINITY {
        return None;
    }
    if db == f32::NEG_INFINITY {
        return Some(0.0);
    }
    let linear = 10.0_f32.powf(db / 20.0);
    linear.is_finite().then_some(linear)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_amplitude_conversions_are_stable() {
        let minus_six = linear_to_db(0.5).expect("0.5 is valid");
        assert!((minus_six - -6.020_600_3).abs() < 0.000_1);
        let linear = db_to_linear(minus_six).expect("finite dB is valid");
        assert!((linear - 0.5).abs() < 0.000_01);
    }

    #[test]
    fn silence_round_trips() {
        assert_eq!(linear_to_db(0.0), Some(f32::NEG_INFINITY));
        assert_eq!(db_to_linear(f32::NEG_INFINITY), Some(0.0));
    }

    #[test]
    fn invalid_values_are_rejected() {
        assert_eq!(linear_to_db(-0.1), None);
        assert_eq!(linear_to_db(f32::NAN), None);
        assert_eq!(db_to_linear(f32::INFINITY), None);
    }
}
