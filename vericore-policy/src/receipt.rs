//! Promotion authorization via receipts.
//!
//! Every promoted artifact must have authorization: either a hash-verified
//! receipt from the security reviewer, or explicit Zar (human) approval
//! with an audit entry.

use vstd::prelude::*;

verus! {

// ── Spec functions ───────────────────────────────────────────────────

/// Spec: a receipt is valid if the hash matches the artifact bytes
/// AND the target path matches the intended destination.
pub open spec fn spec_receipt_valid(
    hash_matches: bool,
    target_matches: bool,
) -> bool {
    hash_matches && target_matches
}

/// Spec: a Zar (human) approval is valid if there is an audit entry.
pub open spec fn spec_zar_approval_valid(has_audit_entry: bool) -> bool {
    has_audit_entry
}

/// Spec: promotion is authorized if either a valid receipt OR valid Zar approval exists.
pub open spec fn spec_promotion_authorized(
    has_valid_receipt: bool,
    has_valid_zar_approval: bool,
) -> bool {
    has_valid_receipt || has_valid_zar_approval
}

// ── Exec functions ───────────────────────────────────────────────────

pub fn receipt_valid(hash_matches: bool, target_matches: bool) -> (result: bool)
    ensures result == spec_receipt_valid(hash_matches, target_matches)
{
    hash_matches && target_matches
}

pub fn zar_approval_valid(has_audit_entry: bool) -> (result: bool)
    ensures result == spec_zar_approval_valid(has_audit_entry)
{
    has_audit_entry
}

pub fn promotion_authorized(
    has_valid_receipt: bool,
    has_valid_zar_approval: bool,
) -> (result: bool)
    ensures result == spec_promotion_authorized(has_valid_receipt, has_valid_zar_approval)
{
    has_valid_receipt || has_valid_zar_approval
}

// ── Proof lemmas ─────────────────────────────────────────────────────

/// Different bytes → invalid receipt (tamper detection).
proof fn lemma_receipt_detects_tamper(target_matches: bool)
    ensures !spec_receipt_valid(false, target_matches)
{}

/// Different target → invalid receipt (swap detection).
proof fn lemma_receipt_detects_target_swap(hash_matches: bool)
    ensures !spec_receipt_valid(hash_matches, false)
{}

/// No audit → invalid Zar approval.
proof fn lemma_zar_approval_requires_audit()
    ensures !spec_zar_approval_valid(false)
{}

/// Promotion requires at least one form of authorization.
proof fn lemma_promotion_requires_at_least_one_auth()
    ensures !spec_promotion_authorized(false, false)
{}

/// Receipt alone is sufficient for promotion.
proof fn lemma_receipt_sufficient_for_promotion(has_zar: bool)
    ensures spec_promotion_authorized(true, has_zar)
{}

/// Zar approval alone is sufficient for promotion.
proof fn lemma_zar_approval_sufficient_for_promotion(has_receipt: bool)
    ensures spec_promotion_authorized(has_receipt, true)
{}

} // verus!

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_receipt_requires_both() {
        assert!(receipt_valid(true, true));
        assert!(!receipt_valid(false, true));
        assert!(!receipt_valid(true, false));
        assert!(!receipt_valid(false, false));
    }

    #[test]
    fn zar_approval_requires_audit() {
        assert!(zar_approval_valid(true));
        assert!(!zar_approval_valid(false));
    }

    #[test]
    fn promotion_needs_at_least_one() {
        assert!(promotion_authorized(true, false));
        assert!(promotion_authorized(false, true));
        assert!(promotion_authorized(true, true));
        assert!(!promotion_authorized(false, false));
    }
}
