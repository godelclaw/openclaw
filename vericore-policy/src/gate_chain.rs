//! Gate chain composition.
//!
//! Maps directly to `core.rs:50-80` — the nested if-chain of gates.
//! The gate chain is a strict conjunction: all gates must allow for the
//! action to proceed.

use vstd::prelude::*;

verus! {

/// Spec: the gate chain result is the conjunction of all individual gates.
/// Order: schedule → action_kind → tool_id → skill_id → ingress → primitive.
/// First deny breaks the chain (short-circuit), but the result is equivalent
/// to AND of all gates.
pub open spec fn spec_gate_chain_allows(
    schedule_ok: bool,
    action_kind_ok: bool,
    tool_id_ok: bool,
    skill_id_ok: bool,
    ingress_ok: bool,
    primitive_ok: bool,
) -> bool {
    schedule_ok && action_kind_ok && tool_id_ok && skill_id_ok && ingress_ok && primitive_ok
}

pub fn gate_chain_allows(
    schedule_ok: bool,
    action_kind_ok: bool,
    tool_id_ok: bool,
    skill_id_ok: bool,
    ingress_ok: bool,
    primitive_ok: bool,
) -> (result: bool)
    ensures result == spec_gate_chain_allows(
        schedule_ok, action_kind_ok, tool_id_ok, skill_id_ok, ingress_ok, primitive_ok,
    )
{
    schedule_ok && action_kind_ok && tool_id_ok && skill_id_ok && ingress_ok && primitive_ok
}

// ── Proof lemmas ─────────────────────────────────────────────────────

/// Any single deny means the chain denies.
proof fn lemma_any_deny_means_chain_denies(
    schedule_ok: bool,
    action_kind_ok: bool,
    tool_id_ok: bool,
    skill_id_ok: bool,
    ingress_ok: bool,
    primitive_ok: bool,
)
    ensures
        (!schedule_ok ==> !spec_gate_chain_allows(schedule_ok, action_kind_ok, tool_id_ok, skill_id_ok, ingress_ok, primitive_ok)),
        (!action_kind_ok ==> !spec_gate_chain_allows(schedule_ok, action_kind_ok, tool_id_ok, skill_id_ok, ingress_ok, primitive_ok)),
        (!tool_id_ok ==> !spec_gate_chain_allows(schedule_ok, action_kind_ok, tool_id_ok, skill_id_ok, ingress_ok, primitive_ok)),
        (!skill_id_ok ==> !spec_gate_chain_allows(schedule_ok, action_kind_ok, tool_id_ok, skill_id_ok, ingress_ok, primitive_ok)),
        (!ingress_ok ==> !spec_gate_chain_allows(schedule_ok, action_kind_ok, tool_id_ok, skill_id_ok, ingress_ok, primitive_ok)),
        (!primitive_ok ==> !spec_gate_chain_allows(schedule_ok, action_kind_ok, tool_id_ok, skill_id_ok, ingress_ok, primitive_ok)),
{}

/// Chain allow implies every individual gate allowed (monotone).
proof fn lemma_chain_monotone(
    schedule_ok: bool,
    action_kind_ok: bool,
    tool_id_ok: bool,
    skill_id_ok: bool,
    ingress_ok: bool,
    primitive_ok: bool,
)
    requires spec_gate_chain_allows(schedule_ok, action_kind_ok, tool_id_ok, skill_id_ok, ingress_ok, primitive_ok)
    ensures
        schedule_ok,
        action_kind_ok,
        tool_id_ok,
        skill_id_ok,
        ingress_ok,
        primitive_ok,
{}

/// Completeness: all individual gates pass → chain allows.
proof fn lemma_all_allow_means_chain_allows()
    ensures spec_gate_chain_allows(true, true, true, true, true, true)
{}

/// Ingress deny blocks even if primitive allows.
proof fn lemma_ingress_deny_blocks_even_if_primitive_allows(
    schedule_ok: bool,
    action_kind_ok: bool,
    tool_id_ok: bool,
    skill_id_ok: bool,
)
    ensures !spec_gate_chain_allows(schedule_ok, action_kind_ok, tool_id_ok, skill_id_ok, false, true)
{}

/// Schedule deny blocks everything (schedule is first and absolute).
proof fn lemma_schedule_deny_blocks_everything(
    action_kind_ok: bool,
    tool_id_ok: bool,
    skill_id_ok: bool,
    ingress_ok: bool,
    primitive_ok: bool,
)
    ensures !spec_gate_chain_allows(false, action_kind_ok, tool_id_ok, skill_id_ok, ingress_ok, primitive_ok)
{}

} // verus!

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_allow_passes() {
        assert!(gate_chain_allows(true, true, true, true, true, true));
    }

    #[test]
    fn any_single_deny_fails() {
        assert!(!gate_chain_allows(false, true, true, true, true, true));
        assert!(!gate_chain_allows(true, false, true, true, true, true));
        assert!(!gate_chain_allows(true, true, false, true, true, true));
        assert!(!gate_chain_allows(true, true, true, false, true, true));
        assert!(!gate_chain_allows(true, true, true, true, false, true));
        assert!(!gate_chain_allows(true, true, true, true, true, false));
    }
}
