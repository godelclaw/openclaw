//! Cross-module composition proofs.
//!
//! These chain multiple verified modules to prove end-to-end system properties.
//! Individual module proofs are necessary; composition proofs are what guarantee
//! the system is coherent.

use vstd::prelude::*;

verus! {

use crate::tiers::*;
use crate::channels::*;
use crate::ingress::*;

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

} // verus!

#[cfg(test)]
mod tests {
    use crate::channels::{Channel, default_context_for_channel, integrity_for_context};
    use crate::tiers::ContextTier;
    use crate::ingress::{ingress_allows_exec, ingress_allows_write, WriteTargetClass};

    #[test]
    fn public_telegram_cannot_exec_runtime() {
        let ctx = default_context_for_channel(Channel::TelegramPublic);
        let integrity = integrity_for_context(ctx);
        assert!(!ingress_allows_exec(integrity));
    }

    #[test]
    fn untrusted_cannot_write_private_runtime() {
        let integrity = integrity_for_context(ContextTier::Public);
        assert!(!ingress_allows_write(integrity, WriteTargetClass::Private));
        assert!(!ingress_allows_write(integrity, WriteTargetClass::SensitiveConfig));
    }
}
