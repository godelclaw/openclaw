/// lesson07_dual_lattice_ingress.rs
///
/// Extends the previous lessons with a dual label algebra:
///   Label = (secrecy, integrity)
///
/// Secrecy is contagious upward (max join), integrity is fragile downward (min join).
/// Also models Phase-1 ingress constraints and top-secret tier demotion rules.
use vstd::prelude::*;

verus! {

#[derive(PartialEq, Eq, Clone, Copy)]
pub enum SecrecyTier {
    Public,
    Family,
    Private,
    TopSecret,
}

pub open spec fn sec_rank(t: SecrecyTier) -> nat {
    match t {
        SecrecyTier::Public => 0,
        SecrecyTier::Family => 1,
        SecrecyTier::Private => 2,
        SecrecyTier::TopSecret => 3,
    }
}

pub open spec fn can_flow_to(label: SecrecyTier, sink: SecrecyTier) -> bool {
    sec_rank(label) <= sec_rank(sink)
}

#[derive(PartialEq, Eq, Clone, Copy)]
pub enum IntegrityTier {
    Untrusted,
    Reviewed,
    Trusted,
}

pub open spec fn int_rank(t: IntegrityTier) -> nat {
    match t {
        IntegrityTier::Untrusted => 0,
        IntegrityTier::Reviewed => 1,
        IntegrityTier::Trusted => 2,
    }
}

#[derive(PartialEq, Eq, Clone, Copy)]
pub struct Label {
    pub sec: SecrecyTier,
    pub int: IntegrityTier,
}

pub open spec fn sec_join(a: SecrecyTier, b: SecrecyTier) -> SecrecyTier {
    if sec_rank(a) >= sec_rank(b) { a } else { b }
}

pub open spec fn int_join(a: IntegrityTier, b: IntegrityTier) -> IntegrityTier {
    if int_rank(a) <= int_rank(b) { a } else { b }
}

pub open spec fn label_join(a: Label, b: Label) -> Label {
    Label {
        sec: sec_join(a.sec, b.sec),
        int: int_join(a.int, b.int),
    }
}

proof fn lemma_secrecy_join_monotone(a: Label, b: Label)
    ensures
        sec_rank(label_join(a, b).sec) >= sec_rank(a.sec),
        sec_rank(label_join(a, b).sec) >= sec_rank(b.sec),
{}

proof fn lemma_integrity_join_monotone(a: Label, b: Label)
    ensures
        int_rank(label_join(a, b).int) <= int_rank(a.int),
        int_rank(label_join(a, b).int) <= int_rank(b.int),
{}

proof fn lemma_private_and_top_secret_block_public_sink()
    ensures
        !can_flow_to(SecrecyTier::Private, SecrecyTier::Public),
        !can_flow_to(SecrecyTier::TopSecret, SecrecyTier::Public),
        !can_flow_to(SecrecyTier::TopSecret, SecrecyTier::Family),
        !can_flow_to(SecrecyTier::TopSecret, SecrecyTier::Private),
{}

#[derive(PartialEq, Eq, Clone, Copy)]
pub enum MutationKind {
    Exec,
    WritePublic,
    WritePrivate,
    WriteConfig,
}

/// Phase-1 ingress policy:
/// - exec/private writes/config writes require Trusted ingress
/// - public writes are allowed for any integrity
pub open spec fn ingress_allows(integrity: IntegrityTier, kind: MutationKind) -> bool {
    match kind {
        MutationKind::Exec => integrity == IntegrityTier::Trusted,
        MutationKind::WritePrivate => integrity == IntegrityTier::Trusted,
        MutationKind::WriteConfig => integrity == IntegrityTier::Trusted,
        MutationKind::WritePublic => true,
    }
}

proof fn lemma_untrusted_ingress_cannot_trigger_sensitive_mutations()
    ensures
        !ingress_allows(IntegrityTier::Untrusted, MutationKind::Exec),
        !ingress_allows(IntegrityTier::Untrusted, MutationKind::WritePrivate),
        !ingress_allows(IntegrityTier::Untrusted, MutationKind::WriteConfig),
        ingress_allows(IntegrityTier::Untrusted, MutationKind::WritePublic),
{}

proof fn lemma_reviewed_ingress_cannot_trigger_sensitive_mutations()
    ensures
        !ingress_allows(IntegrityTier::Reviewed, MutationKind::Exec),
        !ingress_allows(IntegrityTier::Reviewed, MutationKind::WritePrivate),
        !ingress_allows(IntegrityTier::Reviewed, MutationKind::WriteConfig),
        ingress_allows(IntegrityTier::Reviewed, MutationKind::WritePublic),
{}

proof fn lemma_trusted_ingress_can_trigger_sensitive_mutations()
    ensures
        ingress_allows(IntegrityTier::Trusted, MutationKind::Exec),
        ingress_allows(IntegrityTier::Trusted, MutationKind::WritePrivate),
        ingress_allows(IntegrityTier::Trusted, MutationKind::WriteConfig),
{}

/// Reclassification rule:
/// - upward moves are always allowed
/// - any downward move requires operator approval
/// - especially TopSecret -> lower always requires operator approval
pub open spec fn can_reclassify(
    from: SecrecyTier,
    to: SecrecyTier,
    operator_approved: bool,
) -> bool {
    if sec_rank(to) >= sec_rank(from) {
        true
    } else {
        operator_approved
    }
}

proof fn lemma_top_secret_demotion_requires_operator()
    ensures
        !can_reclassify(SecrecyTier::TopSecret, SecrecyTier::Private, false),
        !can_reclassify(SecrecyTier::TopSecret, SecrecyTier::Family, false),
        !can_reclassify(SecrecyTier::TopSecret, SecrecyTier::Public, false),
        can_reclassify(SecrecyTier::TopSecret, SecrecyTier::Private, true),
{}

proof fn lemma_sleep_process_can_only_safe_promote_without_operator()
    ensures
        can_reclassify(SecrecyTier::Private, SecrecyTier::TopSecret, false),
        can_reclassify(SecrecyTier::Family, SecrecyTier::Private, false),
        !can_reclassify(SecrecyTier::Private, SecrecyTier::Family, false),
{}

fn main() {}

} // verus!
