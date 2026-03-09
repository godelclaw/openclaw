use std::collections::HashMap;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::config::{Config, StringList, TimeWindow};
use crate::types::{Action, Channel, ContextTier, IntegrityTier, Verdict};
use vericore_policy::ingress::WriteTargetClass;

/// Filesystem layout:
///
///   /home/zarclaw/              <- home_root (default classification root)
///   /home/zarclaw/private/      <- explicit Private carveout
///   /home/zarclaw/family/       <- explicit Family carveout
///
/// Additional policy:
/// - dot-prefixed paths are private by default (configurable)
/// - additive private prefixes can be explicitly listed
/// - additive family prefixes can be explicitly listed
/// - optional public carveouts can be explicitly listed when default_private is enabled
///
/// Tier resolution order:
///   1) outside home_root => denied
///   2) explicit private carveout => Private
///   3) additive private prefixes => Private
///   4) dot paths (if enabled) => Private
///   5) explicit family carveout => Family
///   6) additive family prefixes => Family
///   7) if default_private: public prefixes => Public, otherwise Private
///   8) everything else under home_root => Public
#[derive(Debug, Clone)]
pub struct ContextRoots {
    pub home_root: PathBuf,
    pub private: PathBuf,
    pub family: PathBuf,
    private_prefixes: Vec<PathBuf>,
    family_prefixes: Vec<PathBuf>,
    public_prefixes: Vec<PathBuf>,
    dot_paths_private: bool,
    default_private: bool,
}

impl ContextRoots {
    pub fn new(
        home_root: impl AsRef<Path>,
        private: impl AsRef<Path>,
        family: impl AsRef<Path>,
    ) -> Result<Self, String> {
        let home_root = canonicalize_dir(home_root.as_ref())?;
        let private = canonicalize_dir(private.as_ref())?;
        let family = canonicalize_dir(family.as_ref())?;

        if !private.starts_with(&home_root) {
            return Err(format!(
                "private root '{}' must be under home '{}'",
                private.display(),
                home_root.display()
            ));
        }
        if !family.starts_with(&home_root) {
            return Err(format!(
                "family root '{}' must be under home '{}'",
                family.display(),
                home_root.display()
            ));
        }
        if private.starts_with(&family) || family.starts_with(&private) {
            return Err(format!(
                "private and family roots must not overlap: '{}', '{}'",
                private.display(),
                family.display()
            ));
        }

        Ok(Self {
            home_root,
            private,
            family,
            private_prefixes: Vec::new(),
            family_prefixes: Vec::new(),
            public_prefixes: Vec::new(),
            dot_paths_private: true,
            default_private: false,
        })
    }

    pub fn with_private_prefixes(mut self, prefixes: Vec<PathBuf>) -> Result<Self, String> {
        let mut canonical = Vec::new();
        for prefix in prefixes {
            let c = canonicalize_dir(&prefix)?;
            if !c.starts_with(&self.home_root) {
                return Err(format!(
                    "private prefix '{}' must be under home '{}'",
                    c.display(),
                    self.home_root.display()
                ));
            }
            if c == self.home_root {
                return Err("home root cannot be marked as private prefix".into());
            }
            if c.starts_with(&self.family) || self.family.starts_with(&c) {
                return Err(format!(
                    "private prefix '{}' overlaps with family root '{}'",
                    c.display(),
                    self.family.display()
                ));
            }
            canonical.push(c);
        }
        self.private_prefixes = canonical;
        Ok(self)
    }

    pub fn with_family_prefixes(mut self, prefixes: Vec<PathBuf>) -> Result<Self, String> {
        let mut canonical = Vec::new();
        for prefix in prefixes {
            let c = canonicalize_dir(&prefix)?;
            if !c.starts_with(&self.home_root) {
                return Err(format!(
                    "family prefix '{}' must be under home '{}'",
                    c.display(),
                    self.home_root.display()
                ));
            }
            if c == self.home_root {
                return Err("home root cannot be marked as family prefix".into());
            }
            if c.starts_with(&self.private) || self.private.starts_with(&c) {
                return Err(format!(
                    "family prefix '{}' overlaps with private root '{}'",
                    c.display(),
                    self.private.display()
                ));
            }
            if self
                .private_prefixes
                .iter()
                .any(|p| c.starts_with(p) || p.starts_with(&c))
            {
                return Err(format!(
                    "family prefix '{}' overlaps with private prefix",
                    c.display()
                ));
            }
            canonical.push(c);
        }
        self.family_prefixes = canonical;
        Ok(self)
    }

    pub fn with_public_prefixes(mut self, prefixes: Vec<PathBuf>) -> Result<Self, String> {
        let mut canonical = Vec::new();
        for prefix in prefixes {
            let c = canonicalize_dir(&prefix)?;
            if !c.starts_with(&self.home_root) {
                return Err(format!(
                    "public prefix '{}' must be under home '{}'",
                    c.display(),
                    self.home_root.display()
                ));
            }
            if c == self.home_root {
                return Err("home root cannot be marked as public prefix".into());
            }
            if c.starts_with(&self.private) || self.private.starts_with(&c) {
                return Err(format!(
                    "public prefix '{}' overlaps with private root '{}'",
                    c.display(),
                    self.private.display()
                ));
            }
            if c.starts_with(&self.family) || self.family.starts_with(&c) {
                return Err(format!(
                    "public prefix '{}' overlaps with family root '{}'",
                    c.display(),
                    self.family.display()
                ));
            }
            if self
                .private_prefixes
                .iter()
                .any(|p| c.starts_with(p) || p.starts_with(&c))
            {
                return Err(format!(
                    "public prefix '{}' overlaps with private prefix",
                    c.display()
                ));
            }
            if self
                .family_prefixes
                .iter()
                .any(|p| c.starts_with(p) || p.starts_with(&c))
            {
                return Err(format!(
                    "public prefix '{}' overlaps with family prefix",
                    c.display()
                ));
            }
            canonical.push(c);
        }
        self.public_prefixes = canonical;
        Ok(self)
    }

    pub fn with_default_private(mut self, enabled: bool) -> Self {
        self.default_private = enabled;
        self
    }

    pub fn with_dot_paths_private(mut self, enabled: bool) -> Self {
        self.dot_paths_private = enabled;
        self
    }

