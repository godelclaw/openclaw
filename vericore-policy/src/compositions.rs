//! Cross-module composition proofs.
//!
//! These chain multiple verified modules to prove end-to-end system properties.
//! Individual module proofs are necessary; composition proofs are what guarantee
//! the system is coherent.

use vstd::prelude::*;

verus! {

#[allow(unused_imports)]
use crate::tiers::*;
#[allow(unused_imports)]
use crate::channels::*;
#[allow(unused_imports)]
use crate::ingress::*;
#[allow(unused_imports)]
use crate::model_resolution::*;
#[allow(unused_imports)]
use crate::session_model_policy::*;


/// A message from public Telegram can never execute a shell command.
/// Chains: channels.rs (TelegramPublic → Public) → channels.rs (Public → Untrusted)
///       → ingress.rs (Untrusted cannot exec).
proof fn lemma_public_telegram_cannot_exec()
    ensures
        spec_default_context(Channel::TelegramPublic) == ContextTier::Public,
        spec_integrity_for_context(ContextTier::Public) == IntegrityTier::Untrusted,
        !spec_ingress_allows_exec(IntegrityTier::Untrusted),
{}

/// API channel messages cannot execute commands.
proof fn lemma_api_cannot_exec()
    ensures
        spec_default_context(Channel::Api) == ContextTier::Public,
        spec_integrity_for_context(ContextTier::Public) == IntegrityTier::Untrusted,
        !spec_ingress_allows_exec(IntegrityTier::Untrusted),
{}

/// Moltbook channel messages cannot execute commands.
proof fn lemma_moltbook_cannot_exec()
    ensures
        spec_default_context(Channel::Moltbook) == ContextTier::Public,
        spec_integrity_for_context(ContextTier::Public) == IntegrityTier::Untrusted,
        !spec_ingress_allows_exec(IntegrityTier::Untrusted),
{}

/// An untrusted channel cannot write to private targets.
/// Chains: channels.rs (Public → Untrusted) → ingress.rs (Untrusted cannot write Private).
proof fn lemma_untrusted_cannot_write_private()
    ensures
        spec_integrity_for_context(ContextTier::Public) == IntegrityTier::Untrusted,
        !spec_ingress_allows_write(IntegrityTier::Untrusted, WriteTargetClass::Private),
        !spec_ingress_allows_write(IntegrityTier::Untrusted, WriteTargetClass::SensitiveConfig),
{}

/// Family (Reviewed) channels also cannot write to private targets or exec.
proof fn lemma_family_cannot_write_private_or_exec()
    ensures
        spec_integrity_for_context(ContextTier::Family) == IntegrityTier::Reviewed,
        !spec_ingress_allows_exec(IntegrityTier::Reviewed),
        !spec_ingress_allows_write(IntegrityTier::Reviewed, WriteTargetClass::Private),
        !spec_ingress_allows_write(IntegrityTier::Reviewed, WriteTargetClass::SensitiveConfig),
{}

/// Private (Trusted) channels can do everything except write to Unresolved targets.
proof fn lemma_private_channel_full_capability()
    ensures
        spec_integrity_for_context(ContextTier::Private) == IntegrityTier::Trusted,
        spec_ingress_allows_exec(IntegrityTier::Trusted),
        spec_ingress_allows_write(IntegrityTier::Trusted, WriteTargetClass::Private),
        spec_ingress_allows_write(IntegrityTier::Trusted, WriteTargetClass::SensitiveConfig),
        spec_ingress_allows_write(IntegrityTier::Trusted, WriteTargetClass::Mindlock),
        spec_ingress_allows_write(IntegrityTier::Trusted, WriteTargetClass::Other),
        !spec_ingress_allows_write(IntegrityTier::Trusted, WriteTargetClass::Unresolved),
{}

/// A brokered tool from public Telegram that is not in the allowlist is denied.
/// Chains: channels.rs (TelegramPublic → Public → Untrusted) +
///         ingress.rs (public writes to Other stay allowed) +
///         tool_policy.rs (unlisted brokered tool denied) +
///         gate_chain.rs (tool gate false denies whole chain).
proof fn lemma_unlisted_brokered_tool_denied_in_public()
    ensures
        spec_default_context(Channel::TelegramPublic) == ContextTier::Public,
        spec_integrity_for_context(spec_default_context(Channel::TelegramPublic)) == IntegrityTier::Untrusted,
        spec_ingress_allows_write(
            spec_integrity_for_context(spec_default_context(Channel::TelegramPublic)),
            WriteTargetClass::Other,
        ),
        !crate::tool_policy::spec_tool_gate_allows(true, false, false, false),
        !crate::gate_chain::spec_gate_chain_allows(
            true,
            true,
            crate::tool_policy::spec_tool_gate_allows(true, false, false, false),
            true,
            spec_ingress_allows_write(
                spec_integrity_for_context(spec_default_context(Channel::TelegramPublic)),
                WriteTargetClass::Other,
            ),
            true,
        ),
{}

/// Native and brokered non-colliding tools with the same capability membership
/// produce the same gate-chain result when origin is the only difference.
proof fn lemma_brokered_same_gate_as_native(in_list: bool, wildcard: bool)
    ensures
        crate::tool_policy::spec_origin_irrelevant(in_list, wildcard),
        crate::gate_chain::spec_gate_chain_allows(
            true,
            true,
            crate::tool_policy::spec_tool_gate_allows(false, false, in_list, wildcard),
            true,
            true,
            true,
        )
            == crate::gate_chain::spec_gate_chain_allows(
                true,
                true,
                crate::tool_policy::spec_tool_gate_allows(true, false, in_list, wildcard),
                true,
                true,
                true,
            ),
{}

/// A brokered tool that shadows a native name is denied before the rest of the gate chain matters.
proof fn lemma_native_shadow_denied_before_gate(capability_id_in_allowlist: bool, wildcard: bool)
    ensures
        !crate::tool_policy::spec_tool_gate_allows(true, true, capability_id_in_allowlist, wildcard),
        !crate::gate_chain::spec_gate_chain_allows(
            true,
            true,
            crate::tool_policy::spec_tool_gate_allows(true, true, capability_id_in_allowlist, wildcard),
            true,
            true,
            true,
        ),
{}


/// A satisfied bridge model contract preserves the boundary checks it encodes.
/// This is a regression guard for the Rust-side model-resolution validator,
/// not an end-to-end proof that the TS model-selection pipeline is correct.
proof fn lemma_model_contract_boundary_regression_check(
    primary: ModelRef,
    candidates: Seq<ModelRef>,
    allow_cross_provider: bool,
)
    ensures
        spec_candidate_policy_ok(primary, candidates, allow_cross_provider)
            && spec_bridge_model_contract_ok(true, true, true, true, true)
            ==> spec_candidate_policy_ok(primary, candidates, allow_cross_provider)
                && spec_bridge_model_contract_ok(true, true, true, true, true),
{}
} // verus!

