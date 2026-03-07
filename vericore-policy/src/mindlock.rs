//! Mindlock state machine.
//!
//! The mindlock is the core trust boundary for artifact movement. Artifacts
//! (files staged for ingress/egress) move through a state machine:
//!
//!   In/Out/Work (staging) → PendingZar → Promoted
//!                         → Rejected
//!
//! This module proves which transitions are valid, which states are terminal,
//! source validation for moves, and cleanup eligibility for terminal artifacts.

use vstd::prelude::*;

verus! {

/// Mindlock artifact stages, mapped from runtime directory names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MindlockStage {
    /// `in/` — incoming artifacts staged for ingress review
    In,
    /// `out/` — outgoing artifacts staged for egress review
    Out,
    /// `work/` — revision working area
    Work,
    /// `pending-zar/` — awaiting human (Zar) approval
    PendingZar,
    /// `rejected/` — security review rejected this artifact
    Rejected,
    /// Promoted — artifact approved and moved to target location
    Promoted,
}

/// Authorization for a state transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransitionAuth {
    /// Security reviewer verdict: Allow (auto-promote from staging)
    ReviewerAllow,
    /// Security reviewer verdict: RequestRevision (move to Work for revision)
    ReviewerRequestRevision,
    /// Security reviewer verdict: RequireZarApproval (escalate to human)
    ReviewerRequireZarApproval,
    /// Security reviewer verdict: Reject
    ReviewerReject,
    /// Agent-initiated move between staging directories
    AgentMove,
    /// Human (Zar) approves from PendingZar
    ZarApprove,
    /// Human (Zar) rejects from PendingZar
    ZarReject,
}

// ── Spec functions ───────────────────────────────────────────────────

/// Spec: is this stage a staging area (not terminal)?
pub open spec fn spec_is_staging(stage: MindlockStage) -> bool {
    match stage {
        MindlockStage::In | MindlockStage::Out | MindlockStage::Work => true,
        _ => false,
    }
}

/// Spec: is this stage terminal (no valid outgoing transitions)?
pub open spec fn spec_is_terminal(stage: MindlockStage) -> bool {
    match stage {
        MindlockStage::Rejected | MindlockStage::Promoted => true,
        _ => false,
    }
}

/// Spec: is this stage a valid source for move_existing_artifact()?
/// Only staging dirs and pending-zar are valid sources.
/// Terminal states (rejected, promoted) are never valid sources.
pub open spec fn spec_is_valid_move_source(stage: MindlockStage) -> bool {
    match stage {
        MindlockStage::In | MindlockStage::Out | MindlockStage::Work | MindlockStage::PendingZar => true,
        _ => false,
    }
}

/// Spec: is this a valid state transition?
pub open spec fn spec_valid_transition(
    from: MindlockStage,
    to: MindlockStage,
    auth: TransitionAuth,
) -> bool {
    match (from, to, auth) {
        // Staging → Promoted via reviewer Allow
        (MindlockStage::In, MindlockStage::Promoted, TransitionAuth::ReviewerAllow) => true,
        (MindlockStage::Out, MindlockStage::Promoted, TransitionAuth::ReviewerAllow) => true,
        (MindlockStage::Work, MindlockStage::Promoted, TransitionAuth::ReviewerAllow) => true,

        // Staging → Work via reviewer RequestRevision
        (MindlockStage::In, MindlockStage::Work, TransitionAuth::ReviewerRequestRevision) => true,
        (MindlockStage::Out, MindlockStage::Work, TransitionAuth::ReviewerRequestRevision) => true,
        (MindlockStage::Work, MindlockStage::Work, TransitionAuth::ReviewerRequestRevision) => true,

        // Staging → PendingZar via reviewer RequireZarApproval
        (MindlockStage::In, MindlockStage::PendingZar, TransitionAuth::ReviewerRequireZarApproval) => true,
        (MindlockStage::Out, MindlockStage::PendingZar, TransitionAuth::ReviewerRequireZarApproval) => true,
        (MindlockStage::Work, MindlockStage::PendingZar, TransitionAuth::ReviewerRequireZarApproval) => true,

        // Staging → Rejected via reviewer Reject
        (MindlockStage::In, MindlockStage::Rejected, TransitionAuth::ReviewerReject) => true,
        (MindlockStage::Out, MindlockStage::Rejected, TransitionAuth::ReviewerReject) => true,
        (MindlockStage::Work, MindlockStage::Rejected, TransitionAuth::ReviewerReject) => true,

        // Agent can move between staging dirs
        (MindlockStage::In, MindlockStage::Out, TransitionAuth::AgentMove) => true,
        (MindlockStage::Out, MindlockStage::In, TransitionAuth::AgentMove) => true,
        (MindlockStage::In, MindlockStage::Work, TransitionAuth::AgentMove) => true,
        (MindlockStage::Out, MindlockStage::Work, TransitionAuth::AgentMove) => true,
        (MindlockStage::Work, MindlockStage::In, TransitionAuth::AgentMove) => true,
        (MindlockStage::Work, MindlockStage::Out, TransitionAuth::AgentMove) => true,

        // Agent-initiated escalation from staging to PendingZar
        (MindlockStage::In, MindlockStage::PendingZar, TransitionAuth::AgentMove) => true,
        (MindlockStage::Out, MindlockStage::PendingZar, TransitionAuth::AgentMove) => true,
        (MindlockStage::Work, MindlockStage::PendingZar, TransitionAuth::AgentMove) => true,

        // PendingZar → Promoted via Zar approval
        (MindlockStage::PendingZar, MindlockStage::Promoted, TransitionAuth::ZarApprove) => true,

        // PendingZar → Rejected via Zar rejection
        (MindlockStage::PendingZar, MindlockStage::Rejected, TransitionAuth::ZarReject) => true,

        // Everything else is invalid
        _ => false,
    }
}

