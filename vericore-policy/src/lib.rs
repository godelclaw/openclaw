//! VeriCore Policy Decision Kernel
//!
//! Pure deterministic decision functions for VeriCore's security policy.
//! This crate contains no I/O — all filesystem, network, and async logic
//! stays in vericore-core. The functions here operate on normalized inputs
//! and return deterministic verdicts.
//!
//! Design goal: this crate will be verified with Verus. All public functions
//! will carry `ensures` postconditions checked by SMT solver. Ghost code
//! compiles away in normal builds via `cfg(verus_keep_ghost)`.

pub mod tiers;
pub mod channels;
pub mod ingress;
pub mod model_resolution;
