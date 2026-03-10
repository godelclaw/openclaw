//! Startup workspace identity boundary contract.
//!
//! These pure decision functions validate the TS-reported startup context facts
//! that Rust can check at the boundary: AGENTS.md and SOUL.md must both be
//! available in workspace bootstrap state and must both actually be injected
//! into the startup prompt context.

use vstd::prelude::*;

verus! {

/// Spec: both required workspace identity sources are present.
pub open spec fn spec_startup_identity_sources_available(
    agents_available: bool,
    soul_available: bool,
) -> bool {
    agents_available && soul_available
}

pub fn startup_identity_sources_available(
    agents_available: bool,
    soul_available: bool,
) -> (result: bool)
    ensures result == spec_startup_identity_sources_available(agents_available, soul_available)
{
    agents_available && soul_available
}

/// Spec: both required identity files were injected into startup context.
pub open spec fn spec_startup_identity_injected(
    agents_injected: bool,
    soul_injected: bool,
) -> bool {
    agents_injected && soul_injected
}

pub fn startup_identity_injected(
    agents_injected: bool,
    soul_injected: bool,
) -> (result: bool)
    ensures result == spec_startup_identity_injected(agents_injected, soul_injected)
{
    agents_injected && soul_injected
}

/// Spec: the full startup identity contract holds only when both files are
/// present and both files were injected into the startup prompt.
pub open spec fn spec_startup_identity_contract_ok(
    sources_available: bool,
    identity_injected: bool,
) -> bool {
    sources_available && identity_injected
}

pub fn startup_identity_contract_ok(
    sources_available: bool,
    identity_injected: bool,
) -> (result: bool)
    ensures result == spec_startup_identity_contract_ok(sources_available, identity_injected)
{
    sources_available && identity_injected
}

proof fn lemma_missing_agents_breaks_source_contract(soul_available: bool)
    ensures !spec_startup_identity_sources_available(false, soul_available)
{}

proof fn lemma_missing_soul_breaks_source_contract(agents_available: bool)
    ensures !spec_startup_identity_sources_available(agents_available, false)
{}

proof fn lemma_missing_agents_injection_breaks_contract(soul_injected: bool)
    ensures !spec_startup_identity_injected(false, soul_injected)
{}

proof fn lemma_missing_soul_injection_breaks_contract(agents_injected: bool)
    ensures !spec_startup_identity_injected(agents_injected, false)
{}

proof fn lemma_incomplete_startup_identity_breaks_contract(identity_injected: bool)
    ensures !spec_startup_identity_contract_ok(false, identity_injected)
{}

proof fn lemma_missing_injection_breaks_contract(sources_available: bool)
    ensures !spec_startup_identity_contract_ok(sources_available, false)
{}

proof fn lemma_all_requirements_imply_contract_ok()
    ensures spec_startup_identity_contract_ok(true, true)
{}

} // verus!

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sources_require_agents_and_soul() {
        assert!(startup_identity_sources_available(true, true));
        assert!(!startup_identity_sources_available(false, true));
        assert!(!startup_identity_sources_available(true, false));
        assert!(!startup_identity_sources_available(false, false));
    }

    #[test]
    fn injection_requires_agents_and_soul() {
        assert!(startup_identity_injected(true, true));
        assert!(!startup_identity_injected(false, true));
        assert!(!startup_identity_injected(true, false));
        assert!(!startup_identity_injected(false, false));
    }

    #[test]
    fn startup_identity_contract_requires_both_predicates() {
        assert!(startup_identity_contract_ok(true, true));
        assert!(!startup_identity_contract_ok(false, true));
        assert!(!startup_identity_contract_ok(true, false));
        assert!(!startup_identity_contract_ok(false, false));
    }
}
