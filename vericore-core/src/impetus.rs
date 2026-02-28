use serde::{Deserialize, Serialize};

use crate::policy::GatePolicy;
use crate::types::{Channel, ContextTier, Verdict};

#[derive(Debug, Clone, Deserialize)]
pub struct StimulusInput {
    pub channel: String,
    pub actor: String,
    pub content: String,
    pub timestamp: Option<u64>,
    #[serde(default)]
    pub session_key: Option<String>,
    #[serde(default)]
    pub route_preference: Option<RoutePreference>,
}

#[derive(Debug, Clone, Serialize)]
pub struct StimulusDecision {
    pub allow: bool,
    pub context: String,
    pub channel: String,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RoutePreference {
    Off,
    Gate,
    Driver,
    Fallback,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RouteTarget {
    Control,
    Driver,
    Fallback,
}

#[derive(Debug, Clone, Serialize)]
pub struct StimulusRouteDecision {
    pub allow: bool,
    pub context: String,
    pub channel: String,
    pub route: RouteTarget,
    pub reason: Option<String>,
    pub is_control: bool,
    pub command: Option<String>,
    pub session_key: String,
}

pub fn decide_stimulus(policy: &GatePolicy, input: &StimulusInput) -> StimulusDecision {
    let parsed_channel = parse_channel_name(&input.channel);
    let Ok(channel) = parsed_channel else {
        return StimulusDecision {
            allow: false,
            context: "unknown".to_string(),
            channel: input.channel.clone(),
            reason: Some(parsed_channel.expect_err("checked err")),
        };
    };

    let context = policy.context_for_channel(channel);
    let schedule_verdict = policy.check_schedule(context);

    match schedule_verdict {
        Verdict::Allow => StimulusDecision {
            allow: true,
            context: context_label(context).to_string(),
            channel: normalized_channel_label(channel).to_string(),
            reason: None,
        },
        Verdict::Deny { reason } => StimulusDecision {
            allow: false,
            context: context_label(context).to_string(),
            channel: normalized_channel_label(channel).to_string(),
            reason: Some(reason),
        },
    }
}

pub fn route_stimulus(policy: &GatePolicy, input: &StimulusInput) -> StimulusRouteDecision {
    let parsed_channel = parse_channel_name(&input.channel);
    let Ok(channel) = parsed_channel else {
        return StimulusRouteDecision {
            allow: false,
            context: "unknown".to_string(),
            channel: input.channel.clone(),
            route: RouteTarget::Fallback,
            reason: Some(parsed_channel.expect_err("checked err")),
            is_control: false,
            command: None,
            session_key: resolve_session_key(input, None),
        };
    };

    let context = policy.context_for_channel(channel);
    let schedule_verdict = policy.check_schedule(context);
    if let Verdict::Deny { reason } = schedule_verdict {
        return StimulusRouteDecision {
            allow: false,
            context: context_label(context).to_string(),
            channel: normalized_channel_label(channel).to_string(),
            route: RouteTarget::Fallback,
            reason: Some(reason),
            is_control: false,
            command: None,
            session_key: resolve_session_key(input, Some(channel)),
        };
    }

    let command = extract_control_command(&input.content);
    if let Some(command_name) = command.as_deref() {
        return match policy.check_control_command(context, Some(command_name)) {
            Verdict::Allow => StimulusRouteDecision {
                allow: true,
                context: context_label(context).to_string(),
                channel: normalized_channel_label(channel).to_string(),
                route: RouteTarget::Control,
                reason: None,
                is_control: true,
                command,
                session_key: resolve_session_key(input, Some(channel)),
            },
            Verdict::Deny { reason } => StimulusRouteDecision {
                allow: false,
                context: context_label(context).to_string(),
                channel: normalized_channel_label(channel).to_string(),
                route: RouteTarget::Control,
                reason: Some(reason),
                is_control: true,
                command,
                session_key: resolve_session_key(input, Some(channel)),
            },
        };
    }

    let preference = input.route_preference.unwrap_or(RoutePreference::Fallback);
    let route = match preference {
        RoutePreference::Driver => RouteTarget::Driver,
        RoutePreference::Off | RoutePreference::Gate | RoutePreference::Fallback => {
            RouteTarget::Fallback
        }
    };

    StimulusRouteDecision {
        allow: true,
        context: context_label(context).to_string(),
        channel: normalized_channel_label(channel).to_string(),
        route,
        reason: None,
        is_control: false,
        command: None,
        session_key: resolve_session_key(input, Some(channel)),
    }
}

fn normalized_channel_label(channel: Channel) -> &'static str {
    match channel {
        Channel::TelegramPublic => "telegram_public",
        Channel::TelegramFamily => "telegram_family",
        Channel::TelegramDm => "telegram_dm",
        Channel::Terminal => "terminal",
        Channel::Internal => "internal",
        Channel::Api => "api",
        Channel::Moltbook => "moltbook",
    }
}

fn context_label(context: ContextTier) -> &'static str {
    match context {
        ContextTier::Public => "public",
        ContextTier::Family => "family",
        ContextTier::Private => "private",
    }
}

