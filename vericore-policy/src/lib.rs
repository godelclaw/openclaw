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
//! - `tiers`: context/integrity rank, flow-label algebra
//! - `channels`: default channel-to-context and context-to-integrity mapping
//! - `ingress`: ingress allow/deny decisions over normalized target classes
//!
//! ## Verified reference specs
//!
//! - `model_resolution`: candidate list validity, cross-provider gating
//! - `llm_bridge`: bridge request/response contract
//! - `mindlock`: artifact state machine
//! - `receipt`: promotion authorization
//! - `egress`: outbound flow control
//! - `gate_chain`: full gate composition
//!
//! ## Cross-module composition proofs
//!
//! - `compositions`: end-to-end properties chaining multiple modules

pub mod tiers;
pub mod channels;
pub mod ingress;
pub mod model_resolution;
pub mod llm_bridge;
pub mod mindlock;
pub mod receipt;
pub mod egress;
pub mod gate_chain;
pub mod compositions;
