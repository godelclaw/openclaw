//! Tool authorization policy.
//!
//! Pure decision functions for tool-level authorization in the gate chain.
//! Key invariants:
//! - allowlist/wildcard membership controls authorization,
//! - native-name collisions block brokered tools before authorization,
//! - for non-colliding tools, origin alone does not change the tool-gate result.

use vstd::prelude::*;

verus! {

// ── Tool authorization ───────────────────────────────────────────────

/// Spec: a tool is authorized if its capability_id is in the allowlist
/// or the allowlist contains a wildcard.
pub open spec fn spec_tool_authorized(
    capability_id_in_allowlist: bool,
    wildcard_present: bool,
) -> bool {
    wildcard_present || capability_id_in_allowlist
}

pub fn tool_authorized(
    capability_id_in_allowlist: bool,
    wildcard_present: bool,
) -> (result: bool)
    ensures result == spec_tool_authorized(capability_id_in_allowlist, wildcard_present)
{
    wildcard_present || capability_id_in_allowlist
}

// ── Native name shadowing ────────────────────────────────────────────

/// Spec: whether a tool passes the native-name collision guard.
/// A brokered tool that reuses a native display name fails this guard.
pub open spec fn spec_native_shadow_clear(
    is_brokered: bool,
    display_name_matches_native: bool,
) -> bool {
    !(is_brokered && display_name_matches_native)
}

pub fn native_shadow_clear(
    is_brokered: bool,
    display_name_matches_native: bool,
) -> (result: bool)
    ensures result == spec_native_shadow_clear(is_brokered, display_name_matches_native)
{
    !(is_brokered && display_name_matches_native)
}

// ── Combined tool gate ───────────────────────────────────────────────

/// Spec: the combined tool gate result used by the action gate chain.
/// A tool must both clear the native-shadow guard and pass allowlist authorization.
pub open spec fn spec_tool_gate_allows(
    is_brokered: bool,
    display_name_matches_native: bool,
    capability_id_in_allowlist: bool,
    wildcard_present: bool,
) -> bool {
    spec_native_shadow_clear(is_brokered, display_name_matches_native)
        && spec_tool_authorized(capability_id_in_allowlist, wildcard_present)
}

pub fn tool_gate_allows(
    is_brokered: bool,
    display_name_matches_native: bool,
    capability_id_in_allowlist: bool,
    wildcard_present: bool,
) -> (result: bool)
    ensures result == spec_tool_gate_allows(
        is_brokered,
        display_name_matches_native,
        capability_id_in_allowlist,
        wildcard_present,
    )
{
    native_shadow_clear(is_brokered, display_name_matches_native)
        && tool_authorized(capability_id_in_allowlist, wildcard_present)
}

// ── Origin-irrelevant authorization ──────────────────────────────────

/// Spec: for non-colliding tools, origin alone does not change the gate result.
/// Native and brokered variants with the same capability membership authorize identically.
pub open spec fn spec_origin_irrelevant(
    capability_id_in_allowlist: bool,
    wildcard: bool,
) -> bool {
    spec_tool_gate_allows(false, false, capability_id_in_allowlist, wildcard)
        == spec_tool_gate_allows(true, false, capability_id_in_allowlist, wildcard)
}

// ── Proof lemmas ─────────────────────────────────────────────────────

/// Wildcard allows all tools regardless of capability_id.
proof fn lemma_wildcard_allows_all(capability_id_in_allowlist: bool)
    ensures spec_tool_authorized(capability_id_in_allowlist, true)
{}

/// Unknown capability (not in allowlist, no wildcard) is denied.
proof fn lemma_unknown_capability_denied()
    ensures !spec_tool_authorized(false, false)
{}

/// Listed capability is always authorized.
proof fn lemma_listed_capability_authorized(wildcard: bool)
    ensures spec_tool_authorized(true, wildcard)
{}

/// Native and brokered non-colliding tools with the same capability membership
/// get the same combined tool-gate result.
proof fn lemma_origin_irrelevant_non_colliding(in_list: bool, wildcard: bool)
    ensures spec_origin_irrelevant(in_list, wildcard)
{}

/// Brokered tool with native name collision is always denied by the collision guard.
proof fn lemma_native_shadow_always_denied()
    ensures !spec_native_shadow_clear(true, true)
{}

/// Non-brokered tools are never shadow-denied.
proof fn lemma_native_tools_not_shadow_denied(display_name_matches: bool)
    ensures spec_native_shadow_clear(false, display_name_matches)
{}

/// Brokered tools with non-colliding names are not shadow-denied.
proof fn lemma_non_colliding_brokered_not_denied()
    ensures spec_native_shadow_clear(true, false)
{}

/// A native-name collision blocks the full tool gate, even if authorization passes.
proof fn lemma_shadow_collision_blocks_tool_gate(
    capability_id_in_allowlist: bool,
    wildcard_present: bool,
)
    ensures !spec_tool_gate_allows(true, true, capability_id_in_allowlist, wildcard_present)
{}

} // verus!

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wildcard_allows_everything() {
        assert!(tool_authorized(true, true));
        assert!(tool_authorized(false, true));
    }

    #[test]
    fn no_wildcard_requires_listing() {
        assert!(tool_authorized(true, false));
        assert!(!tool_authorized(false, false));
    }

    #[test]
    fn shadow_denied_for_brokered_native_collision() {
        assert!(!native_shadow_clear(true, true));
    }

    #[test]
    fn shadow_not_denied_for_native_tools() {
        assert!(native_shadow_clear(false, true));
        assert!(native_shadow_clear(false, false));
    }

    #[test]
    fn shadow_not_denied_for_non_colliding_brokered() {
        assert!(native_shadow_clear(true, false));
    }

    #[test]
    fn origin_irrelevant_for_non_colliding_tools() {
        assert_eq!(
            tool_gate_allows(false, false, true, false),
            tool_gate_allows(true, false, true, false),
        );
        assert_eq!(
            tool_gate_allows(false, false, false, true),
            tool_gate_allows(true, false, false, true),
        );
    }

    #[test]
    fn shadow_collision_blocks_tool_gate_even_if_listed() {
        assert!(!tool_gate_allows(true, true, true, false));
        assert!(!tool_gate_allows(true, true, true, true));
    }
}