    pub fn tier_for_path(&self, path: &Path) -> Option<ContextTier> {
        if !path.starts_with(&self.home_root) {
            return None;
        }
        if path.starts_with(&self.private) {
            return Some(ContextTier::Private);
        }
        if self.private_prefixes.iter().any(|p| path.starts_with(p)) {
            return Some(ContextTier::Private);
        }
        if self.dot_paths_private && has_dot_component_within_home(path, &self.home_root) {
            return Some(ContextTier::Private);
        }
        if path.starts_with(&self.family) {
            return Some(ContextTier::Family);
        }
        if self.family_prefixes.iter().any(|p| path.starts_with(p)) {
            return Some(ContextTier::Family);
        }
        if self.default_private {
            if self.public_prefixes.iter().any(|p| path.starts_with(p)) {
                return Some(ContextTier::Public);
            }
            return Some(ContextTier::Private);
        }
        Some(ContextTier::Public)
    }
}

#[derive(Debug, Clone)]
pub struct GatePolicy {
    roots: ContextRoots,
    mindlock_dir: PathBuf,
    network_allowlist: Vec<String>,
    allow_exec_in_private: bool,
    allow_exec_in_family: bool,
    allow_exec_in_public: bool,

    gate_action_kinds: bool,
    action_kinds_public: Vec<String>,
    action_kinds_family: Vec<String>,
    action_kinds_private: Vec<String>,

    gate_tool_identity: bool,
    tools_public: Vec<String>,
    tools_family: Vec<String>,
    tools_private: Vec<String>,

    gate_skill_identity: bool,
    skills_public: Vec<String>,
    skills_family: Vec<String>,
    skills_private: Vec<String>,

    enforce_control_commands: bool,
    control_public: Vec<String>,
    control_family: Vec<String>,
    control_private: Vec<String>,

    channel_map: HashMap<Channel, ContextTier>,

    utc_offset: i32,
    schedule_public: Option<TimeWindow>,
    schedule_family: Option<TimeWindow>,
    schedule_private: Option<TimeWindow>,
}

impl GatePolicy {
    pub fn new(roots: ContextRoots, network_allowlist: Vec<String>) -> Self {
        Self {
            roots,
            mindlock_dir: PathBuf::from("/nonexistent-mindlock-unconfigured"),
            network_allowlist,
            allow_exec_in_private: false,
            allow_exec_in_family: false,
            allow_exec_in_public: false,
            gate_action_kinds: false,
            action_kinds_public: Vec::new(),
            action_kinds_family: Vec::new(),
            action_kinds_private: Vec::new(),
            gate_tool_identity: false,
            tools_public: Vec::new(),
            tools_family: Vec::new(),
            tools_private: Vec::new(),
            gate_skill_identity: false,
            skills_public: Vec::new(),
            skills_family: Vec::new(),
            skills_private: Vec::new(),
            enforce_control_commands: true,
            control_public: vec!["*".to_string()],
            control_family: vec!["*".to_string()],
            control_private: vec!["*".to_string()],
            channel_map: HashMap::new(),
            utc_offset: 0,
            schedule_public: None,
            schedule_family: None,
            schedule_private: None,
        }
    }

    /// Build policy from a loaded Config.
    pub fn from_config(config: &Config) -> Result<Self, String> {
        let roots = ContextRoots::new(
            &config.paths.home_root,
            &config.paths.private,
            &config.paths.family,
        )?;

        let roots = if config.paths.private_prefixes.is_empty() {
            roots
        } else {
            roots.with_private_prefixes(config.paths.private_prefixes.clone())?
        };

        let roots = if config.paths.family_prefixes.is_empty() {
            roots
        } else {
            roots.with_family_prefixes(config.paths.family_prefixes.clone())?
        };

        let roots = if config.paths.public_prefixes.is_empty() {
            roots
        } else {
            roots.with_public_prefixes(config.paths.public_prefixes.clone())?
        };

        let roots = roots
            .with_dot_paths_private(config.paths.dot_paths_private)
            .with_default_private(config.paths.default_private);

        let mut channel_map = HashMap::new();
        for (channel_name, tier_name) in &config.channels.map {
            let Some(channel) = parse_channel(channel_name) else {
                return Err(format!("unknown channel in config: '{channel_name}'"));
            };
            let Some(tier) = parse_tier(tier_name) else {
                return Err(format!(
                    "unknown tier in config for channel '{channel_name}': '{tier_name}'"
                ));
            };
            channel_map.insert(channel, tier);
        }

        let gate_action_kinds = config
            .gate
            .gate_action_kinds
            .unwrap_or(config.gate.gate_tools);

        let action_kinds_public =
            list_with_fallback(&config.gate.action_kinds_public, &config.gate.tools_public);
        let action_kinds_family =
            list_with_fallback(&config.gate.action_kinds_family, &config.gate.tools_family);
        let action_kinds_private = list_with_fallback(
            &config.gate.action_kinds_private,
            &config.gate.tools_private,
        );

        Ok(Self {
            roots,
            mindlock_dir: config.security_review.mindlock_dir.clone(),
            network_allowlist: config.network.allowlist.clone(),
            allow_exec_in_private: config.exec.allow_in_private,
            allow_exec_in_family: config.exec.allow_in_family,
            allow_exec_in_public: config.exec.allow_in_public,
            gate_action_kinds,
            action_kinds_public,
            action_kinds_family,
            action_kinds_private,
            gate_tool_identity: config.gate.gate_tool_identity,
            tools_public: list_only(&config.gate.tool_ids_public),
            tools_family: list_only(&config.gate.tool_ids_family),
            tools_private: list_only(&config.gate.tool_ids_private),
            gate_skill_identity: config.gate.gate_skill_identity,
            skills_public: list_only(&config.gate.skill_ids_public),
            skills_family: list_only(&config.gate.skill_ids_family),
            skills_private: list_only(&config.gate.skill_ids_private),
            enforce_control_commands: config.control.enforce,
            control_public: config.control.public.clone(),
            control_family: config.control.family.clone(),
            control_private: config.control.private.clone(),
            channel_map,
            utc_offset: config.schedule.utc_offset,
            schedule_public: config.schedule.public.clone(),
            schedule_family: config.schedule.family.clone(),
            schedule_private: config.schedule.private.clone(),
        })
    }

    pub fn with_exec_in_private(mut self, allow: bool) -> Self {
        self.allow_exec_in_private = allow;
        self
    }

    pub fn with_exec_in_family(mut self, allow: bool) -> Self {
        self.allow_exec_in_family = allow;
        self
    }

    pub fn with_exec_in_public(mut self, allow: bool) -> Self {
        self.allow_exec_in_public = allow;
        self
    }

    pub fn with_mindlock_dir(mut self, dir: PathBuf) -> Self {
        self.mindlock_dir = dir;
        self
    }

