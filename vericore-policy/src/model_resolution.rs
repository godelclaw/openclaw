//! Model resolution policy reference spec.
//!
//! Spec-level predicates for candidate list validity, cross-provider gating,
//! and resolution metadata. This is a proved reference spec — the TS fallback
//! resolver should mirror these invariants in its tests.
//!
//! This module does NOT prove that the TS runtime builds valid lists.
//! It proves: IF a candidate list satisfies these predicates, THEN certain
//! structural safety properties hold.

use vstd::prelude::*;

verus! {

/// Opaque model reference: provider + model identity.
/// At runtime these correspond to string provider/model pairs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelRef {
    pub provider: u64,
    pub model: u64,
}

/// The result of model resolution: what was configured vs what was used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolutionResult {
    pub configured: ModelRef,
    pub resolved: ModelRef,
}

/// Alert severity for fallback events.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlertSeverity {
    None,
    ModelFallback,
    ProviderFallback,
}

// ── Candidate list spec predicates ───────────────────────────────
// Written as small helpers rather than relying on Seq methods
// that may not be available or may cause Verus friction.

/// The candidate list is non-empty.
pub open spec fn spec_nonempty(candidates: Seq<ModelRef>) -> bool {
    candidates.len() > 0
}

/// The configured primary is the first candidate.
pub open spec fn spec_primary_first(
    primary: ModelRef,
    candidates: Seq<ModelRef>,
) -> bool {
    candidates.len() > 0 && candidates[0] == primary
}

/// No duplicate entries in the candidate list.
pub open spec fn spec_no_duplicates(candidates: Seq<ModelRef>) -> bool {
    forall|i: int, j: int|
        0 <= i < candidates.len() && 0 <= j < candidates.len() && i != j
        ==> candidates[i] != candidates[j]
}

/// Every candidate shares the given provider.
pub open spec fn spec_all_same_provider(
    candidates: Seq<ModelRef>,
    provider: u64,
) -> bool {
    forall|i: int|
        0 <= i < candidates.len()
        ==> candidates[i].provider == provider
}

/// Combined policy predicate: a candidate list is policy-ok if it is
/// non-empty, primary-first, duplicate-free, and (when cross-provider
/// is disabled) all candidates share the primary's provider.
pub open spec fn spec_candidate_policy_ok(
    primary: ModelRef,
    candidates: Seq<ModelRef>,
    allow_cross_provider: bool,
) -> bool {
    spec_nonempty(candidates)
    && spec_primary_first(primary, candidates)
    && spec_no_duplicates(candidates)
    && (allow_cross_provider || spec_all_same_provider(candidates, primary.provider))
}

// ── Cross-provider gate ──────────────────────────────────────────

/// Spec: whether a candidate provider passes the cross-provider policy gate.
pub open spec fn spec_cross_provider_allowed(
    candidate_provider: u64,
    primary_provider: u64,
    allow_cross_provider: bool,
) -> bool {
    allow_cross_provider || candidate_provider == primary_provider
}

/// Exec: whether a candidate provider is allowed under the cross-provider policy.
pub fn cross_provider_allowed(
    candidate_provider: u64,
    primary_provider: u64,
    allow_cross_provider: bool,
) -> (result: bool)
    ensures result == spec_cross_provider_allowed(
        candidate_provider, primary_provider, allow_cross_provider,
    )
{
    allow_cross_provider || candidate_provider == primary_provider
}

// ── Resolution metadata ──────────────────────────────────────────

pub open spec fn spec_model_changed(r: ResolutionResult) -> bool {
    r.resolved.provider != r.configured.provider
        || r.resolved.model != r.configured.model
}

pub open spec fn spec_provider_changed(r: ResolutionResult) -> bool {
    r.resolved.provider != r.configured.provider
}

pub open spec fn spec_alert_required(r: ResolutionResult) -> bool {
    spec_model_changed(r)
}

pub open spec fn spec_alert_severity(r: ResolutionResult) -> AlertSeverity {
    if spec_provider_changed(r) {
        AlertSeverity::ProviderFallback
    } else if spec_model_changed(r) {
        AlertSeverity::ModelFallback
    } else {
        AlertSeverity::None
    }
}

pub fn model_changed(r: ResolutionResult) -> (result: bool)
    ensures result == spec_model_changed(r)
{
    r.resolved.provider != r.configured.provider
        || r.resolved.model != r.configured.model
}

pub fn provider_changed(r: ResolutionResult) -> (result: bool)
    ensures result == spec_provider_changed(r)
{
    r.resolved.provider != r.configured.provider
}

pub fn alert_required(r: ResolutionResult) -> (result: bool)
    ensures result == spec_alert_required(r)
{
    model_changed(r)
}

pub fn alert_severity(r: ResolutionResult) -> (result: AlertSeverity)
    ensures result == spec_alert_severity(r)
{
    if provider_changed(r) {
        AlertSeverity::ProviderFallback
    } else if model_changed(r) {
        AlertSeverity::ModelFallback
    } else {
        AlertSeverity::None
    }
}