/// Spec: is an artifact eligible for cleanup (deletion)?
/// Only terminal artifacts older than the retention period may be cleaned up.
pub open spec fn spec_cleanup_eligible(
    stage: MindlockStage,
    age_days: nat,
    retention_days: nat,
) -> bool {
    spec_is_terminal(stage) && age_days >= retention_days
}

// ── Exec functions ───────────────────────────────────────────────────

pub fn is_staging(stage: MindlockStage) -> (result: bool)
    ensures result == spec_is_staging(stage)
{
    match stage {
        MindlockStage::In | MindlockStage::Out | MindlockStage::Work => true,
        _ => false,
    }
}

pub fn is_terminal(stage: MindlockStage) -> (result: bool)
    ensures result == spec_is_terminal(stage)
{
    match stage {
        MindlockStage::Rejected | MindlockStage::Promoted => true,
        _ => false,
    }
}

pub fn is_valid_move_source(stage: MindlockStage) -> (result: bool)
    ensures result == spec_is_valid_move_source(stage)
{
    match stage {
        MindlockStage::In | MindlockStage::Out | MindlockStage::Work | MindlockStage::PendingZar => true,
        _ => false,
    }
}

pub fn valid_transition(
    from: MindlockStage,
    to: MindlockStage,
    auth: TransitionAuth,
) -> (result: bool)
    ensures result == spec_valid_transition(from, to, auth)
{
    match (from, to, auth) {
        (MindlockStage::In, MindlockStage::Promoted, TransitionAuth::ReviewerAllow) => true,
        (MindlockStage::Out, MindlockStage::Promoted, TransitionAuth::ReviewerAllow) => true,
        (MindlockStage::Work, MindlockStage::Promoted, TransitionAuth::ReviewerAllow) => true,

        (MindlockStage::In, MindlockStage::Work, TransitionAuth::ReviewerRequestRevision) => true,
        (MindlockStage::Out, MindlockStage::Work, TransitionAuth::ReviewerRequestRevision) => true,
        (MindlockStage::Work, MindlockStage::Work, TransitionAuth::ReviewerRequestRevision) => true,

        (MindlockStage::In, MindlockStage::PendingZar, TransitionAuth::ReviewerRequireZarApproval) => true,
        (MindlockStage::Out, MindlockStage::PendingZar, TransitionAuth::ReviewerRequireZarApproval) => true,
        (MindlockStage::Work, MindlockStage::PendingZar, TransitionAuth::ReviewerRequireZarApproval) => true,

        (MindlockStage::In, MindlockStage::Rejected, TransitionAuth::ReviewerReject) => true,
        (MindlockStage::Out, MindlockStage::Rejected, TransitionAuth::ReviewerReject) => true,
        (MindlockStage::Work, MindlockStage::Rejected, TransitionAuth::ReviewerReject) => true,

        (MindlockStage::In, MindlockStage::Out, TransitionAuth::AgentMove) => true,
        (MindlockStage::Out, MindlockStage::In, TransitionAuth::AgentMove) => true,
        (MindlockStage::In, MindlockStage::Work, TransitionAuth::AgentMove) => true,
        (MindlockStage::Out, MindlockStage::Work, TransitionAuth::AgentMove) => true,
        (MindlockStage::Work, MindlockStage::In, TransitionAuth::AgentMove) => true,
        (MindlockStage::Work, MindlockStage::Out, TransitionAuth::AgentMove) => true,

        (MindlockStage::In, MindlockStage::PendingZar, TransitionAuth::AgentMove) => true,
        (MindlockStage::Out, MindlockStage::PendingZar, TransitionAuth::AgentMove) => true,
        (MindlockStage::Work, MindlockStage::PendingZar, TransitionAuth::AgentMove) => true,

        (MindlockStage::PendingZar, MindlockStage::Promoted, TransitionAuth::ZarApprove) => true,
        (MindlockStage::PendingZar, MindlockStage::Rejected, TransitionAuth::ZarReject) => true,

        _ => false,
    }
}