    /// Back-compat helper: this configures primitive action-kind gating.
    pub fn with_tool_gating(
        mut self,
        gate: bool,
        public: Vec<String>,
        family: Vec<String>,
        private: Vec<String>,
    ) -> Self {
        self.gate_action_kinds = gate;
        self.action_kinds_public = public;
        self.action_kinds_family = family;
        self.action_kinds_private = private;
        self
    }

    pub fn with_tool_identity_gating(
        mut self,
        gate: bool,
        public: Vec<String>,
        family: Vec<String>,
        private: Vec<String>,
    ) -> Self {
        self.gate_tool_identity = gate;
        self.tools_public = public;
        self.tools_family = family;
        self.tools_private = private;
        self
    }

    pub fn with_skill_identity_gating(
        mut self,
        gate: bool,
        public: Vec<String>,
        family: Vec<String>,
        private: Vec<String>,
    ) -> Self {
        self.gate_skill_identity = gate;
        self.skills_public = public;
        self.skills_family = family;
        self.skills_private = private;
        self
    }

    pub fn with_schedule(
        mut self,
        utc_offset: i32,
        public: Option<TimeWindow>,
        family: Option<TimeWindow>,
        private: Option<TimeWindow>,
    ) -> Self {
        self.utc_offset = utc_offset;
        self.schedule_public = public;
        self.schedule_family = family;
        self.schedule_private = private;
        self
    }

    /// Whether exec is allowed for a given context tier (used by tool definitions).
    pub fn exec_allowed_for(&self, context: ContextTier) -> bool {
        match context {
            ContextTier::Private => self.allow_exec_in_private,
            ContextTier::Family => self.allow_exec_in_family,
            ContextTier::Public => self.allow_exec_in_public,
        }
    }

    pub fn context_for_channel(&self, channel: Channel) -> ContextTier {
        if let Some(tier) = self.channel_map.get(&channel) {
            return *tier;
        }

        from_policy_context(vericore_policy::channels::default_context_for_channel(
            to_policy_channel(channel),
        ))
    }

    pub fn integrity_for_channel(&self, channel: Channel) -> IntegrityTier {
        let ctx = self.context_for_channel(channel);
        from_policy_integrity(vericore_policy::channels::integrity_for_context(
            to_policy_context(ctx),
        ))
    }

    /// Ingress integrity check for sensitive mutations.
    ///
    /// Rules:
    /// - exec requires trusted ingress
    /// - writes to private files require trusted ingress
    /// - writes to sensitive config paths require trusted ingress
    /// Check if a raw (uncanonicalized) path is safely under mindlock_dir.
    /// Requires: absolute, lexical prefix match, no ParentDir (..) after prefix.
    fn is_raw_mindlock_write(&self, path: &Path) -> bool {
        if !path.is_absolute() {
            return false;
        }
        if !path.starts_with(&self.mindlock_dir) {
            return false;
        }
        let suffix = path
            .strip_prefix(&self.mindlock_dir)
            .unwrap_or(Path::new(""));
        !suffix
            .components()
            .any(|c| matches!(c, Component::ParentDir))
    }

    fn is_mindlock_stage_path(&self, path: &Path) -> bool {
        path.starts_with(self.mindlock_dir.join("in"))
            || path.starts_with(self.mindlock_dir.join("out"))
            || path.starts_with(self.mindlock_dir.join("work"))
    }

    pub fn check_ingress_integrity(&self, integrity: IntegrityTier, action: &Action) -> Verdict {
        let primitive = action.executable_action();

        match primitive {
            Action::Exec { .. } => {
                let policy_integrity = to_policy_integrity(integrity);
                if vericore_policy::ingress::ingress_allows_exec(policy_integrity) {
                    Verdict::allow()
                } else {
                    Verdict::deny(format!(
                        "ingress integrity {:?} cannot execute commands",
                        integrity
                    ))
                }
            }
            Action::WriteFile { path, .. } => {
                // I/O: classify the write target
                let target_class = self.classify_write_target(path);
                let policy_integrity = to_policy_integrity(integrity);
                if vericore_policy::ingress::ingress_allows_write(policy_integrity, target_class) {
                    Verdict::allow()
                } else {
                    Verdict::deny(format!(
                        "ingress integrity {:?} cannot perform sensitive write: {}",
                        integrity,
                        path.display()
                    ))
                }
            }
            _ => Verdict::allow(),
        }
    }

    /// Classify a write target path into a WriteTargetClass for the verified kernel.
    fn classify_write_target(&self, path: &Path) -> WriteTargetClass {
        // Mindlock check on raw path (parent may not exist yet)
        if self.is_raw_mindlock_write(path) {
            return WriteTargetClass::Mindlock;
        }
        match canonicalize_write_target(path) {
            Ok(canonical) => {
                let is_private = matches!(
                    self.roots.tier_for_path(&canonical),
                    Some(ContextTier::Private)
                );
                let is_sensitive = is_sensitive_config_path(&canonical);
                if is_private {
                    WriteTargetClass::Private
                } else if is_sensitive {
                    WriteTargetClass::SensitiveConfig
                } else {
                    WriteTargetClass::Other
                }
            }
            Err(_) => WriteTargetClass::Unresolved,
        }
    }

    pub fn check_action_kind(&self, context: ContextTier, action_kind: &str) -> Verdict {
        if !self.gate_action_kinds {
            return Verdict::allow();
        }

        let list = match context {
            ContextTier::Public => &self.action_kinds_public,
            ContextTier::Family => &self.action_kinds_family,
            ContextTier::Private => &self.action_kinds_private,
        };

        if list_allows(list, action_kind) {
            Verdict::allow()
        } else {
            Verdict::deny(format!(
                "action kind '{}' not allowed in {:?} context",
                action_kind, context
            ))
        }
    }

    pub fn check_tool_identity(&self, context: ContextTier, tool_name: Option<&str>) -> Verdict {
        if !self.gate_tool_identity {
            return Verdict::allow();
        }

        let Some(tool_name) = tool_name else {
            return Verdict::deny("missing tool identity while tool identity gating is enabled");
        };

        let list = match context {
            ContextTier::Public => &self.tools_public,
            ContextTier::Family => &self.tools_family,
            ContextTier::Private => &self.tools_private,
        };

        if list_allows(list, tool_name) {
            Verdict::allow()
        } else {
            Verdict::deny(format!(
                "tool '{}' not allowed in {:?} context",
                tool_name, context
            ))
        }
    }

    pub fn check_skill_identity(&self, context: ContextTier, skill_name: Option<&str>) -> Verdict {
        if !self.gate_skill_identity {
            return Verdict::allow();
        }

        let Some(skill_name) = skill_name else {
            return Verdict::deny("missing skill identity while skill identity gating is enabled");
        };

        let list = match context {
            ContextTier::Public => &self.skills_public,
            ContextTier::Family => &self.skills_family,
            ContextTier::Private => &self.skills_private,
        };

        if list_allows(list, skill_name) {
            Verdict::allow()
        } else {
            Verdict::deny(format!(
                "skill '{}' not allowed in {:?} context",
                skill_name, context
            ))
        }
    }

