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
//! ## Tool broker and tool policy specs
//!
//! - `tool_broker`: broker request/response contract and parsed capability-ID shape checks
//! - `tool_policy`: tool authorization, native-shadow guard, and combined tool-gate invariants
//!
//! ## Session model policy specs
//!
//! - `session_model_policy`: bridge model resolution contract, fallback visibility, allowlist
//! - `startup_context_policy`: AGENTS/SOUL startup identity boundary contract
//! - `heartbeat_sync_policy`: HEARTBEAT.md canonical/mirror coherence and prompt contract
//! - `heartbeat_context_policy`: recent-turn window and heartbeat-trace continuity contract
//! - `energy_policy`: deterministic energy fold and 30-minute conversation-gap contract
//! - `affect_policy`: affect trace EWMA fold and gamma-lock boundary contract
//!
//! ## Cross-module composition proofs
//!
//! - `compositions`: end-to-end properties chaining multiple modules

pub mod affect_policy;
pub mod channels;
pub mod compositions;
pub mod egress;
pub mod gate_chain;
pub mod heartbeat_context_policy;
pub mod heartbeat_sync_policy;
pub mod ingress;
pub mod energy_policy;
pub mod llm_bridge;
pub mod mindlock;
pub mod model_resolution;
pub mod receipt;
pub mod session_model_policy;
pub mod startup_context_policy;
pub mod tiers;
pub mod tool_broker;
pub mod tool_policy;
