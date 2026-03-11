//! Heartbeat coherence boundary contract.
//!
//! These pure decision functions validate TS-reported facts about heartbeat
//! coherence at the Rust boundary:
//! - the canonical HEARTBEAT.md exists,
//! - the mirrored state HEARTBEAT.md exists and matches the canonical file,
//! - the configured heartbeat prompt follows HEARTBEAT.md rather than a stale
//!   duplicated semantic prompt such as the legacy GAS.md variant.

use vstd::prelude::*;

verus! {

/// Spec: both canonical and mirrored heartbeat files are present.
pub open spec fn spec_heartbeat_sources_available(
    canonical_available: bool,
    mirror_available: bool,
) -> bool {
    canonical_available && mirror_available
}

pub fn heartbeat_sources_available(
    canonical_available: bool,
    mirror_available: bool,
) -> (result: bool)
    ensures result == spec_heartbeat_sources_available(canonical_available, mirror_available)
{
    canonical_available && mirror_available
}

/// Spec: the mirrored heartbeat file matches the canonical file.
pub open spec fn spec_heartbeat_mirror_consistent(mirror_matches: bool) -> bool {
    mirror_matches
}

pub fn heartbeat_mirror_consistent(mirror_matches: bool) -> (result: bool)
    ensures result == spec_heartbeat_mirror_consistent(mirror_matches)
{
    mirror_matches
}

/// Spec: the prompt is valid only when it explicitly follows HEARTBEAT.md and
/// does not reference the legacy GAS.md heartbeat wording.
pub open spec fn spec_heartbeat_prompt_ok(
    prompt_uses_file_reference: bool,
    prompt_mentions_legacy_gas: bool,
) -> bool {
    prompt_uses_file_reference && !prompt_mentions_legacy_gas
}

pub fn heartbeat_prompt_ok(
    prompt_uses_file_reference: bool,
    prompt_mentions_legacy_gas: bool,
) -> (result: bool)
    ensures result == spec_heartbeat_prompt_ok(
        prompt_uses_file_reference, prompt_mentions_legacy_gas,
    )
{
    prompt_uses_file_reference && !prompt_mentions_legacy_gas
}

/// Spec: the full heartbeat coherence contract requires both files, matching
/// mirror contents, and a prompt that defers to HEARTBEAT.md.
pub open spec fn spec_heartbeat_contract_ok(
    sources_available: bool,
    mirror_consistent: bool,
    prompt_ok: bool,
) -> bool {
    sources_available && mirror_consistent && prompt_ok
}

pub fn heartbeat_contract_ok(
    sources_available: bool,
    mirror_consistent: bool,
    prompt_ok: bool,
) -> (result: bool)
    ensures result == spec_heartbeat_contract_ok(sources_available, mirror_consistent, prompt_ok)
{
    sources_available && mirror_consistent && prompt_ok
}

proof fn lemma_missing_canonical_breaks_sources(mirror_available: bool)
    ensures !spec_heartbeat_sources_available(false, mirror_available)
{}

proof fn lemma_missing_mirror_breaks_sources(canonical_available: bool)
    ensures !spec_heartbeat_sources_available(canonical_available, false)
{}

proof fn lemma_mirror_mismatch_breaks_contract(
    sources_available: bool,
    prompt_ok: bool,
)
    ensures !spec_heartbeat_contract_ok(sources_available, false, prompt_ok)
{}

proof fn lemma_missing_file_reference_breaks_prompt(legacy_gas: bool)
    ensures !spec_heartbeat_prompt_ok(false, legacy_gas)
{}

proof fn lemma_legacy_gas_breaks_prompt()
    ensures !spec_heartbeat_prompt_ok(true, true)
{}

proof fn lemma_prompt_failure_breaks_contract(
    sources_available: bool,
    mirror_consistent: bool,
)
    ensures !spec_heartbeat_contract_ok(sources_available, mirror_consistent, false)
{}

proof fn lemma_all_heartbeat_requirements_imply_contract_ok()
    ensures spec_heartbeat_contract_ok(true, true, true)
{}

} // verus!

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sources_require_canonical_and_mirror() {
        assert!(heartbeat_sources_available(true, true));
        assert!(!heartbeat_sources_available(false, true));
        assert!(!heartbeat_sources_available(true, false));
        assert!(!heartbeat_sources_available(false, false));
    }

    #[test]
    fn mirror_consistency_requires_match() {
        assert!(heartbeat_mirror_consistent(true));
        assert!(!heartbeat_mirror_consistent(false));
    }

    #[test]
    fn prompt_requires_file_reference_without_legacy_gas() {
        assert!(heartbeat_prompt_ok(true, false));
        assert!(!heartbeat_prompt_ok(false, false));
        assert!(!heartbeat_prompt_ok(true, true));
        assert!(!heartbeat_prompt_ok(false, true));
    }

    #[test]
    fn full_contract_requires_sources_match_and_prompt() {
        assert!(heartbeat_contract_ok(true, true, true));
        assert!(!heartbeat_contract_ok(false, true, true));
        assert!(!heartbeat_contract_ok(true, false, true));
        assert!(!heartbeat_contract_ok(true, true, false));
    }
}