    pub fn check_control_command(&self, context: ContextTier, command: Option<&str>) -> Verdict {
        if !self.enforce_control_commands {
            return Verdict::allow();
        }

        let Some(command) = command else {
            return Verdict::deny("missing command while control command verification is enabled");
        };

        let list = match context {
            ContextTier::Public => &self.control_public,
            ContextTier::Family => &self.control_family,
            ContextTier::Private => &self.control_private,
        };

        if list_allows(list, command) {
            Verdict::allow()
        } else {
            Verdict::deny(format!(
                "control command '/{}' not allowed in {:?} context",
                command, context
            ))
        }
    }

    /// Wall-clock schedule check.
    pub fn check_schedule(&self, context: ContextTier) -> Verdict {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        self.check_schedule_at(context, timestamp)
    }

    /// Deterministic schedule check (useful for tests).
    pub fn check_schedule_at(&self, context: ContextTier, timestamp: u64) -> Verdict {
        let window = match context {
            ContextTier::Public => &self.schedule_public,
            ContextTier::Family => &self.schedule_family,
            ContextTier::Private => &self.schedule_private,
        };

        let Some(window) = window else {
            return Verdict::allow();
        };

        let local_secs = (timestamp as i64) + (self.utc_offset as i64 * 3600);
        let hour = ((local_secs % 86400 + 86400) % 86400 / 3600) as u8;

        let in_window = if window.allowed_start_hour <= window.allowed_end_hour {
            hour >= window.allowed_start_hour && hour < window.allowed_end_hour
        } else {
            hour >= window.allowed_start_hour || hour < window.allowed_end_hour
        };

        if in_window {
            Verdict::allow()
        } else {
            Verdict::deny(format!(
                "outside allowed hours ({:02}:00-{:02}:00) for {:?} context (current hour: {:02})",
                window.allowed_start_hour, window.allowed_end_hour, context, hour
            ))
        }
    }

    pub fn check_action(&self, context: ContextTier, action: &Action) -> Verdict {
        let primitive = action.executable_action();

        match primitive {
            Action::NoOp { .. } => Verdict::allow(),
            Action::Exec { .. } => {
                let allowed = match context {
                    ContextTier::Private => self.allow_exec_in_private,
                    ContextTier::Family => self.allow_exec_in_family,
                    ContextTier::Public => self.allow_exec_in_public,
                };
                if allowed {
                    Verdict::allow()
                } else {
                    Verdict::deny(format!("exec denied in {:?} context", context))
                }
            }
            Action::WebFetch { host, .. } => {
                if self.host_allowed(host) {
                    Verdict::allow()
                } else {
                    Verdict::deny(format!("host not allowlisted: {host}"))
                }
            }
            Action::Respond { channel, .. } => {
                let target = self.context_for_channel(*channel);
                if context.can_flow_to(target) {
                    Verdict::allow()
                } else {
                    Verdict::deny(format!(
                        "information flow denied: {:?} -> {:?}",
                        context, target
                    ))
                }
            }
            Action::ReadFile { path } => {
                let canonical = match canonicalize_existing_file(path) {
                    Ok(p) => p,
                    Err(err) => return Verdict::deny(err),
                };
                let Some(target_tier) = self.roots.tier_for_path(&canonical) else {
                    return Verdict::deny(format!("path outside home: {}", canonical.display()));
                };
                if context.can_read(target_tier) {
                    Verdict::allow()
                } else {
                    Verdict::deny(format!(
                        "read denied for {:?} on {:?} file: {}",
                        context,
                        target_tier,
                        canonical.display()
                    ))
                }
            }
            Action::ListDir { path } => {
                let canonical = match canonicalize_existing_dir(path) {
                    Ok(p) => p,
                    Err(err) => return Verdict::deny(err),
                };
                let Some(target_tier) = self.roots.tier_for_path(&canonical) else {
                    return Verdict::deny(format!("path outside home: {}", canonical.display()));
                };
                if context.can_read(target_tier) {
                    Verdict::allow()
                } else {
                    Verdict::deny(format!(
                        "list_dir denied for {:?} on {:?} dir: {}",
                        context,
                        target_tier,
                        canonical.display()
                    ))
                }
            }
            Action::WriteFile { path, .. } => {
                let canonical = match canonicalize_write_target(path) {
                    Ok(p) => p,
                    Err(err) => return Verdict::deny(err),
                };
                let Some(target_tier) = self.roots.tier_for_path(&canonical) else {
                    return Verdict::deny(format!("path outside home: {}", canonical.display()));
                };
                if context.can_write(target_tier) {
                    Verdict::allow()
                } else {
                    Verdict::deny(format!(
                        "write denied for {:?} to {:?} file: {}",
                        context,
                        target_tier,
                        canonical.display()
                    ))
                }
            }
            Action::PromoteFromMindlock {
                source_path,
                target_path,
            } => {
                let source = match canonicalize_existing_file(source_path) {
                    Ok(p) => p,
                    Err(err) => return Verdict::deny(err),
                };
                let in_root = self.mindlock_dir.join("in");
                let out_root = self.mindlock_dir.join("out");
                if !source.starts_with(in_root) && !source.starts_with(out_root) {
                    return Verdict::deny(format!(
                        "promote source must be under {}/in or {}/out: {}",
                        self.mindlock_dir.display(),
                        self.mindlock_dir.display(),
                        source.display()
                    ));
                }
                let target = match canonicalize_write_target(target_path) {
                    Ok(p) => p,
                    Err(err) => return Verdict::deny(err),
                };
                let Some(target_tier) = self.roots.tier_for_path(&target) else {
                    return Verdict::deny(format!(
                        "target path outside home: {}",
                        target.display()
                    ));
                };
                if context.can_write(target_tier) {
                    Verdict::allow()
                } else {
                    Verdict::deny(format!(
                        "promote denied for {:?} to {:?} file: {}",
                        context,
                        target_tier,
                        target.display()
                    ))
                }
            }
            Action::RequestReview {
                source_path,
                target_path,
            } => {
                if context != ContextTier::Private {
                    return Verdict::deny("request_review requires private context");
                }
                let source = match canonicalize_existing_file(source_path) {
                    Ok(p) => p,
                    Err(err) => return Verdict::deny(err),
                };
                if !self.is_mindlock_stage_path(&source) {
                    return Verdict::deny(format!(
                        "request_review source must be under {ml}/in, {ml}/out, or {ml}/work: {}",
                        source.display(),
                        ml = self.mindlock_dir.display()
                    ));
                }
                let target = match canonicalize_write_target(target_path) {
                    Ok(p) => p,
                    Err(err) => return Verdict::deny(err),
                };
                if self.roots.tier_for_path(&target).is_none() {
                    return Verdict::deny(format!(
                        "target path outside home: {}",
                        target.display()
                    ));
                }
                Verdict::allow()
            }
            Action::SelfEscalate { source_path, .. } => {
                if context != ContextTier::Private {
                    return Verdict::deny("self_escalate requires private context");
                }
                let source = match canonicalize_existing_file(source_path) {
                    Ok(p) => p,
                    Err(err) => return Verdict::deny(err),
                };
                if self.is_mindlock_stage_path(&source) {
                    Verdict::allow()
                } else {
                    Verdict::deny(format!(
                        "self_escalate source must be under {ml}/in, {ml}/out, or {ml}/work: {}",
                        source.display(),
                        ml = self.mindlock_dir.display()
                    ))
                }
            }
            Action::BrokeredTool { .. } => Verdict::allow(),
            Action::ToolAction { .. } => {
                Verdict::deny("internal error: unflattened ToolAction reached primitive gate")
            }
        }
    }

