use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize)]
pub struct Config {
    pub paths: PathsConfig,
    pub network: NetworkConfig,
    pub exec: ExecConfig,
    pub channels: ChannelsConfig,
    pub gate: GateConfig,
    #[serde(default)]
    pub control: ControlConfig,
    #[serde(default)]
    pub schedule: ScheduleConfig,
    #[serde(default)]
    pub llm: LlmConfig,
    #[serde(default)]
    pub turn: TurnConfig,
    #[serde(default)]
    pub system_prompt: SystemPromptConfig,
    #[serde(default)]
    pub security_review: SecurityReviewConfig,
}

#[derive(Debug, Deserialize)]
pub struct PathsConfig {
    pub home_root: PathBuf,
    pub private: PathBuf,
    pub family: PathBuf,
    #[serde(default)]
    pub private_prefixes: Vec<PathBuf>,
    #[serde(default)]
    pub family_prefixes: Vec<PathBuf>,
    #[serde(default)]
    pub public_prefixes: Vec<PathBuf>,
    #[serde(default = "default_true")]
    pub dot_paths_private: bool,
    #[serde(default)]
    pub default_private: bool,
}

#[derive(Debug, Deserialize)]
pub struct NetworkConfig {
    #[serde(default)]
    pub allowlist: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct ExecConfig {
    #[serde(default)]
    pub allow_in_private: bool,
    #[serde(default)]
    pub allow_in_family: bool,
    #[serde(default)]
    pub allow_in_public: bool,
}

#[derive(Debug, Deserialize)]
pub struct ChannelsConfig {
    #[serde(default)]
    pub map: HashMap<String, String>,
}

#[derive(Debug, Deserialize)]
pub struct GateConfig {
    #[serde(default)]
    pub gate_tools: bool,
    #[serde(default)]
    pub gate_action_kinds: Option<bool>,
    #[serde(default)]
    pub action_kinds_public: Option<StringList>,
    #[serde(default)]
    pub action_kinds_family: Option<StringList>,
    #[serde(default)]
    pub action_kinds_private: Option<StringList>,
    #[serde(default)]
    pub tools_public: Option<StringList>,
    #[serde(default)]
    pub tools_family: Option<StringList>,
    #[serde(default)]
    pub tools_private: Option<StringList>,
    #[serde(default)]
    pub gate_tool_identity: bool,
    #[serde(default)]
    pub tool_ids_public: Option<StringList>,
    #[serde(default)]
    pub tool_ids_family: Option<StringList>,
    #[serde(default)]
    pub tool_ids_private: Option<StringList>,
    #[serde(default)]
    pub gate_skill_identity: bool,
    #[serde(default)]
    pub skill_ids_public: Option<StringList>,
    #[serde(default)]
    pub skill_ids_family: Option<StringList>,
    #[serde(default)]
    pub skill_ids_private: Option<StringList>,
}

#[derive(Debug, Deserialize)]
pub struct ControlConfig {
    #[serde(default = "default_true")]
    pub enforce: bool,
    #[serde(default = "default_wildcard_list")]
    pub public: Vec<String>,
    #[serde(default = "default_wildcard_list")]
    pub family: Vec<String>,
    #[serde(default = "default_wildcard_list")]
    pub private: Vec<String>,
}

impl Default for ControlConfig {
    fn default() -> Self {
        Self {
            enforce: default_true(),
            public: default_wildcard_list(),
            family: default_wildcard_list(),
            private: default_wildcard_list(),
        }
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct StringList {
    #[serde(default)]
    pub allow: Vec<String>,
}

#[derive(Debug, Deserialize, Default, Clone)]
pub struct ScheduleConfig {
    #[serde(default)]
    pub utc_offset: i32,
    #[serde(default)]
    pub public: Option<TimeWindow>,
    #[serde(default)]
    pub family: Option<TimeWindow>,
    #[serde(default)]
    pub private: Option<TimeWindow>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct TimeWindow {
    pub allowed_start_hour: u8,
    pub allowed_end_hour: u8,
}

#[derive(Debug, Deserialize, Clone)]
pub struct LlmConfig {
    #[serde(default = "default_model")]
    pub model: String,
    #[serde(default = "default_api_base")]
    pub api_base: String,
    #[serde(default = "default_api_key_env")]
    pub api_key_env: String,
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u32,
    #[serde(default = "default_temperature")]
    pub temperature: f64,
    #[serde(default = "default_llm_timeout")]
    pub timeout_secs: u64,
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            model: default_model(),
            api_base: default_api_base(),
            api_key_env: default_api_key_env(),
            max_tokens: default_max_tokens(),
            temperature: default_temperature(),
            timeout_secs: default_llm_timeout(),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct TurnConfig {
    #[serde(default = "default_max_iterations")]
    pub max_iterations: u32,
    #[serde(default = "default_gas_budget")]
    pub gas_budget: u64,
    #[serde(default = "default_exec_timeout")]
    pub exec_timeout_secs: u64,
    #[serde(default = "default_max_tool_calls")]
    pub max_tool_calls: u32,
    #[serde(default = "default_true")]
    pub finalize_without_tools_on_limit: bool,
    #[serde(default = "default_history_messages_max")]
    pub history_messages_max: usize,
}

impl Default for TurnConfig {
    fn default() -> Self {
        Self {
            max_iterations: default_max_iterations(),
            gas_budget: default_gas_budget(),
            exec_timeout_secs: default_exec_timeout(),
            max_tool_calls: default_max_tool_calls(),
            finalize_without_tools_on_limit: default_true(),
            history_messages_max: default_history_messages_max(),
        }
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct SecurityReviewConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub egress_prompt_file: Option<PathBuf>,
    #[serde(default)]
    pub ingress_prompt_file: Option<PathBuf>,
    #[serde(default)]
    pub reviewer_context_file: Option<PathBuf>,
    #[serde(default = "default_max_revision_attempts")]
    pub max_revision_attempts: u32,
    #[serde(default = "default_mindlock_dir")]
    pub mindlock_dir: PathBuf,
    #[serde(default)]
    pub trusted_write_prefixes: Vec<PathBuf>,
    #[serde(default = "default_max_precedent_items")]
    pub max_precedent_items: usize,
    #[serde(default)]
    pub llm: Option<LlmConfig>,
}

impl Default for SecurityReviewConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            egress_prompt_file: None,
            ingress_prompt_file: None,
            reviewer_context_file: None,
            max_revision_attempts: default_max_revision_attempts(),
            mindlock_dir: default_mindlock_dir(),
            trusted_write_prefixes: Vec::new(),
            max_precedent_items: default_max_precedent_items(),
            llm: None,
        }
    }
}

#[derive(Debug, Deserialize, Default)]
pub struct SystemPromptConfig {
    #[serde(default)]
    pub file: Option<PathBuf>,
    #[serde(default)]
    pub inline: Option<String>,
}

impl Config {
    pub fn load(path: &Path) -> Result<Self, String> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| format!("failed to read config '{}': {e}", path.display()))?;
        toml::from_str(&content)
            .map_err(|e| format!("failed to parse config '{}': {e}", path.display()))
    }