impl StimulusInput {
    /// Convert to a Stimulus, parsing the channel name and defaulting timestamp to now.
    pub fn to_stimulus(&self) -> Result<crate::types::Stimulus, String> {
        let channel = parse_channel_name(&self.channel)?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        Ok(crate::types::Stimulus {
            channel,
            actor: self.actor.clone(),
            content: self.content.clone(),
            timestamp: self.timestamp.unwrap_or(now),
        })
    }
}

fn parse_channel_name(raw: &str) -> Result<Channel, String> {
    let key = raw.trim().to_ascii_lowercase().replace('-', "_");
    let channel = match key.as_str() {
        "telegram_public" => Channel::TelegramPublic,
        "telegram_family" => Channel::TelegramFamily,
        "telegram_dm" => Channel::TelegramDm,
        "terminal" => Channel::Terminal,
        "internal" | "heartbeat" | "exec_event" | "cron_event" | "cron" => Channel::Internal,
        "api" | "web" | "webchat" | "discord" | "slack" | "signal" | "whatsapp" | "matrix" => {
            Channel::Api
        }
        "moltbook" => Channel::Moltbook,
        other => {
            return Err(format!("unknown stimulus channel: '{other}'"));
        }
    };
    Ok(channel)
}

fn extract_control_command(content: &str) -> Option<String> {
    let trimmed = content.trim_start();
    if !trimmed.starts_with('/') {
        return None;
    }

    let token = trimmed.split_whitespace().next()?;
    let bare = token.trim_start_matches('/');
    let command = bare.split('@').next().unwrap_or("").trim();
    if command.is_empty() {
        return None;
    }

    Some(command.to_ascii_lowercase())
}

