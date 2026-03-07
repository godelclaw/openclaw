//! LLM bridge contract.
//!
//! Pure decision functions for validating bridge request/response invariants.
//! These prove properties about the GatewayLlmClient ↔ TS completions bridge
//! protocol that prevent the class of bugs seen during initial bridge deployment
//! (silent provider routing, missing metadata, prompt mode confusion).

use vstd::prelude::*;

verus! {

/// What kind of LLM call is being made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallKind {
    DriverTurn,
    SecurityReview,
    MemoryRefine,
}

/// Whether the caller is the driver agent or the security reviewer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptMode {
    Driver,
    Reviewer,
}

// ── Request validation ───────────────────────────────────────────────

/// Spec: a bridge request is well-formed if it has messages, a system prompt,
/// and valid generation parameters.
pub open spec fn spec_request_well_formed(
    has_messages: bool,
    has_system_prompt: bool,
    max_tokens_positive: bool,
    temperature_valid: bool,
) -> bool {
    has_messages && has_system_prompt && max_tokens_positive && temperature_valid
}

pub fn request_well_formed(
    has_messages: bool,
    has_system_prompt: bool,
    max_tokens_positive: bool,
    temperature_valid: bool,
) -> (result: bool)
    ensures result == spec_request_well_formed(
        has_messages, has_system_prompt, max_tokens_positive, temperature_valid,
    )
{
    has_messages && has_system_prompt && max_tokens_positive && temperature_valid
}

// ── Response validation ──────────────────────────────────────────────

/// Spec: a bridge response is usable if ok=true, has at least one choice,
/// and includes model metadata for audit.
pub open spec fn spec_response_usable(
    ok: bool,
    has_choices: bool,
    has_model_meta: bool,
) -> bool {
    ok && has_choices && has_model_meta
}

pub fn response_usable(
    ok: bool,
    has_choices: bool,
    has_model_meta: bool,
) -> (result: bool)
    ensures result == spec_response_usable(ok, has_choices, has_model_meta)
{
    ok && has_choices && has_model_meta
}

// ── Call kind / prompt mode consistency ──────────────────────────────

/// Spec: SecurityReview calls must use Reviewer prompt mode.
/// DriverTurn calls must use Driver prompt mode.
pub open spec fn spec_call_kind_prompt_mode_consistent(
    kind: CallKind,
    mode: PromptMode,
) -> bool {
    match kind {
        CallKind::SecurityReview => mode == PromptMode::Reviewer,
        CallKind::DriverTurn => mode == PromptMode::Driver,
        CallKind::MemoryRefine => true, // either mode acceptable
    }
}

pub fn call_kind_prompt_mode_consistent(
    kind: CallKind,
    mode: PromptMode,
) -> (result: bool)
    ensures result == spec_call_kind_prompt_mode_consistent(kind, mode)
{
    match kind {
        CallKind::SecurityReview => matches!(mode, PromptMode::Reviewer),
        CallKind::DriverTurn => matches!(mode, PromptMode::Driver),
        CallKind::MemoryRefine => true,
    }
}

// ── Session key requirement ──────────────────────────────────────────

/// Spec: DriverTurn calls require a session key for proper session resolution.
pub open spec fn spec_session_key_required(
    kind: CallKind,
    has_session_key: bool,
) -> bool {
    match kind {
        CallKind::DriverTurn => has_session_key,
        _ => true, // other call kinds don't need session context
    }
}

pub fn session_key_required(
    kind: CallKind,
    has_session_key: bool,
) -> (result: bool)
    ensures result == spec_session_key_required(kind, has_session_key)
{
    match kind {
        CallKind::DriverTurn => has_session_key,
        _ => true,
    }
}

// ── Proof lemmas ─────────────────────────────────────────────────────

/// Error responses are never usable.
proof fn lemma_error_response_not_usable(has_choices: bool, has_model_meta: bool)
    ensures !spec_response_usable(false, has_choices, has_model_meta)
{}

/// Responses without choices are never usable.
proof fn lemma_empty_response_not_usable(ok: bool, has_model_meta: bool)
    ensures !spec_response_usable(ok, false, has_model_meta)
{}

/// Responses without model metadata are never usable.
proof fn lemma_missing_model_meta_not_usable(ok: bool, has_choices: bool)
    ensures !spec_response_usable(ok, has_choices, false)
{}

/// SecurityReview always requires Reviewer prompt mode.
proof fn lemma_security_review_implies_reviewer_mode(mode: PromptMode)
    ensures spec_call_kind_prompt_mode_consistent(CallKind::SecurityReview, mode)
        ==> mode == PromptMode::Reviewer
{}

/// DriverTurn always requires Driver prompt mode.
proof fn lemma_driver_turn_implies_driver_mode(mode: PromptMode)
    ensures spec_call_kind_prompt_mode_consistent(CallKind::DriverTurn, mode)
        ==> mode == PromptMode::Driver
{}

/// DriverTurn always requires a session key.
proof fn lemma_driver_turn_requires_session(has_key: bool)
    ensures spec_session_key_required(CallKind::DriverTurn, has_key)
        ==> has_key
{}

/// Call kind does not affect request well-formedness (orthogonal concerns).
proof fn lemma_call_kind_orthogonal_to_request(
    has_messages: bool,
    has_system_prompt: bool,
    max_tokens_positive: bool,
    temperature_valid: bool,
)
    ensures
        spec_request_well_formed(has_messages, has_system_prompt, max_tokens_positive, temperature_valid)
        == (has_messages && has_system_prompt && max_tokens_positive && temperature_valid)
{}

} // verus!

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn well_formed_request_requires_all_fields() {
        assert!(request_well_formed(true, true, true, true));
        assert!(!request_well_formed(false, true, true, true));
        assert!(!request_well_formed(true, false, true, true));
        assert!(!request_well_formed(true, true, false, true));
        assert!(!request_well_formed(true, true, true, false));
    }

    #[test]
    fn usable_response_requires_ok_and_choices_and_meta() {
        assert!(response_usable(true, true, true));
        assert!(!response_usable(false, true, true));
        assert!(!response_usable(true, false, true));
        assert!(!response_usable(true, true, false));
    }

    #[test]
    fn security_review_must_use_reviewer() {
        assert!(call_kind_prompt_mode_consistent(CallKind::SecurityReview, PromptMode::Reviewer));
        assert!(!call_kind_prompt_mode_consistent(CallKind::SecurityReview, PromptMode::Driver));
    }

    #[test]
    fn driver_turn_must_use_driver() {
        assert!(call_kind_prompt_mode_consistent(CallKind::DriverTurn, PromptMode::Driver));
        assert!(!call_kind_prompt_mode_consistent(CallKind::DriverTurn, PromptMode::Reviewer));
    }

    #[test]
    fn driver_turn_requires_session_key() {
        assert!(session_key_required(CallKind::DriverTurn, true));
        assert!(!session_key_required(CallKind::DriverTurn, false));
    }

    #[test]
    fn memory_refine_flexible_mode() {
        assert!(call_kind_prompt_mode_consistent(CallKind::MemoryRefine, PromptMode::Driver));
        assert!(call_kind_prompt_mode_consistent(CallKind::MemoryRefine, PromptMode::Reviewer));
    }

    #[test]
    fn non_driver_no_session_requirement() {
        assert!(session_key_required(CallKind::SecurityReview, false));
        assert!(session_key_required(CallKind::MemoryRefine, false));
    }
}
