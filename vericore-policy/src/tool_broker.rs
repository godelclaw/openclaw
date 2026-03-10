//! Tool broker contract.
//!
//! Pure decision functions for validating broker request/response invariants.
//! These prove properties about the GatewayToolBrokerClient ↔ TS tool broker
//! protocol that prevent capability drift and silent tool loss.
//!
//! This module is a parsed-field contract, not a raw JSON or string-parser proof.
//! In particular, capability-ID checks here talk about required semantic pieces
//! after parsing, not full validation of the runtime `mcp:<server>:<tool>` text format.

use vstd::prelude::*;

verus! {

// ── Request validation ───────────────────────────────────────────────

/// Spec: a broker request is well-formed if it has a request ID,
/// a call kind, and (for call_tool) a capability ID and arguments.
pub open spec fn spec_tool_request_well_formed(
    has_request_id: bool,
    has_call_kind: bool,
    has_capability_id: bool,
    has_arguments: bool,
    is_call_tool: bool,
) -> bool {
    has_request_id && has_call_kind &&
    (is_call_tool ==> (has_capability_id && has_arguments))
}

pub fn tool_request_well_formed(
    has_request_id: bool,
    has_call_kind: bool,
    has_capability_id: bool,
    has_arguments: bool,
    is_call_tool: bool,
) -> (result: bool)
    ensures result == spec_tool_request_well_formed(
        has_request_id, has_call_kind, has_capability_id, has_arguments, is_call_tool,
    )
{
    has_request_id && has_call_kind &&
    (!is_call_tool || (has_capability_id && has_arguments))
}

// ── Response validation ──────────────────────────────────────────────

/// Spec: a broker response is usable if ok=true and (for call_tool) has a result.
pub open spec fn spec_tool_response_usable(
    ok: bool,
    has_result: bool,
    is_call_tool: bool,
) -> bool {
    ok && (is_call_tool ==> has_result)
}

pub fn tool_response_usable(
    ok: bool,
    has_result: bool,
    is_call_tool: bool,
) -> (result: bool)
    ensures result == spec_tool_response_usable(ok, has_result, is_call_tool)
{
    ok && (!is_call_tool || has_result)
}

// ── Capability ID stability ──────────────────────────────────────────

/// Spec: a capability ID is stable when all parsed semantic components are present.
/// This is an abstraction of the runtime namespace/server/tool split, not a parser proof.
pub open spec fn spec_capability_id_stable(
    has_mcp_prefix: bool,
    has_server_name: bool,
    has_tool_name: bool,
) -> bool {
    has_mcp_prefix && has_server_name && has_tool_name
}

pub fn capability_id_stable(
    has_mcp_prefix: bool,
    has_server_name: bool,
    has_tool_name: bool,
) -> (result: bool)
    ensures result == spec_capability_id_stable(has_mcp_prefix, has_server_name, has_tool_name)
{
    has_mcp_prefix && has_server_name && has_tool_name
}

// ── Proof lemmas ─────────────────────────────────────────────────────

/// Error responses are never usable.
proof fn lemma_error_response_not_usable(has_result: bool, is_call_tool: bool)
    ensures !spec_tool_response_usable(false, has_result, is_call_tool)
{}

/// call_tool without a result is not usable.
proof fn lemma_missing_result_not_usable(ok: bool)
    ensures !spec_tool_response_usable(ok, false, true)
{}

/// list_tools response does not require a result field.
proof fn lemma_list_tools_no_result_needed()
    ensures spec_tool_response_usable(true, false, false)
{}

/// call_tool request requires capability_id and arguments.
proof fn lemma_call_tool_requires_capability_and_args(
    has_request_id: bool,
    has_call_kind: bool,
)
    ensures
        !spec_tool_request_well_formed(has_request_id, has_call_kind, false, true, true),
        !spec_tool_request_well_formed(has_request_id, has_call_kind, true, false, true),
{}

/// list_tools request does not require capability_id or arguments.
proof fn lemma_list_tools_no_capability_needed()
    ensures spec_tool_request_well_formed(true, true, false, false, false)
{}

/// Missing any parsed capability component makes the ID unstable.
proof fn lemma_unstable_capability_id(
    has_mcp_prefix: bool,
    has_server_name: bool,
    has_tool_name: bool,
)
    ensures
        (!has_mcp_prefix ==> !spec_capability_id_stable(has_mcp_prefix, has_server_name, has_tool_name)),
        (!has_server_name ==> !spec_capability_id_stable(has_mcp_prefix, has_server_name, has_tool_name)),
        (!has_tool_name ==> !spec_capability_id_stable(has_mcp_prefix, has_server_name, has_tool_name)),
{}

} // verus!

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn well_formed_list_tools_request() {
        assert!(tool_request_well_formed(true, true, false, false, false));
    }

    #[test]
    fn well_formed_call_tool_request() {
        assert!(tool_request_well_formed(true, true, true, true, true));
        assert!(!tool_request_well_formed(true, true, false, true, true));
        assert!(!tool_request_well_formed(true, true, true, false, true));
    }

    #[test]
    fn usable_response_requires_ok() {
        assert!(!tool_response_usable(false, true, true));
        assert!(!tool_response_usable(false, false, false));
    }

    #[test]
    fn call_tool_response_requires_result() {
        assert!(tool_response_usable(true, true, true));
        assert!(!tool_response_usable(true, false, true));
    }

    #[test]
    fn list_tools_response_no_result_needed() {
        assert!(tool_response_usable(true, false, false));
    }

    #[test]
    fn stable_capability_id_requires_all_parts() {
        assert!(capability_id_stable(true, true, true));
        assert!(!capability_id_stable(false, true, true));
        assert!(!capability_id_stable(true, false, true));
        assert!(!capability_id_stable(true, true, false));
    }
}
