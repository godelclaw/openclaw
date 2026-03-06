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

impl FlowLabel {
    pub fn new(secrecy: ContextTier, integrity: IntegrityTier) -> (result: FlowLabel)
        ensures result.secrecy == secrecy && result.integrity == integrity
    {
        FlowLabel { secrecy, integrity }
    }

    /// Conservative label join: secrecy rises (max), integrity drops (min).
    pub fn join(self, other: FlowLabel) -> (result: FlowLabel)
        ensures
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

// Proof lemmas

proof fn lemma_can_flow_transitive(a: ContextTier, b: ContextTier, c: ContextTier)
    ensures
        ((context_rank(a) <= context_rank(b)) && (context_rank(b) <= context_rank(c)))
            ==> (context_rank(a) <= context_rank(c))
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