    /// Best-effort data label produced by an executed action in the current turn.
    ///
    /// Minimal Option A policy:
    /// - file and directory reads inherit path tier
    /// - exec inherits caller context (conservative)
    /// - web fetch is treated as public
    /// - writes inherit target path tier
    pub fn output_label_for_action(&self, context: ContextTier, action: &Action) -> ContextTier {
        let primitive = action.executable_action();
        match primitive {
            Action::ReadFile { path } => canonicalize_existing_file(path)
                .ok()
                .and_then(|p| self.roots.tier_for_path(&p))
                .unwrap_or(context),
            Action::ListDir { path } => canonicalize_existing_dir(path)
                .ok()
                .and_then(|p| self.roots.tier_for_path(&p))
                .unwrap_or(context),
            Action::WriteFile { path, .. } => canonicalize_write_target(path)
                .ok()
                .and_then(|p| self.roots.tier_for_path(&p))
                .unwrap_or(context),
            Action::PromoteFromMindlock { target_path, .. } => {
                canonicalize_write_target(target_path)
                    .ok()
                    .and_then(|p| self.roots.tier_for_path(&p))
                    .unwrap_or(context)
            }
            Action::RequestReview { target_path, .. } => canonicalize_write_target(target_path)
                .ok()
                .and_then(|p| self.roots.tier_for_path(&p))
                .unwrap_or(context),
            Action::SelfEscalate { .. } => ContextTier::Private,
            Action::Exec { .. } => context,
            Action::WebFetch { .. } => ContextTier::Public,
            Action::Respond { .. }
            | Action::NoOp { .. }
            | Action::ToolAction { .. }
            | Action::BrokeredTool { .. } => ContextTier::Public,
        }
    }

    fn host_allowed(&self, host: &str) -> bool {
        let host = normalize_host(host);
        if self.network_allowlist.is_empty() {
            // Open policy: allow all external hosts, block local/private (SSRF guard)
            !is_local_or_private_host(&host)
        } else {
            // Allowlist policy: only listed hosts pass
            self.network_allowlist.iter().any(|allowed| {
                let allowed = normalize_host(allowed);
                host == allowed || host.ends_with(&format!(".{allowed}"))
            })
        }
    }
}

/// Convert vericore-core IntegrityTier to vericore-policy IntegrityTier.
/// Convert vericore-core Channel to vericore-policy Channel.
fn to_policy_channel(channel: Channel) -> vericore_policy::channels::Channel {
    match channel {
        Channel::TelegramPublic => vericore_policy::channels::Channel::TelegramPublic,
        Channel::TelegramFamily => vericore_policy::channels::Channel::TelegramFamily,
        Channel::TelegramDm => vericore_policy::channels::Channel::TelegramDm,
        Channel::Terminal => vericore_policy::channels::Channel::Terminal,
        Channel::Internal => vericore_policy::channels::Channel::Internal,
        Channel::Api => vericore_policy::channels::Channel::Api,
        Channel::Moltbook => vericore_policy::channels::Channel::Moltbook,
    }
}

/// Convert vericore-policy ContextTier to vericore-core ContextTier.
fn from_policy_context(ctx: vericore_policy::tiers::ContextTier) -> ContextTier {
    match ctx {
        vericore_policy::tiers::ContextTier::Public => ContextTier::Public,
        vericore_policy::tiers::ContextTier::Family => ContextTier::Family,
        vericore_policy::tiers::ContextTier::Private => ContextTier::Private,
    }
}

/// Convert vericore-core ContextTier to vericore-policy ContextTier.
fn to_policy_context(ctx: ContextTier) -> vericore_policy::tiers::ContextTier {
    match ctx {
        ContextTier::Public => vericore_policy::tiers::ContextTier::Public,
        ContextTier::Family => vericore_policy::tiers::ContextTier::Family,
        ContextTier::Private => vericore_policy::tiers::ContextTier::Private,
    }
}

/// Convert vericore-policy IntegrityTier to vericore-core IntegrityTier.
fn from_policy_integrity(i: vericore_policy::tiers::IntegrityTier) -> IntegrityTier {
    match i {
        vericore_policy::tiers::IntegrityTier::Untrusted => IntegrityTier::Untrusted,
        vericore_policy::tiers::IntegrityTier::Reviewed => IntegrityTier::Reviewed,
        vericore_policy::tiers::IntegrityTier::Trusted => IntegrityTier::Trusted,
    }
}

fn to_policy_integrity(i: IntegrityTier) -> vericore_policy::tiers::IntegrityTier {
    match i {
        IntegrityTier::Untrusted => vericore_policy::tiers::IntegrityTier::Untrusted,
        IntegrityTier::Reviewed => vericore_policy::tiers::IntegrityTier::Reviewed,
        IntegrityTier::Trusted => vericore_policy::tiers::IntegrityTier::Trusted,
    }
}

fn parse_channel(raw: &str) -> Option<Channel> {
    let key = raw.trim().to_ascii_lowercase().replace('-', "_");
    match key.as_str() {
        "telegram_public" => Some(Channel::TelegramPublic),
        "telegram_family" => Some(Channel::TelegramFamily),
        "telegram_dm" => Some(Channel::TelegramDm),
        "terminal" => Some(Channel::Terminal),
        "internal" => Some(Channel::Internal),
        "api" | "web" => Some(Channel::Api),
        "moltbook" => Some(Channel::Moltbook),
        _ => None,
    }
}