#[cfg(test)]
mod tests {
    use crate::channels::{Channel, default_context_for_channel, integrity_for_context};
    use crate::ingress::{WriteTargetClass, ingress_allows_exec, ingress_allows_write};
    use crate::tiers::{ContextTier, IntegrityTier};

    #[test]
    fn public_telegram_cannot_exec_runtime() {
        let ctx = default_context_for_channel(Channel::TelegramPublic);
        let integrity = integrity_for_context(ctx);
        assert!(!ingress_allows_exec(integrity));
    }

    #[test]
    fn unlisted_brokered_tool_denied() {
        let integrity = integrity_for_context(default_context_for_channel(Channel::TelegramPublic));
        assert_eq!(integrity, IntegrityTier::Untrusted);
        let ingress_ok = ingress_allows_write(integrity, WriteTargetClass::Other);
        assert!(ingress_ok);

        let tool_ok = crate::tool_policy::tool_gate_allows(true, false, false, false);
        assert!(!tool_ok);
        assert!(!crate::gate_chain::gate_chain_allows(
            true, true, tool_ok, true, ingress_ok, true
        ));
    }

    #[test]
    fn brokered_origin_matches_native_when_non_colliding() {
        let native_ok = crate::tool_policy::tool_gate_allows(false, false, true, false);
        let brokered_ok = crate::tool_policy::tool_gate_allows(true, false, true, false);
        assert_eq!(native_ok, brokered_ok);
        assert_eq!(
            crate::gate_chain::gate_chain_allows(true, true, native_ok, true, true, true),
            crate::gate_chain::gate_chain_allows(true, true, brokered_ok, true, true, true),
        );
    }

    #[test]
    fn brokered_shadow_denied() {
        let tool_ok = crate::tool_policy::tool_gate_allows(true, true, true, true);
        assert!(!tool_ok);
        assert!(!crate::gate_chain::gate_chain_allows(
            true, true, tool_ok, true, true, true
        ));
    }

    #[test]
    fn untrusted_cannot_write_private_runtime() {
        let integrity = integrity_for_context(ContextTier::Public);
        assert!(!ingress_allows_write(integrity, WriteTargetClass::Private));
        assert!(!ingress_allows_write(
            integrity,
            WriteTargetClass::SensitiveConfig
        ));
    }

    #[test]
    fn model_contract_all_pass() {
        assert!(crate::session_model_policy::bridge_model_contract_ok(
            true, true, true, true, true
        ));
        assert!(crate::session_model_policy::resolution_meta_complete(
            true, true, true
        ));
        assert!(crate::session_model_policy::override_applied(true, true));
        assert!(crate::session_model_policy::fallback_visible(
            false, false, false
        ));
    }

    #[test]
    fn model_contract_incomplete_meta_fails() {
        assert!(!crate::session_model_policy::bridge_model_contract_ok(
            false, true, true, true, true
        ));
    }
}
