//! Affect trace boundary contract.
//!
//! Validates TS-reported affect fold steps at the Rust boundary.
//! Each dimension is scaled ×100 to integer arithmetic: TS 0.69 → Rust 69.
//! The fold recurrence per dimension is:
//!
//!   result = clamp(prev * gamma / 100 + obs * (100 - gamma) / 100, -100, 100)
//!
//! The contract checks:
//! - gamma is locked to 69 (representing 0.69),
//! - all dimensions are bounded in [-100, 100],
//! - the fold result matches Rust's recomputation.

use vstd::prelude::*;

verus! {

pub open spec fn spec_affect_gamma_locked(gamma: nat) -> bool {
    gamma == 69
}

pub fn affect_gamma_locked(gamma: usize) -> (result: bool)
    ensures result == spec_affect_gamma_locked(gamma as nat)
{
    gamma == 69
}

pub open spec fn spec_affect_dimension_bounded(value: int) -> bool {
    -100 <= value && value <= 100
}

pub fn affect_dimension_bounded(value: i64) -> (result: bool)
    ensures result == spec_affect_dimension_bounded(value as int)
{
    -100 <= value && value <= 100
}

pub open spec fn spec_affect_fold_step(prev: int, obs: int, gamma: int) -> int {
    let raw = prev * gamma + obs * (100 - gamma);
    let divided = if raw >= 0 { (raw + 50) / 100 } else { (raw - 50) / 100 };
    if divided < -100 { -100 as int }
    else if divided > 100 { 100 as int }
    else { divided }
}

pub fn affect_fold_step(prev: i64, obs: i64, gamma: i64) -> (result: i64)
    requires
        -100 <= prev && prev <= 100,
        -100 <= obs && obs <= 100,
        0 <= gamma && gamma <= 100,
    ensures result == spec_affect_fold_step(prev as int, obs as int, gamma as int)
{
    let raw: i64 = prev * gamma + obs * (100 - gamma);
    let divided: i64 = if raw >= 0 { (raw + 50) / 100 } else { (raw - 50) / 100 };
    if divided < -100 { -100 }
    else if divided > 100 { 100 }
    else { divided }
}

pub open spec fn spec_affect_fold_matches(
    expected: int,
    actual: int,
) -> bool {
    -1 <= expected - actual && expected - actual <= 1
}

pub fn affect_fold_matches(
    expected: i64,
    actual: i64,
) -> (result: bool)
    ensures result == spec_affect_fold_matches(expected as int, actual as int)
{
    (expected - actual).abs() <= 1
}

pub open spec fn spec_affect_contract_ok(
    gamma_locked: bool,
    all_bounded: bool,
    fold_matches: bool,
) -> bool {
    gamma_locked && all_bounded && fold_matches
}

pub fn affect_contract_ok(
    gamma_locked: bool,
    all_bounded: bool,
    fold_matches: bool,
) -> (result: bool)
    ensures result == spec_affect_contract_ok(gamma_locked, all_bounded, fold_matches)
{
    gamma_locked && all_bounded && fold_matches
}

proof fn lemma_wrong_gamma_breaks_contract(all_bounded: bool, fold_matches: bool)
    ensures !spec_affect_contract_ok(false, all_bounded, fold_matches)
{}

proof fn lemma_unbounded_breaks_contract(gamma_locked: bool, fold_matches: bool)
    ensures !spec_affect_contract_ok(gamma_locked, false, fold_matches)
{}

proof fn lemma_fold_mismatch_breaks_contract(gamma_locked: bool, all_bounded: bool)
    ensures !spec_affect_contract_ok(gamma_locked, all_bounded, false)
{}

proof fn lemma_all_pass_implies_contract_ok()
    ensures spec_affect_contract_ok(true, true, true)
{}

proof fn lemma_fold_step_bounded()
    ensures
        spec_affect_fold_step(100, 100, 69) <= 100,
        spec_affect_fold_step(-100, -100, 69) >= -100i64 as int,
{}

proof fn lemma_fold_step_example()
    ensures spec_affect_fold_step(50, 30, 69) == 44
    // prev=50 (0.50), obs=30 (0.30), gamma=69
    // raw = 50*69 + 30*31 = 3450 + 930 = 4380
    // divided = (4380 + 50) / 100 = 4430 / 100 = 44
{}

} // verus!

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gamma_is_locked_to_69() {
        assert!(affect_gamma_locked(69));
        assert!(!affect_gamma_locked(70));
        assert!(!affect_gamma_locked(0));
    }

    #[test]
    fn dimension_bounded_accepts_valid_range() {
        assert!(affect_dimension_bounded(0));
        assert!(affect_dimension_bounded(100));
        assert!(affect_dimension_bounded(-100));
        assert!(!affect_dimension_bounded(101));
        assert!(!affect_dimension_bounded(-101));
    }

    #[test]
    fn fold_step_matches_ewma_formula() {
        // prev=50, obs=30, gamma=69
        // raw = 50*69 + 30*31 = 3450 + 930 = 4380
        // (4380 + 50) / 100 = 44
        assert_eq!(affect_fold_step(50, 30, 69), 44);

        // prev=0, obs=100, gamma=69
        // raw = 0 + 100*31 = 3100
        // (3100 + 50) / 100 = 31
        assert_eq!(affect_fold_step(0, 100, 69), 31);

        // prev=0, obs=-100, gamma=69
        // raw = 0 + (-100)*31 = -3100
        // (-3100 - 50) / 100 = -31
        assert_eq!(affect_fold_step(0, -100, 69), -31);
    }

    #[test]
    fn fold_step_clamps_to_bounds() {
        // extreme case: prev=100, obs=100 → stays at 100
        assert_eq!(affect_fold_step(100, 100, 69), 100);
        assert_eq!(affect_fold_step(-100, -100, 69), -100);
    }

    #[test]
    fn fold_matches_allows_rounding_tolerance() {
        assert!(affect_fold_matches(44, 44));
        assert!(affect_fold_matches(44, 45));
        assert!(affect_fold_matches(44, 43));
        assert!(!affect_fold_matches(44, 46));
        assert!(!affect_fold_matches(44, 42));
    }

    #[test]
    fn full_contract_requires_all_three() {
        assert!(affect_contract_ok(true, true, true));
        assert!(!affect_contract_ok(false, true, true));
        assert!(!affect_contract_ok(true, false, true));
        assert!(!affect_contract_ok(true, true, false));
        assert!(!affect_contract_ok(false, false, false));
    }
}