fn resolve_session_key(input: &StimulusInput, channel: Option<Channel>) -> String {
    if let Some(key) = input.session_key.as_deref() {
        let trimmed = key.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }

    if let Some(channel) = channel {
        return format!(
            "{}:{}",
            normalized_channel_label(channel),
            input.actor.trim().to_ascii_lowercase()
        );
    }

    format!("{}:{}", input.channel.trim(), input.actor.trim())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    use crate::config::TimeWindow;
    use crate::policy::{ContextRoots, GatePolicy};

    use super::{RoutePreference, RouteTarget, StimulusInput, decide_stimulus, route_stimulus};

    fn make_policy() -> GatePolicy {
        let id = format!(
            "vericore-impetus-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos(),
        );
        let base = std::env::temp_dir().join(id);
        fs::create_dir_all(base.join("private")).expect("private dir");
        fs::create_dir_all(base.join("family")).expect("family dir");

        let roots = ContextRoots::new(&base, base.join("private"), base.join("family"))
            .expect("create roots");
        GatePolicy::new(roots, vec![])
    }

    #[test]
    fn allows_known_channel_when_schedule_allows() {
        let policy = make_policy();
        let decision = decide_stimulus(
            &policy,
            &StimulusInput {
                channel: "telegram_dm".to_string(),
                actor: "user".to_string(),
                content: "hi".to_string(),
                timestamp: None,
                session_key: None,
                route_preference: None,
            },
        );
        assert!(decision.allow);
        assert_eq!(decision.context, "private");
        assert_eq!(decision.channel, "telegram_dm");
    }

    #[test]
    fn denies_unknown_channel() {
        let policy = make_policy();
        let decision = decide_stimulus(
            &policy,
            &StimulusInput {
                channel: "totally_new_channel".to_string(),
                actor: "user".to_string(),
                content: "hi".to_string(),
                timestamp: None,
                session_key: None,
                route_preference: None,
            },
        );
        assert!(!decision.allow);
        assert_eq!(decision.context, "unknown");
        assert!(
            decision
                .reason
                .as_deref()
                .unwrap_or_default()
                .contains("unknown stimulus channel")
        );
    }

    #[test]
    fn denies_when_schedule_blocks_context() {
        let id = format!(
            "vericore-impetus-test-schedule-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos(),
        );
        let base = std::env::temp_dir().join(id);
        fs::create_dir_all(base.join("private")).expect("private dir");
        fs::create_dir_all(base.join("family")).expect("family dir");

        let roots = ContextRoots::new(&base, base.join("private"), base.join("family"))
            .expect("create roots");
        let policy = GatePolicy::new(roots, vec![]).with_schedule(
            0,
            Some(TimeWindow {
                allowed_start_hour: 0,
                allowed_end_hour: 0,
            }),
            None,
            None,
        );

        let decision = decide_stimulus(
            &policy,
            &StimulusInput {
                channel: "telegram_public".to_string(),
                actor: "visitor".to_string(),
                content: "hello".to_string(),
                timestamp: None,
                session_key: None,
                route_preference: None,
            },
        );
        assert!(!decision.allow);
        assert_eq!(decision.context, "public");
    }

    #[test]
    fn route_prefers_control_for_commands() {
        let policy = make_policy();
        let route = route_stimulus(
            &policy,
            &StimulusInput {
                channel: "telegram_dm".to_string(),
                actor: "zar".to_string(),
                content: "/model list".to_string(),
                timestamp: None,
                session_key: Some("chat:1".to_string()),
                route_preference: Some(RoutePreference::Driver),
            },
        );
        assert!(route.allow);
        assert_eq!(route.route, RouteTarget::Control);
        assert_eq!(route.command.as_deref(), Some("model"));
        assert_eq!(route.session_key, "chat:1");
    }

    #[test]
    fn route_prefers_driver_for_non_control_when_requested() {
        let policy = make_policy();
        let route = route_stimulus(
            &policy,
            &StimulusInput {
                channel: "telegram_dm".to_string(),
                actor: "zar".to_string(),
                content: "hello".to_string(),
                timestamp: None,
                session_key: None,
                route_preference: Some(RoutePreference::Driver),
            },
        );
        assert!(route.allow);
        assert_eq!(route.route, RouteTarget::Driver);
    }

    #[test]
    fn route_falls_back_when_driver_not_requested() {
        let policy = make_policy();
        let route = route_stimulus(
            &policy,
            &StimulusInput {
                channel: "telegram_dm".to_string(),
                actor: "zar".to_string(),
                content: "hello".to_string(),
                timestamp: None,
                session_key: None,
                route_preference: Some(RoutePreference::Gate),
            },
        );
        assert!(route.allow);
        assert_eq!(route.route, RouteTarget::Fallback);
    }

    #[test]
    fn route_denies_disallowed_control_command_for_context() {
        let id = format!(
            "vericore-impetus-test-control-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos(),
        );
        let base = std::env::temp_dir().join(id);
        fs::create_dir_all(base.join("private")).expect("private dir");
        fs::create_dir_all(base.join("family")).expect("family dir");

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
family = ["*"]
private = ["*"]

[schedule]
utc_offset = 0
"#,
            base.display(),
            base.join("private").display(),
            base.join("family").display(),
        );
        let config: crate::config::Config = toml::from_str(&cfg).expect("parse config");
        let policy = GatePolicy::from_config(&config).expect("build policy");

        let route = route_stimulus(
            &policy,
            &StimulusInput {
                channel: "telegram_public".to_string(),
                actor: "visitor".to_string(),
                content: "/restart".to_string(),
                timestamp: None,
                session_key: None,
                route_preference: Some(RoutePreference::Driver),
            },
        );

        assert!(!route.allow);
        assert!(route.is_control);
        assert_eq!(route.route, RouteTarget::Control);
        assert!(
            route
                .reason
                .as_deref()
                .unwrap_or_default()
                .contains("not allowed")
        );
    }
}
