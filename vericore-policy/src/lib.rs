//! VeriCore Policy Crate
//!
//! This crate contains both runtime-called verified policy functions and
//! reference specs for adjacent policy surfaces. All modules are pure
//! deterministic code with no I/O — filesystem, network, and async logic
//! stays in vericore-core.
//!
//! Verified with Verus: all public functions carry `ensures` postconditions
//! checked by SMT solver. Ghost code compiles away in normal builds via
//! `cfg(verus_keep_ghost)`.
//!
//! ## Runtime-called verified kernel
//!
//! The following modules contain functions that production (vericore-core)
//! calls directly for live policy decisions:
//!
//! - `tiers`: context/integrity rank, flow-label algebra
//! - `channels`: default channel-to-context and context-to-integrity mapping
//! - `ingress`: ingress allow/deny decisions over normalized target classes
//!
//! ## Verified reference specs
//!
//! The following modules are proved reference specs. They are not yet called
//! by runtime but define invariants that TypeScript tests should mirror:
//!
//! - `model_resolution`: candidate list validity, cross-provider gating,
//!   and resolution metadata for the OpenClaw model fallback resolver

pub mod tiers;
pub mod channels;
pub mod ingress;
pub mod model_resolution;