    pub fn load_system_prompt(&self) -> Result<String, String> {
        if let Some(path) = &self.system_prompt.file {
            std::fs::read_to_string(path)
                .map_err(|e| format!("failed to read system prompt '{}': {e}", path.display()))
        } else if let Some(inline) = &self.system_prompt.inline {
            Ok(inline.clone())
        } else {
            Ok("You are a helpful assistant.".into())
        }
    }
}

fn default_true() -> bool {
    true
}
fn default_model() -> String {
    "x-ai/grok-4.1-fast".into()
}
fn default_api_base() -> String {
    "https://openrouter.ai/api/v1".into()
}
fn default_api_key_env() -> String {
    "OPENROUTER_API_KEY".into()
}
fn default_max_tokens() -> u32 {
    4096
}
fn default_temperature() -> f64 {
    0.7
}
fn default_llm_timeout() -> u64 {
    120
}
fn default_max_iterations() -> u32 {
    20
}
fn default_gas_budget() -> u64 {
    200
}
fn default_exec_timeout() -> u64 {
    60
}
fn default_max_tool_calls() -> u32 {
    64
}
fn default_history_messages_max() -> usize {
    40
}
fn default_wildcard_list() -> Vec<String> {
    vec!["*".to_string()]
}

fn default_max_revision_attempts() -> u32 {
    1
}
fn default_max_precedent_items() -> usize {
    50
}

fn default_mindlock_dir() -> PathBuf {
    std::env::var("HOME")
        .map(|h| PathBuf::from(h).join("mindlock"))
        .unwrap_or_else(|_| PathBuf::from("/tmp/mindlock"))
}
