//! Context tiers, integrity tiers, and the dual-lattice flow label.
//!
//! These are the core types of VeriCore's information-flow model:
//! - Secrecy (ContextTier): Public < Family < Private
//! - Integrity (IntegrityTier): Untrusted < Reviewed < Trusted
//! - FlowLabel joins: secrecy rises (max), integrity drops (min)

use vstd::prelude::*;

verus! {

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextTier {
    Public,
    Family,
    Private,
}

pub open spec fn context_rank(c: ContextTier) -> nat {
    match c {
        ContextTier::Public => 0,
        ContextTier::Family => 1,
        ContextTier::Private => 2,
    }
}

impl ContextTier {
    pub fn rank(self) -> (r: u8)
        ensures r == context_rank(self)
    {
        match self {
            ContextTier::Public => 0,
            ContextTier::Family => 1,
            ContextTier::Private => 2,
        }
    }

    pub fn can_read(self, target: ContextTier) -> (result: bool)
        ensures result == (context_rank(target) <= context_rank(self))
    {
        target.rank() <= self.rank()
    }

    pub fn can_write(self, target: ContextTier) -> (result: bool)
        ensures result == (context_rank(target) <= context_rank(self))
    {
        self.can_read(target)
    }

    pub fn can_flow_to(self, target: ContextTier) -> (result: bool)
        ensures result == (context_rank(self) <= context_rank(target))
    {
        self.rank() <= target.rank()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntegrityTier {
    Untrusted,
    Reviewed,
    Trusted,
}

pub open spec fn integrity_rank(i: IntegrityTier) -> nat {
    match i {
        IntegrityTier::Untrusted => 0,
        IntegrityTier::Reviewed => 1,
        IntegrityTier::Trusted => 2,
    }
}

impl IntegrityTier {
    pub fn rank(self) -> (r: u8)
        ensures r == integrity_rank(self)
    {
        match self {
            IntegrityTier::Untrusted => 0,
            IntegrityTier::Reviewed => 1,
            IntegrityTier::Trusted => 2,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FlowLabel {
    pub secrecy: ContextTier,
    pub integrity: IntegrityTier,
}

/// Spec-level max of two context tiers by rank.
pub open spec fn spec_context_max(a: ContextTier, b: ContextTier) -> ContextTier {
    if context_rank(a) >= context_rank(b) { a } else { b }
}

/// Spec-level min of two integrity tiers by rank.
pub open spec fn spec_integrity_min(a: IntegrityTier, b: IntegrityTier) -> IntegrityTier {
    if integrity_rank(a) <= integrity_rank(b) { a } else { b }
}

/// Spec-level join for FlowLabel.
pub open spec fn spec_join(a: FlowLabel, b: FlowLabel) -> FlowLabel {
    FlowLabel {
        secrecy: spec_context_max(a.secrecy, b.secrecy),
        integrity: spec_integrity_min(a.integrity, b.integrity),
    }
}

impl FlowLabel {
    pub fn new(secrecy: ContextTier, integrity: IntegrityTier) -> (result: FlowLabel)
        ensures result.secrecy == secrecy && result.integrity == integrity
    {
        FlowLabel { secrecy, integrity }
    }

    /// Conservative label join: secrecy rises (max), integrity drops (min).
    pub fn join(self, other: FlowLabel) -> (result: FlowLabel)
        ensures
            result == spec_join(self, other),
            context_rank(result.secrecy) >= context_rank(self.secrecy),
            context_rank(result.secrecy) >= context_rank(other.secrecy),
            integrity_rank(result.integrity) <= integrity_rank(self.integrity),
            integrity_rank(result.integrity) <= integrity_rank(other.integrity),
    {
        let secrecy = if self.secrecy.rank() >= other.secrecy.rank() {
            self.secrecy
        } else {
            other.secrecy
        };

        let integrity = if self.integrity.rank() <= other.integrity.rank() {
            self.integrity
        } else {
            other.integrity
        };

        FlowLabel { secrecy, integrity }
    }
}

// ── Proof lemmas ──────────────────────────────────────────────────────

proof fn lemma_can_flow_transitive(a: ContextTier, b: ContextTier, c: ContextTier)
    ensures
        ((context_rank(a) <= context_rank(b)) && (context_rank(b) <= context_rank(c)))
            ==> (context_rank(a) <= context_rank(c))
{}

proof fn lemma_can_read_reflexive(t: ContextTier)
    ensures context_rank(t) <= context_rank(t)
{}

proof fn lemma_can_read_transitive(a: ContextTier, b: ContextTier, c: ContextTier)
    ensures
        (context_rank(b) <= context_rank(a) && context_rank(c) <= context_rank(b))
            ==> context_rank(c) <= context_rank(a)
{}

/// can_flow_to(a,b) <==> can_read(b,a)
proof fn lemma_flow_and_read_dual(a: ContextTier, b: ContextTier)
    ensures
        (context_rank(a) <= context_rank(b)) <==> (context_rank(b) >= context_rank(a))
{}

/// join(a, a) == a
proof fn lemma_join_idempotent(a: FlowLabel)
    ensures spec_join(a, a) == a
{}

/// join(a, b) == join(b, a)
proof fn lemma_join_commutative(a: FlowLabel, b: FlowLabel)
    ensures spec_join(a, b) == spec_join(b, a)
{}

/// join(join(a,b), c) == join(a, join(b,c))
proof fn lemma_join_associative(a: FlowLabel, b: FlowLabel, c: FlowLabel)
    ensures spec_join(spec_join(a, b), c) == spec_join(a, spec_join(b, c))
{}

} // verus!

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rank_ordering() {
        assert!(ContextTier::Public.rank() < ContextTier::Family.rank());
        assert!(ContextTier::Family.rank() < ContextTier::Private.rank());
        assert!(IntegrityTier::Untrusted.rank() < IntegrityTier::Reviewed.rank());
        assert!(IntegrityTier::Reviewed.rank() < IntegrityTier::Trusted.rank());
    }

    #[test]
    fn can_flow_to_respects_order() {
        assert!(ContextTier::Public.can_flow_to(ContextTier::Private));
        assert!(ContextTier::Public.can_flow_to(ContextTier::Public));
        assert!(!ContextTier::Private.can_flow_to(ContextTier::Public));
    }

    #[test]
    fn can_read_is_downward() {
        assert!(ContextTier::Private.can_read(ContextTier::Public));
        assert!(!ContextTier::Public.can_read(ContextTier::Private));
    }

    #[test]
    fn join_raises_secrecy_lowers_integrity() {
        let a = FlowLabel::new(ContextTier::Public, IntegrityTier::Trusted);
        let b = FlowLabel::new(ContextTier::Private, IntegrityTier::Untrusted);
        let joined = a.join(b);
        assert_eq!(joined.secrecy, ContextTier::Private);
        assert_eq!(joined.integrity, IntegrityTier::Untrusted);
    }

    #[test]
    fn join_is_commutative() {
        let a = FlowLabel::new(ContextTier::Family, IntegrityTier::Reviewed);
        let b = FlowLabel::new(ContextTier::Public, IntegrityTier::Trusted);
        assert_eq!(a.join(b), b.join(a));
    }

    #[test]
    fn join_is_idempotent() {
        let a = FlowLabel::new(ContextTier::Family, IntegrityTier::Reviewed);
        assert_eq!(a.join(a), a);
    }
}