fn parse_tier(raw: &str) -> Option<ContextTier> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "public" => Some(ContextTier::Public),
        "family" => Some(ContextTier::Family),
        "private" => Some(ContextTier::Private),
        _ => None,
    }
}

fn list_only(list: &Option<StringList>) -> Vec<String> {
    list.as_ref().map(|l| l.allow.clone()).unwrap_or_default()
}

fn list_with_fallback(primary: &Option<StringList>, fallback: &Option<StringList>) -> Vec<String> {
    if let Some(primary) = primary {
        return primary.allow.clone();
    }
    if let Some(fallback) = fallback {
        return fallback.allow.clone();
    }
    Vec::new()
}

fn list_allows(list: &[String], value: &str) -> bool {
    list.iter()
        .any(|entry| entry == "*" || entry.eq_ignore_ascii_case(value))
}

fn canonicalize_dir(path: &Path) -> Result<PathBuf, String> {
    let canonical = fs::canonicalize(path)
        .map_err(|err| format!("failed to canonicalize '{}': {err}", path.display()))?;
    if !canonical.is_dir() {
        return Err(format!("expected directory: {}", canonical.display()));
    }
    Ok(canonical)
}

fn canonicalize_existing_file(path: &Path) -> Result<PathBuf, String> {
    if !path.is_absolute() {
        return Err(format!("path must be absolute: {}", path.display()));
    }
    let canonical = fs::canonicalize(path)
        .map_err(|err| format!("failed to canonicalize '{}': {err}", path.display()))?;
    if !canonical.is_file() {
        return Err(format!("expected file: {}", canonical.display()));
    }
    Ok(canonical)
}

fn canonicalize_existing_dir(path: &Path) -> Result<PathBuf, String> {
    if !path.is_absolute() {
        return Err(format!("path must be absolute: {}", path.display()));
    }
    let canonical = fs::canonicalize(path)
        .map_err(|err| format!("failed to canonicalize '{}': {err}", path.display()))?;
    if !canonical.is_dir() {
        return Err(format!("expected directory: {}", canonical.display()));
    }
    Ok(canonical)
}

fn canonicalize_write_target(path: &Path) -> Result<PathBuf, String> {
    if !path.is_absolute() {
        return Err(format!("path must be absolute: {}", path.display()));
    }
    let file_name = path
        .file_name()
        .ok_or_else(|| format!("write target must include file name: {}", path.display()))?;
    let parent = path.parent().ok_or_else(|| {
        format!(
            "write target must have parent directory: {}",
            path.display()
        )
    })?;
    let canonical_parent = fs::canonicalize(parent).map_err(|err| {
        format!(
            "failed to canonicalize parent '{}': {err}",
            parent.display()
        )
    })?;
    Ok(canonical_parent.join(file_name))
}

fn has_dot_component_within_home(path: &Path, home_root: &Path) -> bool {
    let Ok(rel) = path.strip_prefix(home_root) else {
        return false;
    };
    rel.components().any(|c| match c {
        Component::Normal(name) => name.to_string_lossy().starts_with('.'),
        _ => false,
    })
}

fn is_sensitive_config_path(path: &Path) -> bool {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .map(|name| name.to_ascii_lowercase());

    if let Some(name) = file_name {
        if matches!(
            name.as_str(),
            "openclaw.json" | "vericore.toml" | ".env" | "agents.md" | "soul.md"
        ) {
            return true;
        }
    }

    path.components().any(|component| {
        let value = component.as_os_str().to_string_lossy();
        value == ".BGIseed-state"
            || value == "credentials"
            || value == "vericore-core"
            || value == "vericore.toml"
    })
}

fn normalize_host(host: &str) -> String {
    host.trim().trim_end_matches('.').to_ascii_lowercase()
}

