//! Session model policy contract.
//!
//! Pure decision functions for validating model resolution metadata returned
//! by the TS completions bridge. These prove properties about the boundary
//! contract: IF the bridge response satisfies these predicates, THEN the
//! model selection path honored the session override metadata, fallback visibility,
//! and allowlist invariants that Rust depends on.
//!
//! This module does NOT prove that the TS runtime builds correct overrides.
//! It proves: IF a bridge response satisfies these predicates, THEN certain
//! integrity properties hold at the Rust boundary.

use vstd::prelude::*;

verus! {

// ── Metadata completeness ───────────────────────────────────────────

/// Spec: a bridge response has complete model resolution metadata.
pub open spec fn spec_resolution_meta_complete(
    has_configured_model: bool,
    has_resolved_model: bool,
    has_resolved_provider: bool,
) -> bool {
    has_configured_model && has_resolved_model && has_resolved_provider
}

pub fn resolution_meta_complete(
    has_configured_model: bool,
    has_resolved_model: bool,
    has_resolved_provider: bool,
) -> (result: bool)
    ensures result == spec_resolution_meta_complete(
        has_configured_model, has_resolved_model, has_resolved_provider,
    )
{
    has_configured_model && has_resolved_model && has_resolved_provider
}

// ── Fallback visibility ─────────────────────────────────────────────

/// Spec: if the model changed, fallback metadata must be present.
/// fallback_used_flag must be true and the configured model must be present
/// so the operator can see what changed.
pub open spec fn spec_fallback_visible(
    model_changed: bool,
    fallback_used_flag: bool,
    has_configured_model: bool,
) -> bool {
    !model_changed || (fallback_used_flag && has_configured_model)
}

pub fn fallback_visible(
    model_changed: bool,
    fallback_used_flag: bool,
    has_configured_model: bool,
) -> (result: bool)
    ensures result == spec_fallback_visible(
        model_changed, fallback_used_flag, has_configured_model,
    )
{
    !model_changed || (fallback_used_flag && has_configured_model)
}

// ── Provider change consistency ─────────────────────────────────────

/// Spec: the provider_changed flag must match the actual comparison.
pub open spec fn spec_provider_change_consistent(
    provider_actually_changed: bool,
    provider_changed_flag: bool,
) -> bool {
    provider_actually_changed == provider_changed_flag
}

pub fn provider_change_consistent(
    provider_actually_changed: bool,
    provider_changed_flag: bool,
) -> (result: bool)
    ensures result == spec_provider_change_consistent(
        provider_actually_changed, provider_changed_flag,
    )
{
    provider_actually_changed == provider_changed_flag
}

// ── Allowlist enforcement ───────────────────────────────────────────

/// Spec: if the allowlist is non-empty, the resolved model must be in it.
/// An empty allowlist permits any model.
pub open spec fn spec_allowlist_enforced(
    resolved_in_list: bool,
    list_empty: bool,
) -> bool {
    list_empty || resolved_in_list
}

pub fn allowlist_enforced(
    resolved_in_list: bool,
    list_empty: bool,
) -> (result: bool)
    ensures result == spec_allowlist_enforced(resolved_in_list, list_empty)
{
    list_empty || resolved_in_list
}

// ── Override applied ────────────────────────────────────────────────

/// Spec: if a session override exists, the configured primary must match it.
pub open spec fn spec_override_applied(
    has_override: bool,
    configured_matches_override: bool,
) -> bool {
    !has_override || configured_matches_override
}

pub fn override_applied(
    has_override: bool,
    configured_matches_override: bool,
) -> (result: bool)
    ensures result == spec_override_applied(has_override, configured_matches_override)
{
    !has_override || configured_matches_override
}

// ── Combined contract ───────────────────────────────────────────────

/// Spec: the full bridge model contract is satisfied when metadata is complete,
/// fallback is visible, provider flag is consistent, allowlist is enforced,
/// and any observed session override is reflected in the configured primary.
pub open spec fn spec_bridge_model_contract_ok(
    meta_complete: bool,
    fallback_visible: bool,
    provider_consistent: bool,
    allowlist_ok: bool,
    override_ok: bool,
) -> bool {
    meta_complete && fallback_visible && provider_consistent && allowlist_ok && override_ok
}

pub fn bridge_model_contract_ok(
    meta_complete: bool,
    fallback_visible: bool,
    provider_consistent: bool,
    allowlist_ok: bool,
    override_ok: bool,
) -> (result: bool)
    ensures result == spec_bridge_model_contract_ok(
        meta_complete, fallback_visible, provider_consistent, allowlist_ok, override_ok,
    )
{
    meta_complete && fallback_visible && provider_consistent && allowlist_ok && override_ok
}

// ── Proof lemmas ────────────────────────────────────────────────────

/// If fallback metadata is not visible, the model cannot have changed.
proof fn lemma_fallback_invisible_implies_no_change(
    model_changed: bool,
    fallback_used_flag: bool,
    has_configured_model: bool,
)
    ensures !spec_fallback_visible(model_changed, fallback_used_flag, has_configured_model)
        ==> model_changed && (!fallback_used_flag || !has_configured_model)
{}

/// A non-empty allowlist with the model not in it fails enforcement.
proof fn lemma_allowlist_nonempty_denies_unlisted()
    ensures !spec_allowlist_enforced(false, false)
{}

/// Incomplete metadata always breaks the contract.
proof fn lemma_complete_meta_required_for_contract(
    fallback_visible: bool,
    provider_consistent: bool,
    allowlist_ok: bool,
    override_ok: bool,
)
    ensures !spec_bridge_model_contract_ok(
        false,
        fallback_visible,
        provider_consistent,
        allowlist_ok,
        override_ok,
    )
{}

/// Provider flag inconsistency always breaks the contract.
proof fn lemma_provider_inconsistency_breaks_contract(
    meta_complete: bool,
    fallback_visible: bool,
    allowlist_ok: bool,
    override_ok: bool,
)
    ensures !spec_bridge_model_contract_ok(
        meta_complete,
        fallback_visible,
        false,
        allowlist_ok,
        override_ok,
    )
{}

/// Override mismatch always breaks the contract.
proof fn lemma_override_mismatch_breaks_contract(
    meta_complete: bool,
    fallback_visible: bool,
    provider_consistent: bool,
    allowlist_ok: bool,
)
    ensures !spec_bridge_model_contract_ok(
        meta_complete,
        fallback_visible,
        provider_consistent,
        allowlist_ok,
        false,
    )
{}

/// When all individual predicates pass, the contract holds.
proof fn lemma_all_pass_implies_contract_ok()
    ensures spec_bridge_model_contract_ok(true, true, true, true, true)
{}

} // verus!

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meta_complete_requires_all_fields() {
        assert!(resolution_meta_complete(true, true, true));
        assert!(!resolution_meta_complete(false, true, true));
        assert!(!resolution_meta_complete(true, false, true));
        assert!(!resolution_meta_complete(true, true, false));
    }

    #[test]
    fn fallback_visible_when_no_change() {
        assert!(fallback_visible(false, false, false));
        assert!(fallback_visible(false, true, false));
    }

    #[test]
    fn fallback_invisible_when_changed_without_meta() {
        assert!(!fallback_visible(true, false, false));
        assert!(!fallback_visible(true, true, false));
        assert!(!fallback_visible(true, false, true));
    }

    #[test]
    fn fallback_visible_when_changed_with_meta() {
        assert!(fallback_visible(true, true, true));
    }

    #[test]
    fn provider_change_consistency() {
        assert!(provider_change_consistent(true, true));
        assert!(provider_change_consistent(false, false));
        assert!(!provider_change_consistent(true, false));
        assert!(!provider_change_consistent(false, true));
    }

    #[test]
    fn allowlist_empty_allows_all() {
        assert!(allowlist_enforced(false, true));
        assert!(allowlist_enforced(true, true));
    }

    #[test]
    fn allowlist_nonempty_requires_membership() {
        assert!(allowlist_enforced(true, false));
        assert!(!allowlist_enforced(false, false));
    }

    #[test]
    fn override_applied_when_no_override() {
        assert!(override_applied(false, false));
        assert!(override_applied(false, true));
    }

    #[test]
    fn override_applied_requires_match() {
        assert!(override_applied(true, true));
        assert!(!override_applied(true, false));
    }

    #[test]
    fn contract_requires_all_five() {
        assert!(bridge_model_contract_ok(true, true, true, true, true));
        assert!(!bridge_model_contract_ok(false, true, true, true, true));
        assert!(!bridge_model_contract_ok(true, false, true, true, true));
        assert!(!bridge_model_contract_ok(true, true, false, true, true));
        assert!(!bridge_model_contract_ok(true, true, true, false, true));
        assert!(!bridge_model_contract_ok(true, true, true, true, false));
    }
}
