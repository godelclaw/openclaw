/// lesson06_sink_classes.rs
///
/// Introduces SinkClass — the distinction between ordinary data sinks
/// and policy/config sinks. This is the formal gap identified in the
/// current vericore-core: vericore.toml lives under home_root but is
/// not in private/ or family/, so it is currently classified Public
/// and gets no special write protection.
///
/// SinkClass adds a second axis orthogonal to ContextTier:
///
///   Data sinks:   Public | Family | Private  (information-flow tier)
///   Config sinks: ConfigOperational | ConfigSensitive  (self-modification class)
///
/// The key property: writing to any config sink requires at least
/// Private context (only the DM / terminal can propose policy changes),
/// and ConfigSensitive additionally requires parent approval — even
/// from Private context.
///
/// This is the formal statement of "the filter filters changes to itself."
use vstd::prelude::*;

verus! {

#[derive(PartialEq, Eq, Clone, Copy)]
pub enum ContextTier {
    Public,
    Family,
    Private,
}

pub open spec fn rank(c: ContextTier) -> nat {
    match c {
        ContextTier::Public  => 0,
        ContextTier::Family  => 1,
        ContextTier::Private => 2,
    }
}

/// Sink classification — what kind of thing is being written to.
#[derive(PartialEq, Eq, Clone, Copy)]
pub enum SinkClass {
    /// Ordinary data: a file, a chat message, a web request.
    /// Governed purely by ContextTier flow rules.
    DataSink,
    /// A config/policy file that affects future gate behavior,
    /// but does NOT require parent approval (e.g. adding a network
    /// host to the allowlist, changing a schedule window).
    ConfigOperational,
    /// A config/policy file that changes the security posture itself:
    /// lowering a tier requirement, adding a new channel, changing
    /// require_parent_approval. Always requires explicit parent approval.
    ConfigSensitive,
}

/// Whether a (context, sink_class) pair is allowed to proceed
/// without any further approval step.
///
/// Rules:
///   DataSink         — governed by ContextTier (handled in lesson05)
///   ConfigOperational — requires Private context; no extra approval
///   ConfigSensitive   — requires Private context AND parent approval
///                       (approval is modeled as a separate predicate here)
pub open spec fn sink_class_allows_without_approval(
    context: ContextTier,
    class: SinkClass,
) -> bool {
    match class {
        SinkClass::DataSink => true, // tier check handled elsewhere
        SinkClass::ConfigOperational => context == ContextTier::Private,
        SinkClass::ConfigSensitive => false, // always needs approval, even Private
    }
}

pub open spec fn requires_parent_approval(class: SinkClass) -> bool {
    class == SinkClass::ConfigSensitive
}

// ── Core properties ───────────────────────────────────────────────────────

/// No public or family context can write to any config sink without approval.
proof fn lemma_config_sinks_require_private_context(class: SinkClass)
    requires
        class == SinkClass::ConfigOperational || class == SinkClass::ConfigSensitive,
    ensures
        !sink_class_allows_without_approval(ContextTier::Public, class),
        !sink_class_allows_without_approval(ContextTier::Family, class),
{}

/// ConfigSensitive always requires parent approval — even from Private context.
/// This is the "constitutional amendment" property: you can't bootstrap a more
/// permissive policy without going through the current approval gate.
proof fn lemma_sensitive_always_requires_approval()
    ensures
        !sink_class_allows_without_approval(ContextTier::Private, SinkClass::ConfigSensitive),
        requires_parent_approval(SinkClass::ConfigSensitive),
        !requires_parent_approval(SinkClass::ConfigOperational),
        !requires_parent_approval(SinkClass::DataSink),
{}

/// ConfigOperational is accessible from Private without extra approval —
/// the agent can propose and apply operational changes autonomously.
proof fn lemma_operational_accessible_from_private()
    ensures
        sink_class_allows_without_approval(ContextTier::Private, SinkClass::ConfigOperational),
{}

// ── The no-bootstrap-bypass property ─────────────────────────────────────

/// This is the formal statement of GödelClaw self-modification safety:
/// for every possible context tier and every config sink class,
/// a ConfigSensitive write always requires parent approval.
/// There is no context tier that bypasses this.
proof fn lemma_no_bootstrap_bypass()
    ensures
        forall |class: SinkClass| #[trigger] requires_parent_approval(class) || true ==> (
            class == SinkClass::ConfigSensitive ==> requires_parent_approval(class)
        ),
{}

/// Corollary: the agent cannot write a ConfigSensitive policy change
/// and have it take effect in the same turn without approval,
/// regardless of which channel the stimulus came from.
proof fn lemma_policy_change_always_gated(ctx: ContextTier)
    ensures
        requires_parent_approval(SinkClass::ConfigSensitive),
        !sink_class_allows_without_approval(ctx, SinkClass::ConfigSensitive),
{}

// ── Sink class is orthogonal to tier flow ─────────────────────────────────

/// A DataSink write at Private context is always allowed by the class check
/// (tier flow is handled separately). Config sinks add an independent gate.
proof fn lemma_sink_class_is_orthogonal_to_tier()
    ensures
        // DataSink: class check passes; tier check is separate
        sink_class_allows_without_approval(ContextTier::Private, SinkClass::DataSink),
        sink_class_allows_without_approval(ContextTier::Family,  SinkClass::DataSink),
        sink_class_allows_without_approval(ContextTier::Public,  SinkClass::DataSink),
        // ConfigOperational: class check adds Private requirement
        sink_class_allows_without_approval(ContextTier::Private, SinkClass::ConfigOperational),
        !sink_class_allows_without_approval(ContextTier::Family, SinkClass::ConfigOperational),
        !sink_class_allows_without_approval(ContextTier::Public, SinkClass::ConfigOperational),
        // ConfigSensitive: class check blocks everything without approval
        !sink_class_allows_without_approval(ContextTier::Private, SinkClass::ConfigSensitive),
        !sink_class_allows_without_approval(ContextTier::Family,  SinkClass::ConfigSensitive),
        !sink_class_allows_without_approval(ContextTier::Public,  SinkClass::ConfigSensitive),
{}

// ── Faichney connection: the gate decodes meaning ─────────────────────────

/// The same bytes written to a DataSink vs a ConfigSensitive sink have
/// different *meaning* — one is content, one is a policy change.
/// The SinkClass is what encodes that distinction formally.
///
/// We model this as: the "effect class" of a write is determined by
/// the sink class, not just the content bytes.
#[derive(PartialEq, Eq, Clone, Copy)]
pub enum EffectClass {
    ContentEffect,  // changes what the agent says/stores
    PolicyEffect,   // changes how the agent will behave in future turns
}

pub open spec fn effect_of(class: SinkClass) -> EffectClass {
    match class {
        SinkClass::DataSink => EffectClass::ContentEffect,
        SinkClass::ConfigOperational | SinkClass::ConfigSensitive => EffectClass::PolicyEffect,
    }
}

/// Policy effects always require at minimum Private context.
proof fn lemma_policy_effects_require_private(class: SinkClass)
    requires effect_of(class) == EffectClass::PolicyEffect
    ensures
        !sink_class_allows_without_approval(ContextTier::Public, class),
        !sink_class_allows_without_approval(ContextTier::Family, class),
{}

fn main() {}

} // verus!