pub fn cleanup_eligible(
    stage: MindlockStage,
    age_days: u64,
    retention_days: u64,
) -> (result: bool)
    ensures result == spec_cleanup_eligible(stage, age_days as nat, retention_days as nat)
{
    is_terminal(stage) && age_days >= retention_days
}

// ── Proof lemmas ─────────────────────────────────────────────────────

/// Nothing can leave Rejected.
proof fn lemma_rejected_is_terminal(to: MindlockStage, auth: TransitionAuth)
    ensures !spec_valid_transition(MindlockStage::Rejected, to, auth)
{}

/// Nothing can leave Promoted.
proof fn lemma_promoted_is_terminal(to: MindlockStage, auth: TransitionAuth)
    ensures !spec_valid_transition(MindlockStage::Promoted, to, auth)
{}

/// PendingZar can only exit via ZarApprove or ZarReject.
proof fn lemma_pending_requires_zar(to: MindlockStage, auth: TransitionAuth)
    requires spec_valid_transition(MindlockStage::PendingZar, to, auth)
    ensures auth == TransitionAuth::ZarApprove || auth == TransitionAuth::ZarReject
{}

/// Promotion from staging requires ReviewerAllow.
proof fn lemma_staging_promote_requires_review(from: MindlockStage, auth: TransitionAuth)
    requires
        spec_is_staging(from),
        spec_valid_transition(from, MindlockStage::Promoted, auth),
    ensures auth == TransitionAuth::ReviewerAllow
{}

/// ZarApprove is only valid from PendingZar.
proof fn lemma_zar_approve_only_from_pending(from: MindlockStage, to: MindlockStage)
    requires spec_valid_transition(from, to, TransitionAuth::ZarApprove)
    ensures from == MindlockStage::PendingZar
{}

/// AgentMove never reaches Promoted or Rejected directly.
proof fn lemma_agent_move_stays_safe(from: MindlockStage, to: MindlockStage)
    requires spec_valid_transition(from, to, TransitionAuth::AgentMove)
    ensures to != MindlockStage::Promoted && to != MindlockStage::Rejected
{}

/// No single transition goes directly from staging to Promoted via ZarApprove
/// (must go through PendingZar first).
proof fn lemma_no_skip_to_promoted_via_zar(from: MindlockStage)
    requires spec_is_staging(from)
    ensures !spec_valid_transition(from, MindlockStage::Promoted, TransitionAuth::ZarApprove)
{}

/// Any staging state can escalate to PendingZar (via agent or reviewer).
proof fn lemma_escalation_always_valid_from_staging(from: MindlockStage)
    requires spec_is_staging(from)
    ensures
        spec_valid_transition(from, MindlockStage::PendingZar, TransitionAuth::ReviewerRequireZarApproval),
        spec_valid_transition(from, MindlockStage::PendingZar, TransitionAuth::AgentMove),
{}

// ── Source validation lemmas ─────────────────────────────────────────

/// Terminal states are never valid move sources.
proof fn lemma_terminal_not_valid_move_source(stage: MindlockStage)
    requires spec_is_terminal(stage)
    ensures !spec_is_valid_move_source(stage)
{}

/// Valid move sources are exactly staging + PendingZar.
proof fn lemma_valid_move_source_coverage(stage: MindlockStage)
    ensures spec_is_valid_move_source(stage) == (spec_is_staging(stage) || stage == MindlockStage::PendingZar)
{}

// ── Cleanup lemmas ───────────────────────────────────────────────────

/// Only terminal artifacts can be cleaned up (staging artifacts are never eligible).
proof fn lemma_staging_never_cleanup_eligible(stage: MindlockStage, age_days: nat, retention_days: nat)
    requires spec_is_staging(stage)
    ensures !spec_cleanup_eligible(stage, age_days, retention_days)
{}

