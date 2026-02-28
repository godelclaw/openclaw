/// lesson04_taint_propagation.rs
///
/// Proves the taint-join semantics used in turn.rs:
///   response_label = join over all action labels in the turn
///
/// Key property: once a turn touches private data, the response is
/// permanently private-labeled — no subsequent public action can
/// "un-taint" it. The join is monotone and the lattice has no
/// declassification path inside a single turn.
use vstd::prelude::*;

verus! {

#[derive(PartialEq, Eq, Clone, Copy)]
pub enum ContextTier {
    Public,   // rank 0
    Family,   // rank 1
    Private,  // rank 2
}

pub open spec fn rank(c: ContextTier) -> nat {
    match c {
        ContextTier::Public  => 0,
        ContextTier::Family  => 1,
        ContextTier::Private => 2,
    }
}

/// Taint join: take the higher-ranked (more sensitive) label.
/// This mirrors turn.rs `join_label`.
pub open spec fn join(a: ContextTier, b: ContextTier) -> ContextTier {
    if rank(a) >= rank(b) { a } else { b }
}

/// A label can flow to a sink only if the label is no more sensitive
/// than the sink's context. Mirrors `can_flow_to` / `sink_allows_label`.
pub open spec fn can_flow_to(label: ContextTier, sink: ContextTier) -> bool {
    rank(label) <= rank(sink)
}

// ── Lattice properties of join ────────────────────────────────────────────

proof fn lemma_join_commutative(a: ContextTier, b: ContextTier)
    ensures join(a, b) == join(b, a),
{}

proof fn lemma_join_associative(a: ContextTier, b: ContextTier, c: ContextTier)
    ensures join(join(a, b), c) == join(a, join(b, c)),
{
    // All cases are rank comparisons on {0,1,2}; Verus handles by enumeration.
    assert(rank(join(join(a, b), c)) == rank(join(a, join(b, c))));
}

proof fn lemma_join_idempotent(a: ContextTier)
    ensures join(a, a) == a,
{}

/// join is the least upper bound: both inputs are ≤ the result.
proof fn lemma_join_upper_bound(a: ContextTier, b: ContextTier)
    ensures
        rank(a) <= rank(join(a, b)),
        rank(b) <= rank(join(a, b)),
{}

// ── Monotonicity: join never decreases the label ──────────────────────────

/// Adding any action label to an existing taint can only raise or hold it —
/// it can never lower it. This is the formal "no un-taint" property.
proof fn lemma_join_monotone(existing: ContextTier, new_label: ContextTier)
    ensures rank(join(existing, new_label)) >= rank(existing),
{}

/// Corollary: if we start at Public and accumulate labels, the result
/// is always at least as sensitive as any individual label we saw.
proof fn lemma_accumulation_dominates_each(a: ContextTier, b: ContextTier, c: ContextTier)
    ensures
        rank(join(join(ContextTier::Public, a), b)) >= rank(a),
        rank(join(join(ContextTier::Public, a), b)) >= rank(b),
{
    // rank(join(join(Public,a),b)) = max(max(0,rank(a)),rank(b))
    //                              = max(rank(a),rank(b))
    assert(rank(join(ContextTier::Public, a)) == rank(a));
}

// ── Sink-flow properties ──────────────────────────────────────────────────

/// If the accumulated taint cannot flow to a sink, no amount of
/// additional public actions will rescue it — the denial is permanent
/// for this turn.
proof fn lemma_taint_denial_is_permanent(
    taint: ContextTier,
    sink: ContextTier,
    next_label: ContextTier,
)
    requires
        !can_flow_to(taint, sink),
        rank(next_label) <= rank(taint),  // next action is no more sensitive
    ensures
        !can_flow_to(join(taint, next_label), sink),
{
    // join(taint, next_label) has rank >= rank(taint) > rank(sink)
    assert(rank(join(taint, next_label)) >= rank(taint));
}

/// Private taint can never flow to a public sink.
proof fn lemma_private_taint_blocks_public_sink()
    ensures
        !can_flow_to(ContextTier::Private, ContextTier::Public),
        !can_flow_to(ContextTier::Private, ContextTier::Family),
        can_flow_to(ContextTier::Private, ContextTier::Private),
        can_flow_to(ContextTier::Family, ContextTier::Family),
        can_flow_to(ContextTier::Family, ContextTier::Private),
        !can_flow_to(ContextTier::Family, ContextTier::Public),
        can_flow_to(ContextTier::Public, ContextTier::Public),
{}

/// Once a turn reads a private file, the response label is Private
/// regardless of how many public web fetches follow.
proof fn lemma_private_read_taints_whole_turn()
    ensures
        // start Public, read private file (Private label), then fetch public (Public label)
        join(join(ContextTier::Public, ContextTier::Private), ContextTier::Public)
            == ContextTier::Private,
        // that result cannot flow to a public sink
        !can_flow_to(
            join(join(ContextTier::Public, ContextTier::Private), ContextTier::Public),
            ContextTier::Public,
        ),
{}

fn main() {}

} // verus!
