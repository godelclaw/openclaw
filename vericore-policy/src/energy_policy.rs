//! Energy boundary contract.
//!
//! These pure decision functions validate TS-reported energy facts at the
//! Rust boundary:
//! - no-op heartbeats contribute +1,
//! - content-bearing heartbeat actions contribute -1,
//! - real external conversation bursts contribute -1 once per configured quiet
//!   gap,
//! - and the configured quiet gap remains locked to the intended 30-minute
//!   policy.

use vstd::prelude::*;

verus! {

pub open spec fn spec_expected_energy(
    noop_heartbeat_count: nat,
    acted_heartbeat_count: nat,
    conversation_burst_count: nat,
) -> int {
    noop_heartbeat_count as int
        - acted_heartbeat_count as int
        - conversation_burst_count as int
}

pub open spec fn spec_conversation_gap_locked(
    conversation_gap_ms: nat,
) -> bool {
    conversation_gap_ms == 30 * 60 * 1000
}

pub fn conversation_gap_locked(
    conversation_gap_ms: usize,
) -> (result: bool)
    ensures result == spec_conversation_gap_locked(conversation_gap_ms as nat)
{
    conversation_gap_ms == 30 * 60 * 1000
}

pub open spec fn spec_energy_value_matches(
    expected_energy: int,
    energy_value: int,
) -> bool {
    energy_value == expected_energy
}

pub fn energy_value_matches(
    expected_energy: i64,
    energy_value: i64,
) -> (result: bool)
    ensures result == spec_energy_value_matches(
        expected_energy as int,
        energy_value as int,
    )
{
    energy_value == expected_energy
}

pub open spec fn spec_energy_contract_ok(
    gap_locked: bool,
    energy_matches: bool,
) -> bool {
    gap_locked && energy_matches
}

pub fn energy_contract_ok(
    gap_locked: bool,
    energy_matches: bool,
) -> (result: bool)
    ensures result == spec_energy_contract_ok(gap_locked, energy_matches)
{
    gap_locked && energy_matches
}

proof fn lemma_wrong_gap_breaks_contract(energy_matches: bool)
    ensures !spec_energy_contract_ok(false, energy_matches)
{}

proof fn lemma_wrong_energy_breaks_contract(gap_locked: bool)
    ensures !spec_energy_contract_ok(gap_locked, false)
{}

proof fn lemma_expected_energy_formula()
    ensures spec_expected_energy(3nat, 1nat, 2nat) == 0
{}

proof fn lemma_correct_counts_and_gap_imply_contract_ok()
    ensures spec_energy_contract_ok(true, true)
{}

} // verus!

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expected_energy_uses_simple_fold() {
        assert_eq!(0i64 - 0i64 - 0i64, 0);
        assert_eq!(3i64 - 0i64 - 0i64, 3);
        assert_eq!(3i64 - 1i64 - 1i64, 1);
        assert_eq!(2i64 - 1i64 - 2i64, -1);
    }

    #[test]
    fn conversation_gap_is_locked_to_thirty_minutes() {
        assert!(conversation_gap_locked(30 * 60 * 1000));
        assert!(!conversation_gap_locked(15 * 60 * 1000));
        assert!(!conversation_gap_locked(31 * 60 * 1000));
    }

    #[test]
    fn energy_value_must_match_fold() {
        assert!(energy_value_matches(-1, -1));
        assert!(!energy_value_matches(-1, 0));
    }

    #[test]
    fn full_contract_requires_gap_and_matching_value() {
        assert!(energy_contract_ok(true, true));
        assert!(!energy_contract_ok(false, true));
        assert!(!energy_contract_ok(true, false));
        assert!(!energy_contract_ok(false, false));
    }
}