/// Returns true if the host is local, loopback, link-local, or RFC-1918 private.
/// Used as an SSRF guard when the network allowlist is empty (open external policy).
fn is_local_or_private_host(host: &str) -> bool {
    // Exact local names
    let local_names = ["localhost", "localhost.localdomain", "broadcasthost"];
    if local_names.contains(&host) {
        return true;
    }

    // Local TLD suffixes
    let local_suffixes = [".local", ".internal", ".home.arpa", ".localdomain"];
    for suffix in &local_suffixes {
        if host.ends_with(suffix) {
            return true;
        }
    }

    // Try IPv4 parsing
    if let Ok(addr) = host.parse::<std::net::Ipv4Addr>() {
        let octets = addr.octets();
        return matches!(
            octets,
            // Loopback 127.0.0.0/8
            [127, ..] |
            // Link-local 169.254.0.0/16
            [169, 254, ..] |
            // RFC-1918: 10.0.0.0/8
            [10, ..] |
            // RFC-1918: 192.168.0.0/16
            [192, 168, ..] |
            // Unspecified
            [0, 0, 0, 0]
        ) || (octets[0] == 172 && octets[1] >= 16 && octets[1] <= 31); // 172.16.0.0/12
    }

    // Try IPv6 parsing
    if let Ok(addr) = host.parse::<std::net::Ipv6Addr>() {
        let segs = addr.segments();
        // ::1 loopback
        if addr == std::net::Ipv6Addr::LOCALHOST {
            return true;
        }
        // fe80::/10 link-local
        if segs[0] & 0xffc0 == 0xfe80 {
            return true;
        }
        // fc00::/7 ULA
        if segs[0] & 0xfe00 == 0xfc00 {
            return true;
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use crate::config::Config;
    use crate::types::{Action, Channel, ContextTier, IntegrityTier};

    use super::GatePolicy;

    fn make_test_dirs() -> (std::path::PathBuf, std::path::PathBuf, std::path::PathBuf) {
        let id = format!(
            "vericore-policy-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        );
        let base = std::env::temp_dir().join(id);
        let private = base.join("private");
        let family = base.join("family");
        fs::create_dir_all(&private).expect("private dir");
        fs::create_dir_all(&family).expect("family dir");
        (base, private, family)
    }

    fn make_test_dirs_with_mindlock() -> (PathBuf, PathBuf, PathBuf, PathBuf) {
        let (base, private, family) = make_test_dirs();
        let mindlock = base.join("mindlock");
        fs::create_dir_all(mindlock.join("in")).expect("mindlock/in");
        fs::create_dir_all(mindlock.join("out")).expect("mindlock/out");
        fs::create_dir_all(mindlock.join("work")).expect("mindlock/work");
        (base, private, family, mindlock)
    }

    #[test]
    fn channel_map_from_config_overrides_default() {
        let (base, private, family) = make_test_dirs();
        let cfg = format!(
            r#"
[paths]
home_root = "{}"
private = "{}"
family = "{}"

[network]
allowlist = []

[exec]
allow_in_private = true
allow_in_family = false
allow_in_public = false

[channels.map]
telegram_public = "family"

[gate]
gate_action_kinds = false
gate_tool_identity = false
gate_skill_identity = false

[schedule]
utc_offset = 0
"#,
            base.display(),
            private.display(),
            family.display(),
        );

        let config: Config = toml::from_str(&cfg).expect("parse config");
        let policy = GatePolicy::from_config(&config).expect("build policy");
        assert_eq!(
            policy.context_for_channel(Channel::TelegramPublic),
            ContextTier::Family
        );
    }

    #[test]
    fn unknown_channel_in_config_is_rejected() {
        let (base, private, family) = make_test_dirs();
        let cfg = format!(
            r#"
[paths]
home_root = "{}"
private = "{}"
family = "{}"

[network]
allowlist = []

[exec]
allow_in_private = true
allow_in_family = false
allow_in_public = false

[channels.map]
unknown_channel = "public"

[gate]
gate_action_kinds = false
gate_tool_identity = false
gate_skill_identity = false

[schedule]
utc_offset = 0
"#,
            base.display(),
            private.display(),
            family.display(),
        );

        let config: Config = toml::from_str(&cfg).expect("parse config");
        let err = GatePolicy::from_config(&config).expect_err("must reject unknown channel");
        assert!(err.contains("unknown channel"));
    }

    #[test]
    fn control_command_defaults_allow_all() {
        let (base, private, family) = make_test_dirs();
        let roots = super::ContextRoots::new(&base, &private, &family).expect("roots");
        let policy = GatePolicy::new(roots, vec![]);

        assert!(
            policy
                .check_control_command(ContextTier::Public, Some("model"))
                .is_allowed()
        );
        assert!(
            policy
                .check_control_command(ContextTier::Family, Some("restart"))
                .is_allowed()
        );
        assert!(
            policy
                .check_control_command(ContextTier::Private, Some("credits"))
                .is_allowed()
        );
    }

    #[test]
    fn control_command_config_applies_per_context() {
        let (base, private, family) = make_test_dirs();
        let cfg = format!(
            r#"
[paths]
home_root = "{}"
private = "{}"
family = "{}"

[network]
allowlist = []

[exec]
allow_in_private = true
allow_in_family = false
allow_in_public = false

[channels.map]
telegram_public = "public"

[gate]
gate_action_kinds = false
gate_tool_identity = false
gate_skill_identity = false

[control]
enforce = true
public = ["help", "model"]
family = ["help", "model", "status"]
private = ["*"]

[schedule]
utc_offset = 0
"#,
            base.display(),
            private.display(),
            family.display(),
        );

        let config: Config = toml::from_str(&cfg).expect("parse config");
        let policy = GatePolicy::from_config(&config).expect("build policy");

        assert!(
            policy
                .check_control_command(ContextTier::Public, Some("help"))
                .is_allowed()
        );
        assert!(
            policy
                .check_control_command(ContextTier::Family, Some("status"))
                .is_allowed()
        );
        assert!(
            policy
                .check_control_command(ContextTier::Private, Some("restart"))
                .is_allowed()
        );

        assert!(
            !policy
                .check_control_command(ContextTier::Public, Some("restart"))
                .is_allowed()
        );
        assert!(
            !policy
                .check_control_command(ContextTier::Family, Some("restart"))
                .is_allowed()
        );
    }

    #[test]
    fn integrity_mapping_follows_context_mapping() {
        let (base, private, family) = make_test_dirs();
        let roots = super::ContextRoots::new(&base, &private, &family).expect("roots");
        let policy = GatePolicy::new(roots, vec![]);

        assert_eq!(
            policy.integrity_for_channel(Channel::TelegramPublic),
            IntegrityTier::Untrusted
        );
        assert_eq!(
            policy.integrity_for_channel(Channel::TelegramFamily),
            IntegrityTier::Reviewed
        );
        assert_eq!(
            policy.integrity_for_channel(Channel::TelegramDm),
            IntegrityTier::Trusted
        );
    }

    #[test]
    fn untrusted_ingress_cannot_exec_or_sensitive_write() {
        let (base, private, family) = make_test_dirs();
        let roots = super::ContextRoots::new(&base, &private, &family).expect("roots");
        let policy = GatePolicy::new(roots, vec![]).with_exec_in_public(true);

        let exec_verdict = policy.check_ingress_integrity(
            IntegrityTier::Untrusted,
            &Action::Exec {
                command: "echo hi".to_string(),
            },
        );
        assert!(!exec_verdict.is_allowed());

        let private_write = policy.check_ingress_integrity(
            IntegrityTier::Untrusted,
            &Action::WriteFile {
                path: base.join("private/blocked.txt"),
                content: "x".to_string(),
            },
        );
        assert!(!private_write.is_allowed());

        let public_write = policy.check_ingress_integrity(
            IntegrityTier::Untrusted,
            &Action::WriteFile {
                path: base.join("notes.txt"),
                content: "x".to_string(),
            },
        );
        assert!(public_write.is_allowed());

        let _ = family;
    }

    #[test]
    fn untrusted_ingress_can_write_into_mindlock() {
        let (base, private, family, mindlock) = make_test_dirs_with_mindlock();
        let roots = super::ContextRoots::new(&base, &private, &family).expect("roots");
        let policy = GatePolicy::new(roots, vec![]).with_mindlock_dir(mindlock.clone());

        let verdict = policy.check_ingress_integrity(
            IntegrityTier::Untrusted,
            &Action::WriteFile {
                path: mindlock.join("in/artifact.txt"),
                content: "x".to_string(),
            },
        );
        assert!(verdict.is_allowed());
    }

    #[test]
    fn untrusted_ingress_denied_for_nonexistent_private_path() {
        let (base, private, family, mindlock) = make_test_dirs_with_mindlock();
        let roots = super::ContextRoots::new(&base, &private, &family).expect("roots");
        let policy = GatePolicy::new(roots, vec![]).with_mindlock_dir(mindlock);

        // Parent doesn't exist -> canonicalization fails -> must deny
        let verdict = policy.check_ingress_integrity(
            IntegrityTier::Untrusted,
            &Action::WriteFile {
                path: private.join("nonexistent-subdir/evil.sh"),
                content: "malicious".to_string(),
            },
        );
        assert!(!verdict.is_allowed());
    }

    #[test]
    fn mindlock_traversal_bypass_denied() {
        let (base, private, family, mindlock) = make_test_dirs_with_mindlock();
        let roots = super::ContextRoots::new(&base, &private, &family).expect("roots");
        let policy = GatePolicy::new(roots, vec![]).with_mindlock_dir(mindlock.clone());

        // Traversal: mindlock/../private/evil.sh should NOT be treated as mindlock write
        let traversal_path = mindlock.join("../private/evil.sh");
        let verdict = policy.check_ingress_integrity(
            IntegrityTier::Untrusted,
            &Action::WriteFile {
                path: traversal_path,
                content: "malicious".to_string(),
            },
        );
        assert!(!verdict.is_allowed(), "traversal bypass must be denied");
    }

    #[test]
    fn deep_mindlock_write_allowed() {
        let (base, private, family, mindlock) = make_test_dirs_with_mindlock();
        let roots = super::ContextRoots::new(&base, &private, &family).expect("roots");
        let policy = GatePolicy::new(roots, vec![]).with_mindlock_dir(mindlock.clone());

        let verdict = policy.check_ingress_integrity(
            IntegrityTier::Untrusted,
            &Action::WriteFile {
                path: mindlock.join("in/subdir/file.txt"),
                content: "ok".to_string(),
            },
        );
        assert!(verdict.is_allowed());
    }

    #[test]
    fn custom_mindlock_root_is_honored() {
        let (base, private, family) = make_test_dirs();
        let custom_ml = base.join("mindlock-custom");
        fs::create_dir_all(custom_ml.join("in")).expect("custom mindlock/in");
        let roots = super::ContextRoots::new(&base, &private, &family).expect("roots");
        let policy = GatePolicy::new(roots, vec![]).with_mindlock_dir(custom_ml.clone());

        // Write to custom mindlock root -> allow
        let allowed = policy.check_ingress_integrity(
            IntegrityTier::Untrusted,
            &Action::WriteFile {
                path: custom_ml.join("in/file.txt"),
                content: "ok".to_string(),
            },
        );
        assert!(allowed.is_allowed(), "custom mindlock root must be honored");

        // Write to some other path that looks like default mindlock -> deny (not configured)
        let other = base.join("mindlock-wrong/in/file.txt");
        let denied = policy.check_ingress_integrity(
            IntegrityTier::Untrusted,
            &Action::WriteFile {
                path: other,
                content: "x".to_string(),
            },
        );
        // Should deny because it's not under the configured mindlock, and parent doesn't exist
        assert!(
            !denied.is_allowed(),
            "non-configured mindlock path must deny"
        );
    }

    #[test]
    fn trusted_ingress_can_write_sensitive_config_path() {
        let (base, private, family) = make_test_dirs();
        let state_dir = base.join(".BGIseed-state");
        fs::create_dir_all(&state_dir).expect("state dir");

        let roots = super::ContextRoots::new(&base, &private, &family).expect("roots");
        let policy = GatePolicy::new(roots, vec![]);

        let denied = policy.check_ingress_integrity(
            IntegrityTier::Untrusted,
            &Action::WriteFile {
                path: state_dir.join("openclaw.json"),
                content: "{}".to_string(),
            },
        );
        assert!(!denied.is_allowed());

        let allowed = policy.check_ingress_integrity(
            IntegrityTier::Trusted,
            &Action::WriteFile {
                path: state_dir.join("openclaw.json"),
                content: "{}".to_string(),
            },
        );
        assert!(allowed.is_allowed());
    }

    #[test]
    fn default_private_mode_requires_explicit_public_prefix() {
        let (base, private, family) = make_test_dirs();
        let public = base.join("public");
        fs::create_dir_all(&public).expect("public dir");

        let roots = super::ContextRoots::new(&base, &private, &family)
            .expect("roots")
            .with_public_prefixes(vec![public.clone()])
            .expect("public prefixes")
            .with_default_private(true);

        assert_eq!(
            roots.tier_for_path(&public.join("note.txt")),
            Some(ContextTier::Public)
        );
        assert_eq!(
            roots.tier_for_path(&base.join("repo/readme.md")),
            Some(ContextTier::Private)
        );
    }

    #[test]
    fn public_prefix_cannot_overlap_private_root() {
        let (base, private, family) = make_test_dirs();
        let nested = private.join("public-ish");
        fs::create_dir_all(&nested).expect("nested");

        let roots = super::ContextRoots::new(&base, &private, &family).expect("roots");
        let err = roots
            .with_public_prefixes(vec![nested])
            .expect_err("must reject overlap");
        assert!(err.contains("overlaps with private root"));
    }

    // ── host_allowed / SSRF guard tests ──────────────────────────────────────

    fn policy_with_allowlist(hosts: &[&str]) -> super::GatePolicy {
        let (base, private, family) = make_test_dirs();
        let roots = super::ContextRoots::new(&base, &private, &family).expect("roots");
        let allowlist: Vec<String> = hosts.iter().map(|s| s.to_string()).collect();
        super::GatePolicy::new(roots, allowlist)
    }

    #[test]
    fn open_policy_allows_public_hosts() {
        let policy = policy_with_allowlist(&[]);
        for host in &["github.com", "docs.rs", "openrouter.ai", "api.example.com"] {
            assert!(
                policy.host_allowed(host),
                "expected {host} to be allowed under open policy"
            );
        }
    }

    #[test]
    fn open_policy_blocks_local_and_private_hosts() {
        let policy = policy_with_allowlist(&[]);
        let blocked = [
            "localhost",
            "localhost.localdomain",
            "127.0.0.1",
            "127.0.0.2",
            "10.0.0.5",
            "192.168.1.8",
            "172.16.1.2",
            "172.31.255.255",
            "169.254.169.254",
            "0.0.0.0",
            "::1",
            "fe80::1",
            "fc00::1",
            "internal.local",
            "myhost.internal",
        ];
        for host in &blocked {
            assert!(
                !policy.host_allowed(host),
                "expected {host} to be blocked under open policy (SSRF guard)"
            );
        }
    }

    #[test]
    fn allowlist_policy_allows_listed_and_subdomains() {
        let policy = policy_with_allowlist(&["openrouter.ai", "moltbook.com"]);
        assert!(policy.host_allowed("openrouter.ai"));
        assert!(policy.host_allowed("api.openrouter.ai"));
        assert!(policy.host_allowed("moltbook.com"));
        assert!(policy.host_allowed("www.moltbook.com"));
    }

    #[test]
    fn allowlist_policy_blocks_unlisted_external_hosts() {
        let policy = policy_with_allowlist(&["openrouter.ai"]);
        assert!(!policy.host_allowed("github.com"));
        assert!(!policy.host_allowed("docs.rs"));
    }
}