/// PendingZar artifacts are never cleanup-eligible (they need human action).
proof fn lemma_pending_never_cleanup_eligible(age_days: nat, retention_days: nat)
    ensures !spec_cleanup_eligible(MindlockStage::PendingZar, age_days, retention_days)
{}

/// Young terminal artifacts are not cleanup-eligible.
proof fn lemma_young_terminal_not_eligible(stage: MindlockStage, retention_days: nat)
    requires retention_days > 0
    ensures !spec_cleanup_eligible(stage, 0, retention_days)
{}

/// Rejected artifacts at retention age are eligible.
proof fn lemma_rejected_at_retention_eligible(retention_days: nat)
    ensures spec_cleanup_eligible(MindlockStage::Rejected, retention_days, retention_days)
{}

} // verus!

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn staging_states() {
        assert!(is_staging(MindlockStage::In));
        assert!(is_staging(MindlockStage::Out));
        assert!(is_staging(MindlockStage::Work));
        assert!(!is_staging(MindlockStage::PendingZar));
        assert!(!is_staging(MindlockStage::Rejected));
        assert!(!is_staging(MindlockStage::Promoted));
    }

    #[test]
    fn terminal_states() {
        assert!(is_terminal(MindlockStage::Rejected));
        assert!(is_terminal(MindlockStage::Promoted));
        assert!(!is_terminal(MindlockStage::In));
        assert!(!is_terminal(MindlockStage::PendingZar));
    }

    #[test]
    fn valid_move_sources() {
        assert!(is_valid_move_source(MindlockStage::In));
        assert!(is_valid_move_source(MindlockStage::Out));
        assert!(is_valid_move_source(MindlockStage::Work));
        assert!(is_valid_move_source(MindlockStage::PendingZar));
        assert!(!is_valid_move_source(MindlockStage::Rejected));
        assert!(!is_valid_move_source(MindlockStage::Promoted));
    }

    #[test]
    fn reviewer_allow_promotes() {
        assert!(valid_transition(MindlockStage::In, MindlockStage::Promoted, TransitionAuth::ReviewerAllow));
        assert!(valid_transition(MindlockStage::Out, MindlockStage::Promoted, TransitionAuth::ReviewerAllow));
        assert!(valid_transition(MindlockStage::Work, MindlockStage::Promoted, TransitionAuth::ReviewerAllow));
    }

    #[test]
    fn rejected_is_terminal() {
        for auth in [TransitionAuth::ReviewerAllow, TransitionAuth::AgentMove, TransitionAuth::ZarApprove] {
            assert!(!valid_transition(MindlockStage::Rejected, MindlockStage::In, auth));
            assert!(!valid_transition(MindlockStage::Rejected, MindlockStage::Promoted, auth));
        }
    }

    #[test]
    fn promoted_is_terminal() {
        for auth in [TransitionAuth::ReviewerAllow, TransitionAuth::AgentMove, TransitionAuth::ZarApprove] {
            assert!(!valid_transition(MindlockStage::Promoted, MindlockStage::In, auth));
            assert!(!valid_transition(MindlockStage::Promoted, MindlockStage::Rejected, auth));
        }
    }

    #[test]
    fn pending_zar_only_exits_via_zar() {
        assert!(valid_transition(MindlockStage::PendingZar, MindlockStage::Promoted, TransitionAuth::ZarApprove));
        assert!(valid_transition(MindlockStage::PendingZar, MindlockStage::Rejected, TransitionAuth::ZarReject));
        assert!(!valid_transition(MindlockStage::PendingZar, MindlockStage::Promoted, TransitionAuth::ReviewerAllow));
        assert!(!valid_transition(MindlockStage::PendingZar, MindlockStage::In, TransitionAuth::AgentMove));
    }

    #[test]
    fn agent_move_never_promotes_or_rejects() {
        for from in [MindlockStage::In, MindlockStage::Out, MindlockStage::Work] {
            assert!(!valid_transition(from, MindlockStage::Promoted, TransitionAuth::AgentMove));
            assert!(!valid_transition(from, MindlockStage::Rejected, TransitionAuth::AgentMove));
        }
    }

    #[test]
    fn cleanup_eligibility() {
        assert!(cleanup_eligible(MindlockStage::Rejected, 30, 30));
        assert!(cleanup_eligible(MindlockStage::Rejected, 31, 30));
        assert!(!cleanup_eligible(MindlockStage::Rejected, 29, 30));
        assert!(!cleanup_eligible(MindlockStage::In, 100, 30));
        assert!(!cleanup_eligible(MindlockStage::PendingZar, 100, 30));
    }
}
