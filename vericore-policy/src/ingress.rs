//! Ingress integrity decisions over normalized inputs.

use crate::tiers::IntegrityTier;
use vstd::prelude::*;

verus! {

/// Classification of a write target, produced by the I/O layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteTargetClass {
    Mindlock,
    Private,
    SensitiveConfig,
    Other,
    Unresolved,
}

/// Spec-level check: is this integrity tier Trusted?
pub open spec fn is_trusted(i: IntegrityTier) -> bool {
    i is Trusted
}

/// Whether an ingress integrity level is allowed to execute commands.
/// Only Trusted ingress may exec.
pub fn ingress_allows_exec(integrity: IntegrityTier) -> (result: bool)
    ensures result == spec_ingress_allows_exec(integrity)
{
    match integrity {
        IntegrityTier::Trusted => true,
        _ => false,
    }
}

/// Whether an ingress integrity level is allowed to write to the given target.
pub fn ingress_allows_write(integrity: IntegrityTier, target: WriteTargetClass) -> (result: bool)
    ensures result == spec_ingress_allows_write(integrity, target)
{
    match target {
        WriteTargetClass::Mindlock => true,
        WriteTargetClass::Private | WriteTargetClass::SensitiveConfig => {
            match integrity {
                IntegrityTier::Trusted => true,
                _ => false,
            }
        }
        WriteTargetClass::Other => true,
        WriteTargetClass::Unresolved => false,
    }
}

// Proof lemmas

// Spec-level versions of the decision functions for use in proofs
pub open spec fn spec_ingress_allows_exec(integrity: IntegrityTier) -> bool {
    is_trusted(integrity)
}

pub open spec fn spec_ingress_allows_write(integrity: IntegrityTier, target: WriteTargetClass) -> bool {
    match target {
        WriteTargetClass::Mindlock => true,
        WriteTargetClass::Private | WriteTargetClass::SensitiveConfig => is_trusted(integrity),
        WriteTargetClass::Other => true,
        WriteTargetClass::Unresolved => false,
    }
}

proof fn lemma_untrusted_cannot_exec()
    ensures !spec_ingress_allows_exec(IntegrityTier::Untrusted)
{}

proof fn lemma_mindlock_always_allowed(i: IntegrityTier)
    ensures spec_ingress_allows_write(i, WriteTargetClass::Mindlock)
{}

proof fn lemma_unresolved_always_denied(i: IntegrityTier)
    ensures !spec_ingress_allows_write(i, WriteTargetClass::Unresolved)
{}

// ── New lemmas ──────────────────────────────────────────────────────

/// Reviewed integrity also cannot exec (not just Untrusted).
proof fn lemma_reviewed_cannot_exec()
    ensures !spec_ingress_allows_exec(IntegrityTier::Reviewed)
{}

/// Trusted integrity can do everything (exec + write to any non-Unresolved target).
proof fn lemma_trusted_can_do_everything(target: WriteTargetClass)
    requires target != WriteTargetClass::Unresolved
    ensures
        spec_ingress_allows_exec(IntegrityTier::Trusted),
        spec_ingress_allows_write(IntegrityTier::Trusted, target),
{}

/// SensitiveConfig and Private have identical write rules for all integrity tiers.
proof fn lemma_sensitive_config_same_as_private(i: IntegrityTier)
    ensures spec_ingress_allows_write(i, WriteTargetClass::SensitiveConfig)
        == spec_ingress_allows_write(i, WriteTargetClass::Private)
{}

/// Write access is monotone in integrity: if Untrusted is denied, Reviewed is also denied.
/// (In fact they are identical for writes — both require Trusted for Private/SensitiveConfig.)
proof fn lemma_ingress_write_monotone(target: WriteTargetClass)
    ensures !spec_ingress_allows_write(IntegrityTier::Untrusted, target)
        ==> !spec_ingress_allows_write(IntegrityTier::Reviewed, target)
{}

} // verus!

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_trusted_can_exec() {
        assert!(!ingress_allows_exec(IntegrityTier::Untrusted));
        assert!(!ingress_allows_exec(IntegrityTier::Reviewed));
        assert!(ingress_allows_exec(IntegrityTier::Trusted));
    }

    #[test]
    fn mindlock_always_allowed() {
        for integrity in [
            IntegrityTier::Untrusted,
            IntegrityTier::Reviewed,
            IntegrityTier::Trusted,
        ] {
            assert!(ingress_allows_write(integrity, WriteTargetClass::Mindlock));
        }
    }

    #[test]
    fn private_only_trusted() {
        assert!(!ingress_allows_write(
            IntegrityTier::Untrusted,
            WriteTargetClass::Private
        ));
        assert!(!ingress_allows_write(
            IntegrityTier::Reviewed,
            WriteTargetClass::Private
        ));
        assert!(ingress_allows_write(
            IntegrityTier::Trusted,
            WriteTargetClass::Private
        ));
    }

    #[test]
    fn sensitive_config_only_trusted() {
        assert!(!ingress_allows_write(
            IntegrityTier::Untrusted,
            WriteTargetClass::SensitiveConfig
        ));
        assert!(!ingress_allows_write(
            IntegrityTier::Reviewed,
            WriteTargetClass::SensitiveConfig
        ));
        assert!(ingress_allows_write(
            IntegrityTier::Trusted,
            WriteTargetClass::SensitiveConfig
        ));
    }

    #[test]
    fn other_always_allowed() {
        for integrity in [
            IntegrityTier::Untrusted,
            IntegrityTier::Reviewed,
            IntegrityTier::Trusted,
        ] {
            assert!(ingress_allows_write(integrity, WriteTargetClass::Other));
        }
    }

    #[test]
    fn unresolved_always_denied() {
        for integrity in [
            IntegrityTier::Untrusted,
            IntegrityTier::Reviewed,
            IntegrityTier::Trusted,
        ] {
            assert!(!ingress_allows_write(
                integrity,
                WriteTargetClass::Unresolved
            ));
        }
    }
}
