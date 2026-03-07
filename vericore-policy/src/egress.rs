//! Egress (outbound) flow control.
//!
//! Determines whether information can flow from a source context tier
//! to a sink context tier, and whether security review is required.
//! Maps to the runtime's `can_flow_to` check in `policy.rs::check_action()`
//! for Respond actions.

use vstd::prelude::*;
use crate::tiers::ContextTier;

verus! {

/// Spec: egress is allowed if the source context can flow to the sink.
/// Information flows upward in secrecy: Public → Family → Private.
/// A response from context X to channel Y is allowed if rank(X) >= rank(Y),
/// i.e., the source is at least as secret as the sink.
///
/// Wait — this needs careful checking against the runtime.
/// In `policy.rs::check_action()` for Respond, the check is:
///   context.can_read(response_channel_context)
/// which is: rank(response_channel) <= rank(context)
///
/// So: Private context (rank 2) can respond to Public channel (rank 0) — allowed.
/// But the runtime actually DENIES this! The test `private_cannot_respond_on_public_channel`
/// shows Private context responding to TelegramPublic is DENIED.
///
/// Let me re-read... The check is actually on the FLOW direction:
///   The context tier of the STIMULUS channel must be able to flow to the
///   RESPONSE channel. Private data shouldn't flow to public sinks.
///
/// Looking at core.rs test: stimulus is TelegramDm (Private), response channel
/// is TelegramPublic. The deny means: Private context cannot RESPOND on Public.
/// This is egress control: don't let private-context responses leak to public.
///
/// The correct model: egress_allowed(source, sink) iff source can_flow_to sink,
/// i.e., rank(source) <= rank(sink). Public can flow to anything. Private can
/// only flow to Private.
pub open spec fn spec_egress_allowed(source: ContextTier, sink: ContextTier) -> bool {
    crate::tiers::context_rank(source) <= crate::tiers::context_rank(sink)
}

pub fn egress_allowed(source: ContextTier, sink: ContextTier) -> (result: bool)
    ensures result == spec_egress_allowed(source, sink)
{
    source.can_flow_to(sink)
}

/// Spec: security review is needed when information crosses from a higher
/// secrecy tier to a lower one AND security review is enabled.
pub open spec fn spec_needs_security_review(
    source: ContextTier,
    sink: ContextTier,
    review_enabled: bool,
) -> bool {
    review_enabled && !spec_egress_allowed(source, sink)
}

pub fn needs_security_review(
    source: ContextTier,
    sink: ContextTier,
    review_enabled: bool,
) -> (result: bool)
    ensures result == spec_needs_security_review(source, sink, review_enabled)
{
    review_enabled && !egress_allowed(source, sink)
}

// ── Proof lemmas ─────────────────────────────────────────────────────

/// Same tier always allows egress.
proof fn lemma_same_tier_always_ok(tier: ContextTier)
    ensures spec_egress_allowed(tier, tier)
{}

/// Upward flow (Public → Private) always allowed.
proof fn lemma_upward_flow_ok()
    ensures
        spec_egress_allowed(ContextTier::Public, ContextTier::Private),
        spec_egress_allowed(ContextTier::Public, ContextTier::Family),
        spec_egress_allowed(ContextTier::Family, ContextTier::Private),
{}

/// Private → Public is denied (the core egress invariant).
proof fn lemma_private_to_public_denied()
    ensures !spec_egress_allowed(ContextTier::Private, ContextTier::Public)
{}

/// Private → Family is also denied.
proof fn lemma_private_to_family_denied()
    ensures !spec_egress_allowed(ContextTier::Private, ContextTier::Family)
{}

/// Family → Public is denied.
proof fn lemma_family_to_public_denied()
    ensures !spec_egress_allowed(ContextTier::Family, ContextTier::Public)
{}

/// Same tier never triggers security review.
proof fn lemma_same_tier_no_review(tier: ContextTier, enabled: bool)
    ensures !spec_needs_security_review(tier, tier, enabled)
{}

/// Private response to public sink triggers review when enabled.
proof fn lemma_private_public_needs_review()
    ensures spec_needs_security_review(ContextTier::Private, ContextTier::Public, true)
{}

/// Review disabled skips all reviews.
proof fn lemma_review_disabled_skips(source: ContextTier, sink: ContextTier)
    ensures !spec_needs_security_review(source, sink, false)
{}

/// Egress is transitive: if a→b ok and b→c ok, then a→c ok.
proof fn lemma_egress_transitive(a: ContextTier, b: ContextTier, c: ContextTier)
    ensures
        (spec_egress_allowed(a, b) && spec_egress_allowed(b, c))
            ==> spec_egress_allowed(a, c)
{}

} // verus!

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_tier_allowed() {
        assert!(egress_allowed(ContextTier::Public, ContextTier::Public));
        assert!(egress_allowed(ContextTier::Family, ContextTier::Family));
        assert!(egress_allowed(ContextTier::Private, ContextTier::Private));
    }

    #[test]
    fn private_to_public_denied() {
        assert!(!egress_allowed(ContextTier::Private, ContextTier::Public));
    }

    #[test]
    fn public_to_private_allowed() {
        assert!(egress_allowed(ContextTier::Public, ContextTier::Private));
    }

    #[test]
    fn review_triggered_on_cross_tier() {
        assert!(needs_security_review(ContextTier::Private, ContextTier::Public, true));
        assert!(!needs_security_review(ContextTier::Private, ContextTier::Public, false));
        assert!(!needs_security_review(ContextTier::Public, ContextTier::Public, true));
    }
}