// ── Proof lemmas ─────────────────────────────────────────────────
// These unpack the spec predicates. They are definition-level proofs,
// not deep theorems — their value is as regression guards that break
// if someone changes the spec in incompatible ways.

/// A policy-ok list always has the primary at index 0.
proof fn lemma_primary_always_first(
    primary: ModelRef,
    candidates: Seq<ModelRef>,
    allow_cross_provider: bool,
)
    requires spec_candidate_policy_ok(primary, candidates, allow_cross_provider)
    ensures candidates[0] == primary
{}

/// A policy-ok list always contains the primary.
proof fn lemma_primary_in_candidates(
    primary: ModelRef,
    candidates: Seq<ModelRef>,
    allow_cross_provider: bool,
)
    requires spec_candidate_policy_ok(primary, candidates, allow_cross_provider)
    ensures exists|i: int| 0 <= i < candidates.len() && candidates[i] == primary
{
    // Witness: index 0
    assert(candidates[0] == primary);
}

/// When cross-provider is disabled and the list is policy-ok,
/// every candidate shares the primary's provider.
proof fn lemma_gate_restricts_provider(
    primary: ModelRef,
    candidates: Seq<ModelRef>,
)
    requires spec_candidate_policy_ok(primary, candidates, false)
    ensures spec_all_same_provider(candidates, primary.provider)
{}

/// When cross-provider is disabled, picking any candidate from a
/// policy-ok list cannot change the provider.
proof fn lemma_valid_list_prevents_provider_change(
    primary: ModelRef,
    candidates: Seq<ModelRef>,
    i: int,
)
    requires
        spec_candidate_policy_ok(primary, candidates, false),
        0 <= i < candidates.len(),
    ensures
        candidates[i].provider == primary.provider
{}

/// When cross-provider gate is disabled, only same-provider passes.
proof fn lemma_cross_provider_gate_same_provider(
    candidate_provider: u64,
    primary_provider: u64,
)
    ensures spec_cross_provider_allowed(candidate_provider, primary_provider, false)
        ==> candidate_provider == primary_provider
{}

/// Provider change is strictly stronger than model change.
proof fn lemma_provider_change_implies_model_change(r: ResolutionResult)
    ensures spec_provider_changed(r) ==> spec_model_changed(r)
{}

/// Provider change always yields the highest alert severity.
proof fn lemma_provider_change_highest_severity(r: ResolutionResult)
    ensures spec_provider_changed(r)
        ==> spec_alert_severity(r) == AlertSeverity::ProviderFallback
{}

} // verus!

#[cfg(test)]
mod tests {
    use super::*;

    const ANTHROPIC: u64 = 1;
    const OPENROUTER: u64 = 2;
    const OPUS: u64 = 100;
    const SONNET: u64 = 101;
    const GROK: u64 = 200;

    fn anthropic_opus() -> ModelRef {
        ModelRef {
            provider: ANTHROPIC,
            model: OPUS,
        }
    }

    fn anthropic_sonnet() -> ModelRef {
        ModelRef {
            provider: ANTHROPIC,
            model: SONNET,
        }
    }

    fn openrouter_grok() -> ModelRef {
        ModelRef {
            provider: OPENROUTER,
            model: GROK,
        }
    }

    #[test]
    fn no_fallback_no_alert() {
        let r = ResolutionResult {
            configured: anthropic_opus(),
            resolved: anthropic_opus(),
        };
        assert!(!model_changed(r));
        assert!(!provider_changed(r));
        assert!(!alert_required(r));
        assert_eq!(alert_severity(r), AlertSeverity::None);
    }

    #[test]
    fn same_provider_model_fallback() {
        let r = ResolutionResult {
            configured: anthropic_opus(),
            resolved: anthropic_sonnet(),
        };
        assert!(model_changed(r));
        assert!(!provider_changed(r));
        assert!(alert_required(r));
        assert_eq!(alert_severity(r), AlertSeverity::ModelFallback);
    }

    #[test]
    fn cross_provider_fallback_detected() {
        let r = ResolutionResult {
            configured: anthropic_opus(),
            resolved: openrouter_grok(),
        };
        assert!(model_changed(r));
        assert!(provider_changed(r));
        assert!(alert_required(r));
        assert_eq!(alert_severity(r), AlertSeverity::ProviderFallback);
    }

    #[test]
    fn cross_provider_gate_blocks_when_disabled() {
        assert!(!cross_provider_allowed(OPENROUTER, ANTHROPIC, false));
        assert!(cross_provider_allowed(ANTHROPIC, ANTHROPIC, false));
    }

    #[test]
    fn cross_provider_gate_allows_when_enabled() {
        assert!(cross_provider_allowed(OPENROUTER, ANTHROPIC, true));
        assert!(cross_provider_allowed(ANTHROPIC, ANTHROPIC, true));
    }
}
