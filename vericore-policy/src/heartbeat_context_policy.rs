//! Heartbeat continuity-context boundary contract.
//!
//! These pure decision functions validate TS-reported facts about the
//! deterministic heartbeat continuity view at the Rust boundary:
//! - the recent completed-turn window stays within its configured limit,
//! - a reservable recent main Telegram turn is preserved when present,
//! - content-bearing heartbeat turns remain visible in the main window,
//! - no-op heartbeats do not occupy the main recent-turn slots,
//! - the separate heartbeat trace stays within its limit and is ordered
//!   oldest-first,
//! - the heartbeat trace remains a heartbeat-only block rather than mixing in
//!   ordinary user turns.

use vstd::prelude::*;

verus! {

/// Spec: the recent-turn window never exceeds its configured limit.
pub open spec fn spec_recent_turns_within_limit(
    recent_turn_count: nat,
    recent_turn_limit: nat,
) -> bool {
    recent_turn_count <= recent_turn_limit
}

pub fn recent_turns_within_limit(
    recent_turn_count: usize,
    recent_turn_limit: usize,
) -> (result: bool)
    ensures result == spec_recent_turns_within_limit(recent_turn_count as nat, recent_turn_limit as nat)
{
    recent_turn_count <= recent_turn_limit
}

/// Spec: if a reservable recent main Telegram turn exists in the candidate
/// pool, it must be present in the selected recent-turn window.
pub open spec fn spec_reserved_main_turn_preserved(
    reserved_candidate_present: bool,
    reserved_included: bool,
) -> bool {
    !reserved_candidate_present || reserved_included
}

pub fn reserved_main_turn_preserved(
    reserved_candidate_present: bool,
    reserved_included: bool,
) -> (result: bool)
    ensures result == spec_reserved_main_turn_preserved(
        reserved_candidate_present,
        reserved_included,
    )
{
    !reserved_candidate_present || reserved_included
}

/// Spec: if a content-bearing heartbeat exists in the candidate pool, at least
/// one such heartbeat remains visible in the recent-turn window.
pub open spec fn spec_content_heartbeat_preserved(
    content_heartbeat_candidate_present: bool,
    content_heartbeat_included: bool,
) -> bool {
    !content_heartbeat_candidate_present || content_heartbeat_included
}

pub fn content_heartbeat_preserved(
    content_heartbeat_candidate_present: bool,
    content_heartbeat_included: bool,
) -> (result: bool)
    ensures result == spec_content_heartbeat_preserved(
        content_heartbeat_candidate_present,
        content_heartbeat_included,
    )
{
    !content_heartbeat_candidate_present || content_heartbeat_included
}

/// Spec: no-op heartbeats belong in the separate heartbeat trace, not in the
/// main recent-turn window.
pub open spec fn spec_noop_heartbeat_excluded_from_recent_turns(
    noop_heartbeat_in_recent_turns: bool,
) -> bool {
    !noop_heartbeat_in_recent_turns
}

pub fn noop_heartbeat_excluded_from_recent_turns(
    noop_heartbeat_in_recent_turns: bool,
) -> (result: bool)
    ensures result == spec_noop_heartbeat_excluded_from_recent_turns(
        noop_heartbeat_in_recent_turns,
    )
{
    !noop_heartbeat_in_recent_turns
}

/// Spec: the heartbeat trace stays within its configured limit and is rendered
/// oldest-first for temporal continuity.
pub open spec fn spec_heartbeat_trace_ok(
    heartbeat_trace_count: nat,
    heartbeat_trace_limit: nat,
    heartbeat_trace_oldest_first: bool,
) -> bool {
    heartbeat_trace_count <= heartbeat_trace_limit && heartbeat_trace_oldest_first
}

pub fn heartbeat_trace_ok(
    heartbeat_trace_count: usize,
    heartbeat_trace_limit: usize,
    heartbeat_trace_oldest_first: bool,
) -> (result: bool)
    ensures result == spec_heartbeat_trace_ok(
        heartbeat_trace_count as nat,
        heartbeat_trace_limit as nat,
        heartbeat_trace_oldest_first,
    )
{
    heartbeat_trace_count <= heartbeat_trace_limit && heartbeat_trace_oldest_first
}

/// Spec: the separate heartbeat trace block contains heartbeat entries only.
pub open spec fn spec_heartbeat_trace_separate(
    heartbeat_trace_only_heartbeat_entries: bool,
) -> bool {
    heartbeat_trace_only_heartbeat_entries
}

pub fn heartbeat_trace_separate(
    heartbeat_trace_only_heartbeat_entries: bool,
) -> (result: bool)
    ensures result == spec_heartbeat_trace_separate(
        heartbeat_trace_only_heartbeat_entries,
    )
{
    heartbeat_trace_only_heartbeat_entries
}

/// Spec: the full continuity-context contract holds only when the recent-turn
/// window and heartbeat trace both satisfy their structural invariants.
pub open spec fn spec_heartbeat_context_contract_ok(
    recent_turns_ok: bool,
    reserved_main_ok: bool,
    content_heartbeat_ok: bool,
    noop_recent_turns_ok: bool,
    heartbeat_trace_ok: bool,
    heartbeat_trace_separate: bool,
) -> bool {
    recent_turns_ok
        && reserved_main_ok
        && content_heartbeat_ok
        && noop_recent_turns_ok
        && heartbeat_trace_ok
        && heartbeat_trace_separate
}

pub fn heartbeat_context_contract_ok(
    recent_turns_ok: bool,
    reserved_main_ok: bool,
    content_heartbeat_ok: bool,
    noop_recent_turns_ok: bool,
    heartbeat_trace_ok: bool,
    heartbeat_trace_separate: bool,
) -> (result: bool)
    ensures result == spec_heartbeat_context_contract_ok(
        recent_turns_ok,
        reserved_main_ok,
        content_heartbeat_ok,
        noop_recent_turns_ok,
        heartbeat_trace_ok,
        heartbeat_trace_separate,
    )
{
    recent_turns_ok
        && reserved_main_ok
        && content_heartbeat_ok
        && noop_recent_turns_ok
        && heartbeat_trace_ok
        && heartbeat_trace_separate
}

proof fn lemma_recent_turn_overflow_breaks_recent_turn_window(limit: nat)
    ensures !spec_recent_turns_within_limit(limit + 1, limit)
{}

proof fn lemma_missing_reserved_main_turn_breaks_preservation()
    ensures !spec_reserved_main_turn_preserved(true, false)
{}

proof fn lemma_missing_content_heartbeat_breaks_preservation()
    ensures !spec_content_heartbeat_preserved(true, false)
{}

proof fn lemma_noop_heartbeat_in_recent_turns_breaks_contract()
    ensures !spec_noop_heartbeat_excluded_from_recent_turns(true)
{}

proof fn lemma_unordered_trace_breaks_trace_contract(
    heartbeat_trace_count: nat,
    heartbeat_trace_limit: nat,
)
    ensures !spec_heartbeat_trace_ok(
        heartbeat_trace_count,
        heartbeat_trace_limit,
        false,
    )
{}

proof fn lemma_mixed_trace_breaks_separation()
    ensures !spec_heartbeat_trace_separate(false)
{}

proof fn lemma_all_heartbeat_context_requirements_imply_contract_ok()
    ensures spec_heartbeat_context_contract_ok(true, true, true, true, true, true)
{}

} // verus!

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recent_turns_respect_limit() {
        assert!(recent_turns_within_limit(0, 5));
        assert!(recent_turns_within_limit(5, 5));
        assert!(!recent_turns_within_limit(6, 5));
    }

    #[test]
    fn reserved_main_turn_must_be_kept_when_present() {
        assert!(reserved_main_turn_preserved(false, false));
        assert!(reserved_main_turn_preserved(true, true));
        assert!(!reserved_main_turn_preserved(true, false));
    }

    #[test]
    fn content_heartbeat_must_be_kept_when_present() {
        assert!(content_heartbeat_preserved(false, false));
        assert!(content_heartbeat_preserved(true, true));
        assert!(!content_heartbeat_preserved(true, false));
    }

    #[test]
    fn noop_heartbeats_do_not_belong_in_recent_turn_window() {
        assert!(noop_heartbeat_excluded_from_recent_turns(false));
        assert!(!noop_heartbeat_excluded_from_recent_turns(true));
    }

    #[test]
    fn heartbeat_trace_requires_limit_and_order() {
        assert!(heartbeat_trace_ok(0, 32, true));
        assert!(heartbeat_trace_ok(32, 32, true));
        assert!(!heartbeat_trace_ok(33, 32, true));
        assert!(!heartbeat_trace_ok(1, 32, false));
    }

    #[test]
    fn heartbeat_trace_must_stay_separate() {
        assert!(heartbeat_trace_separate(true));
        assert!(!heartbeat_trace_separate(false));
    }

    #[test]
    fn full_contract_requires_all_parts() {
        assert!(heartbeat_context_contract_ok(
            true, true, true, true, true, true
        ));
        assert!(!heartbeat_context_contract_ok(
            false, true, true, true, true, true
        ));
        assert!(!heartbeat_context_contract_ok(
            true, true, true, true, false, true
        ));
    }
}
