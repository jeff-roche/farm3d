//! Milligram range constants and gram<->milligram conversion (D1). Rust,
//! SQLite, and the wire all store amounts as integer milligrams; grams
//! exist only for display and the frontend's `src/spools/weight.ts`. This
//! module is the single place that knows the conversion, so
//! [`super::validate_fields`] and later tasks' ledger/reservation math
//! (Task 2, Task 4) share one definition of each D1 range.

use std::ops::RangeInclusive;

/// D1: nominal net weight is 1 g to 50 kg.
pub const NOMINAL_MG_RANGE: RangeInclusive<i64> = 1_000..=50_000_000;
/// D1: current net weight is 0 to 50 kg.
pub const CURRENT_MG_RANGE: RangeInclusive<i64> = 0..=50_000_000;
/// D1: tare is 0 to 5 kg.
pub const TARE_MG_RANGE: RangeInclusive<i64> = 0..=5_000_000;
/// D1: low threshold is 0 to 50 kg.
pub const LOW_THRESHOLD_MG_RANGE: RangeInclusive<i64> = 0..=50_000_000;

const MG_PER_GRAM: f64 = 1_000.0;

/// Converts a (possibly fractional) gram amount to whole milligrams,
/// rounding up. D1: "when P7 converts a slicer estimate (fractional grams)
/// into a reservation, it rounds up to the next milligram."
pub fn grams_to_mg_round_up(grams: f64) -> i64 {
    (grams * MG_PER_GRAM).ceil() as i64
}

/// Converts whole milligrams to grams, for display.
pub fn mg_to_grams(mg: i64) -> f64 {
    mg as f64 / MG_PER_GRAM
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grams_to_mg_round_up_rounds_up_a_fractional_gram() {
        assert_eq!(grams_to_mg_round_up(1.0001), 1001);
        assert_eq!(grams_to_mg_round_up(2.0), 2000);
        assert_eq!(grams_to_mg_round_up(0.0), 0);
    }

    #[test]
    fn mg_to_grams_divides_by_a_thousand() {
        assert_eq!(mg_to_grams(1_500), 1.5);
        assert_eq!(mg_to_grams(0), 0.0);
    }

    #[test]
    fn nominal_range_matches_d1_one_gram_to_fifty_kilograms() {
        assert!(!NOMINAL_MG_RANGE.contains(&999));
        assert!(NOMINAL_MG_RANGE.contains(&1_000));
        assert!(NOMINAL_MG_RANGE.contains(&50_000_000));
        assert!(!NOMINAL_MG_RANGE.contains(&50_000_001));
    }

    #[test]
    fn current_and_low_threshold_ranges_allow_zero_to_fifty_kilograms() {
        assert!(CURRENT_MG_RANGE.contains(&0));
        assert!(CURRENT_MG_RANGE.contains(&50_000_000));
        assert!(!CURRENT_MG_RANGE.contains(&50_000_001));
        assert!(LOW_THRESHOLD_MG_RANGE.contains(&0));
        assert!(LOW_THRESHOLD_MG_RANGE.contains(&50_000_000));
        assert!(!LOW_THRESHOLD_MG_RANGE.contains(&50_000_001));
    }

    #[test]
    fn tare_range_allows_zero_to_five_kilograms() {
        assert!(TARE_MG_RANGE.contains(&0));
        assert!(TARE_MG_RANGE.contains(&5_000_000));
        assert!(!TARE_MG_RANGE.contains(&5_000_001));
    }
}
