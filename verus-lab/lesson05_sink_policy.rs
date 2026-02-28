/// lesson05_sink_policy.rs
///
/// Proves the output-boundary (sink) policy:
///   - every response goes through a sink check before leaving the agent
///   - the check is: accumulated_taint.can_flow_to(channel_context)
///   - if denied, the response is replaced with a denial message (never leaked)
///
/// This is the formal I/O boundary from the GödelClaw cognition/action split:
/// internal reasoning is free; what exits is gated.
use vstd::prelude::*;

verus! {

#[derive(PartialEq, Eq, Clone, Copy)]
pub enum ContextTier {
    Public,
    Family,
    Private,
}

pub open spec fn rank(c: ContextTier) -> nat {
    match c {
        ContextTier::Public  => 0,
        ContextTier::Family  => 1,
        ContextTier::Private => 2,
    }
}

pub open spec fn can_flow_to(label: ContextTier, sink: ContextTier) -> bool {
    rank(label) <= rank(sink)
}

/// The two possible outcomes of a sink check.
#[derive(PartialEq, Eq, Clone, Copy)]
pub enum SinkVerdict {
    Release,  // content exits
    Suppress, // content is replaced with denial message; nothing leaks
}

/// Model of the sink check in turn.rs `apply_sink_policy`.
pub open spec fn sink_check(label: ContextTier, sink: ContextTier) -> SinkVerdict {
    if can_flow_to(label, sink) {
        SinkVerdict::Release
    } else {
        SinkVerdict::Suppress
    }
}

// ── Core safety property ──────────────────────────────────────────────────

/// The sink never releases data whose label exceeds the sink's tier.
/// This is the central non-leakage guarantee.
proof fn lemma_sink_never_releases_above_tier(label: ContextTier, sink: ContextTier)
    ensures
        sink_check(label, sink) == SinkVerdict::Release ==> can_flow_to(label, sink),
{}

/// Equivalently: if data cannot flow to the sink, it is always suppressed.
proof fn lemma_sink_suppresses_when_denied(label: ContextTier, sink: ContextTier)
    ensures
        !can_flow_to(label, sink) ==> sink_check(label, sink) == SinkVerdict::Suppress,
{}

// ── Channel examples matching vericore.toml ───────────────────────────────

/// telegram_dm  -> Private  (full authority)
/// telegram_family -> Family
/// telegram_public / moltbook / api -> Public

proof fn lemma_private_label_only_exits_private_sink()
    ensures
        // Private data: only the DM (Private sink) releases it
        sink_check(ContextTier::Private, ContextTier::Private) == SinkVerdict::Release,
        sink_check(ContextTier::Private, ContextTier::Family)  == SinkVerdict::Suppress,
        sink_check(ContextTier::Private, ContextTier::Public)  == SinkVerdict::Suppress,
{}

proof fn lemma_family_label_exits_family_and_private_sinks()
    ensures
        sink_check(ContextTier::Family, ContextTier::Private) == SinkVerdict::Release,
        sink_check(ContextTier::Family, ContextTier::Family)  == SinkVerdict::Release,
        sink_check(ContextTier::Family, ContextTier::Public)  == SinkVerdict::Suppress,
{}

proof fn lemma_public_label_exits_any_sink()
    ensures
        sink_check(ContextTier::Public, ContextTier::Public)  == SinkVerdict::Release,
        sink_check(ContextTier::Public, ContextTier::Family)  == SinkVerdict::Release,
        sink_check(ContextTier::Public, ContextTier::Private) == SinkVerdict::Release,
{}

// ── Moltbook scenario: the concrete leak-prevention case ─────────────────

/// Moltbook is a public sink. If the agent read a private file during
/// the turn, the taint is Private, and the sink check suppresses the
/// response before it reaches Moltbook. No private data leaks.
proof fn lemma_moltbook_cannot_receive_private_taint()
    ensures
        // moltbook_sink = Public
        sink_check(ContextTier::Private, ContextTier::Public) == SinkVerdict::Suppress,
{}

// ── Suppression is total: no partial leak ────────────────────────────────

/// When Suppress fires, the *entire* response is replaced — not truncated.
/// We model this as: the output is a fixed denial string (represented here
/// as a boolean "is_denial") rather than the original content.
///
/// The proof obligation: for every (label, sink) pair where Suppress fires,
/// the output is the denial message, not the original content.
pub open spec fn output_is_denial(verdict: SinkVerdict) -> bool {
    verdict == SinkVerdict::Suppress
}

proof fn lemma_suppressed_output_is_never_original(label: ContextTier, sink: ContextTier)
    ensures
        !can_flow_to(label, sink) ==> output_is_denial(sink_check(label, sink)),
{}

// ── Sink check is deterministic and total ────────────────────────────────

/// For every (label, sink) pair, the verdict is always exactly one of
/// Release or Suppress — never undefined.
proof fn lemma_sink_check_total_and_deterministic(label: ContextTier, sink: ContextTier)
    ensures
        sink_check(label, sink) == SinkVerdict::Release
        || sink_check(label, sink) == SinkVerdict::Suppress,
{}

fn main() {}

} // verus!
