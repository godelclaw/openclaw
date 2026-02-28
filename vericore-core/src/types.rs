use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ContextTier {
    Public,
    Family,
    Private,
}

impl ContextTier {
    pub fn rank(self) -> u8 {
        match self {
            ContextTier::Public => 0,
            ContextTier::Family => 1,
            ContextTier::Private => 2,
        }
    }

    pub fn can_read(self, target: ContextTier) -> bool {
        target.rank() <= self.rank()
    }

    pub fn can_write(self, target: ContextTier) -> bool {
        self.can_read(target)
    }

    pub fn can_flow_to(self, target: ContextTier) -> bool {
        self.rank() <= target.rank()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Channel {
    TelegramPublic,
    TelegramFamily,
    TelegramDm,
    Terminal,
    Internal,
    Api,
    Moltbook,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stimulus {
    pub channel: Channel,
    pub actor: String,
    pub content: String,
    pub timestamp: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Respond {
        channel: Channel,
        recipient: String,
        content: String,
    },
    ReadFile {
        path: PathBuf,
    },
    ListDir {
        path: PathBuf,
    },
    WriteFile {
        path: PathBuf,
        content: String,
    },
    Exec {
        command: String,
    },
    WebFetch {
        host: String,
        path: String,
    },
    /// Optional identity wrapper to support tool/skill identity gating
    /// while still enforcing primitive action checks.
    ToolAction {
        tool_name: String,
        skill_name: Option<String>,
        action: Box<Action>,
    },
    NoOp {
        reason: String,
    },
}

impl Action {
    /// Primitive action kind used for capability gating.
    pub fn kind(&self) -> &'static str {
        match self {
            Action::Respond { .. } => "respond",
            Action::ReadFile { .. } => "read_file",
            Action::ListDir { .. } => "list_dir",
            Action::WriteFile { .. } => "write_file",
            Action::Exec { .. } => "exec",
            Action::WebFetch { .. } => "web_fetch",
            Action::ToolAction { action, .. } => action.kind(),
            Action::NoOp { .. } => "noop",
        }
    }

    pub fn tool_name(&self) -> Option<&str> {
        match self {
            Action::ToolAction { tool_name, .. } => Some(tool_name.as_str()),
            _ => None,
        }
    }

    pub fn skill_name(&self) -> Option<&str> {
        match self {
            Action::ToolAction { skill_name, .. } => skill_name.as_deref(),
            _ => None,
        }
    }

    /// Action actually executed after gate allow.
    pub fn executable_action(&self) -> &Action {
        match self {
            Action::ToolAction { action, .. } => action.executable_action(),
            _ => self,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Allow,
    Deny { reason: String },
}

impl Verdict {
    pub fn allow() -> Self {
        Verdict::Allow
    }

    pub fn deny(reason: impl Into<String>) -> Self {
        Verdict::Deny {
            reason: reason.into(),
        }
    }

    pub fn is_allowed(&self) -> bool {
        matches!(self, Verdict::Allow)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    Executed { kind: String },
    Denied { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditEntry {
    pub seq: u64,
    pub context: ContextTier,
    pub stimulus_channel: Channel,
    pub stimulus_actor: String,
    pub action: Action,
    pub verdict: Verdict,
    pub gas_before: u64,
    pub gas_after: u64,
}
