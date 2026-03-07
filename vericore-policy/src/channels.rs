//! Channel-to-tier mapping.
//!
//! Default mapping from communication channels to context/integrity tiers.
//! Config overrides (channel_map) remain in vericore-core since they involve
//! runtime configuration. This module provides the pure default mapping.

use vstd::prelude::*;
use crate::tiers::{ContextTier, IntegrityTier};

verus! {

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    TelegramPublic,
    TelegramFamily,
    TelegramDm,
    Terminal,
    Internal,
    Api,
    Moltbook,
}

// Spec versions for use in proofs

pub open spec fn spec_default_context(channel: Channel) -> ContextTier {
    match channel {
        Channel::TelegramPublic | Channel::Api | Channel::Moltbook => ContextTier::Public,
        Channel::TelegramFamily => ContextTier::Family,
        Channel::TelegramDm | Channel::Internal | Channel::Terminal => ContextTier::Private,
    }
}

pub open spec fn spec_integrity_for_context(context: ContextTier) -> IntegrityTier {
    match context {
        ContextTier::Public => IntegrityTier::Untrusted,
        ContextTier::Family => IntegrityTier::Reviewed,
        ContextTier::Private => IntegrityTier::Trusted,
    }
}

/// Default context tier for a channel (before config overrides).
pub fn default_context_for_channel(channel: Channel) -> (result: ContextTier)
    ensures result == spec_default_context(channel)
{
    match channel {
        Channel::TelegramPublic | Channel::Api | Channel::Moltbook => ContextTier::Public,
        Channel::TelegramFamily => ContextTier::Family,
        Channel::TelegramDm | Channel::Internal | Channel::Terminal => ContextTier::Private,
    }
}

/// Integrity tier derived from context tier.
/// Public -> Untrusted, Family -> Reviewed, Private -> Trusted.
pub fn integrity_for_context(context: ContextTier) -> (result: IntegrityTier)
    ensures result == spec_integrity_for_context(context)
{
    match context {
        ContextTier::Public => IntegrityTier::Untrusted,
        ContextTier::Family => IntegrityTier::Reviewed,
        ContextTier::Private => IntegrityTier::Trusted,
    }
}

/// Convenience: channel -> integrity in one step (using default context).
pub fn default_integrity_for_channel(channel: Channel) -> (result: IntegrityTier)
    ensures result == spec_integrity_for_context(spec_default_context(channel))
{
    integrity_for_context(default_context_for_channel(channel))
}

// ── Proof lemmas ──────────────────────────────────────────────────────

/// All public-facing channels map to Untrusted integrity.
proof fn lemma_public_channels_untrusted()
    ensures
        spec_integrity_for_context(spec_default_context(Channel::TelegramPublic)) == IntegrityTier::Untrusted,
        spec_integrity_for_context(spec_default_context(Channel::Api)) == IntegrityTier::Untrusted,
        spec_integrity_for_context(spec_default_context(Channel::Moltbook)) == IntegrityTier::Untrusted,
{}

/// All private channels map to Trusted integrity.
proof fn lemma_private_channels_trusted()
    ensures
        spec_integrity_for_context(spec_default_context(Channel::TelegramDm)) == IntegrityTier::Trusted,
        spec_integrity_for_context(spec_default_context(Channel::Internal)) == IntegrityTier::Trusted,
        spec_integrity_for_context(spec_default_context(Channel::Terminal)) == IntegrityTier::Trusted,
{}

/// No channel maps to a context tier above Private (i.e., Private is the ceiling).
proof fn lemma_no_channel_maps_above_private(ch: Channel)
    ensures crate::tiers::context_rank(spec_default_context(ch)) <= crate::tiers::context_rank(ContextTier::Private)
{}

/// Context-to-integrity mapping is injective: distinct contexts yield distinct integrity.
proof fn lemma_integrity_injective_on_context(a: ContextTier, b: ContextTier)
    ensures a != b ==> spec_integrity_for_context(a) != spec_integrity_for_context(b)
{}

} // verus!

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_channels_are_untrusted() {
        for ch in [Channel::TelegramPublic, Channel::Api, Channel::Moltbook] {
            assert_eq!(default_context_for_channel(ch), ContextTier::Public);
            assert_eq!(default_integrity_for_channel(ch), IntegrityTier::Untrusted);
        }
    }

    #[test]
    fn family_channel_is_reviewed() {
        assert_eq!(default_context_for_channel(Channel::TelegramFamily), ContextTier::Family);
        assert_eq!(default_integrity_for_channel(Channel::TelegramFamily), IntegrityTier::Reviewed);
    }

    #[test]
    fn private_channels_are_trusted() {
        for ch in [Channel::TelegramDm, Channel::Internal, Channel::Terminal] {
            assert_eq!(default_context_for_channel(ch), ContextTier::Private);
            assert_eq!(default_integrity_for_channel(ch), IntegrityTier::Trusted);
        }
    }
}
