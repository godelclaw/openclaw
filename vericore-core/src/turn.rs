use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use adclaw_memory::MemoryCortex;
use serde::Serialize;
use tokio::sync::Mutex;

use crate::config::Config;
use crate::core::CoreLoop;
use crate::executor::Executor;
use crate::llm::{ChatMessage, LlmClient, LlmTurnResult, LlmUsage};
use crate::policy::GatePolicy;
use crate::security_review::SecurityReviewer;
use crate::utils::truncate_chars;
use crate::tools::{parse_tool_call, tool_definitions_for_context};
use crate::types::{
    Action, ContextTier, Effect, IntegrityTier, SecurityHold, SecurityReviewDecision,
    SecurityVerdict, Stimulus,
};

const FINALIZE_FALLBACK_TEXT: &str = "I hit a tool-loop limit and could not safely produce a final response. Please try a more specific request.";

#[derive(Debug, Serialize)]
pub struct TurnOutcome {
    pub response: String,
    pub tools_used: Vec<String>,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub security_hold: Option<SecurityHold>,
}

#[derive(Debug)]
enum BoundaryReviewOutcome {
    Execute,
    Message(String),
}

/// Run a full ReAct turn without prior chat state.
pub async fn run_turn(
    config: &Config,
    policy: &GatePolicy,
    stimulus: &Stimulus,
    system_prompt: &str,
) -> Result<TurnOutcome, String> {
    let (outcome, _history) = run_turn_with_history_with_reviewer_memory(
        config,
        policy,
        stimulus,
        system_prompt,
        &[],
        None,
        None,
    )
    .await?;
    Ok(outcome)
}

/// Run a full ReAct turn with prior chat history and return updated history.
///
/// `history` should not contain the system prompt. It is prepended internally.
pub async fn run_turn_with_history(
    config: &Config,
    policy: &GatePolicy,
    stimulus: &Stimulus,
    system_prompt: &str,
    history: &[ChatMessage],
) -> Result<(TurnOutcome, Vec<ChatMessage>), String> {
    run_turn_with_history_with_reviewer_memory(
        config,
        policy,
        stimulus,
        system_prompt,
        history,
        None,
        None,
    )
    .await
}

pub async fn run_turn_with_history_with_reviewer_memory(
    config: &Config,
    policy: &GatePolicy,
    stimulus: &Stimulus,
    system_prompt: &str,
    history: &[ChatMessage],
    reviewer_memory: Option<Arc<Mutex<MemoryCortex>>>,
    reviewer_memory_path: Option<PathBuf>,
) -> Result<(TurnOutcome, Vec<ChatMessage>), String> {
    let llm = LlmClient::from_config(&config.llm).map_err(|e| e.to_string())?;
    let security_reviewer = SecurityReviewer::from_config(config, reviewer_memory, reviewer_memory_path)?;
    let executor = Executor::new(&config.turn, config.security_review.mindlock_dir.clone());
    let context = policy.context_for_channel(stimulus.channel);
    let tool_defs = tool_definitions_for_context(policy, context, &config.security_review.mindlock_dir);

    let mut messages = Vec::with_capacity(history.len() + 2);
    messages.push(ChatMessage::system(system_prompt));
    messages.extend(history.iter().cloned());

    let user_message = ChatMessage::user(&stimulus.content);
    messages.push(user_message.clone());

    let mut updated_history = history.to_vec();
    updated_history.push(user_message);

    let mut tools_used = Vec::new();
    let mut total_usage = LlmUsage::default();
    let mut core = CoreLoop::new(policy.clone(), config.turn.gas_budget);

    let mut attempted_tool_calls: u32 = 0;
    let mut iterations: u32 = 0;
    let mut security_hold: Option<SecurityHold> = None;
    let mut boundary_revision_count: u32 = 0;

    loop {
        if config.turn.max_iterations != 0 && iterations >= config.turn.max_iterations {
            break;
        }
        iterations = iterations.saturating_add(1);

        let (result, usage) = llm
            .chat(&messages, &tool_defs)
            .await
            .map_err(|e| e.to_string())?;
        total_usage.prompt_tokens += usage.prompt_tokens;
        total_usage.completion_tokens += usage.completion_tokens;

        match result {
            LlmTurnResult::FinalResponse { content } => {
                updated_history.push(ChatMessage::assistant(&content));
                return Ok((
                    make_outcome(content, tools_used, &total_usage, security_hold),
                    updated_history,
                ));
            }
            LlmTurnResult::ToolCalls {
                content,
                tool_calls,
            } => {
                let assistant_message =
                    ChatMessage::assistant_with_tools(content.as_deref(), &tool_calls);
                messages.push(assistant_message.clone());
                updated_history.push(assistant_message);

                let actions: Vec<_> = tool_calls.iter().map(parse_tool_call).collect();

                // Pass 1: gate all actions through policy (synchronous)
                let effects = core.tick(
                    stimulus.clone(),
                    |_| actions.clone(),
                    |_| Effect::Executed {
                        kind: "allowed".into(),
                    },
                );

                // Pass 2: execute allowed actions (async)
                for ((call, action), effect) in
                    tool_calls.iter().zip(actions.iter()).zip(effects.iter())
                {
                    attempted_tool_calls = attempted_tool_calls.saturating_add(1);

                    let result_content = match effect {
                        Effect::Executed { .. } => {
                            let primitive = action.executable_action();
                            match (security_reviewer.as_ref(), primitive) {
                                (
                                    Some(reviewer),
                                    Action::PromoteFromMindlock {
                                        source_path,
                                        target_path,
                                    },
                                ) => {
                                    match review_boundary_crossing(
                                        policy,
                                        reviewer,
                                        source_path,
                                        target_path,
                                        &mut boundary_revision_count,
                                        &mut security_hold,
                                        true,
                                    )
                                    .await?
                                    {
                                        BoundaryReviewOutcome::Execute => {
                                            tools_used.push(call.name.clone());
                                            executor.execute_tool(call).await.content
                                        }
                                        BoundaryReviewOutcome::Message(message) => message,
                                    }
                                }
                                (
                                    Some(reviewer),
                                    Action::RequestReview {
                                        source_path,
                                        target_path,
                                    },
                                ) => {
                                    match review_boundary_crossing(
                                        policy,
                                        reviewer,
                                        source_path,
                                        target_path,
                                        &mut boundary_revision_count,
                                        &mut security_hold,
                                        false,
                                    )
                                    .await?
                                    {
                                        BoundaryReviewOutcome::Execute => {
                                            "SECURITY REVIEW: allow (receipt recorded)".to_string()
                                        }
                                        BoundaryReviewOutcome::Message(message) => message,
                                    }
                                }
                                (
                                    Some(reviewer),
                                    Action::SelfEscalate {
                                        source_path,
                                        reason,
                                    },
                                ) => self_escalate_artifact(
                                    reviewer,
                                    source_path,
                                    reason,
                                    &mut security_hold,
                                ).await,
                                (Some(reviewer), Action::Exec { command }) => {
                                    match maybe_guard_exec_artifact(reviewer, command) {
                                        Ok(()) => {
                                            tools_used.push(call.name.clone());
                                            executor.execute_tool(call).await.content
                                        }
                                        Err(message) => message,
                                    }
                                }
                                _ => {
                                    tools_used.push(call.name.clone());
                                    executor.execute_tool(call).await.content
                                }
                            }
                        }
                        Effect::Denied { reason } => format!("DENIED: {reason}"),
                    };

                    let tool_message =
                        ChatMessage::tool_result(&call.id, &call.name, &result_content);
                    messages.push(tool_message.clone());
                    updated_history.push(tool_message);
                }

                if reached_tool_call_limit(attempted_tool_calls, config.turn.max_tool_calls) {
                    let forced = if config.turn.finalize_without_tools_on_limit {
                        force_finalize_without_tools(
                            &llm,
                            &mut messages,
                            &mut updated_history,
                            &mut total_usage,
                            &format!("tool call limit reached ({})", config.turn.max_tool_calls),
                        )
                        .await?
                    } else {
                        format!("Tool call limit reached ({}).", config.turn.max_tool_calls)
                    };

                    updated_history.push(ChatMessage::assistant(&forced));
                    return Ok((
                        make_outcome(forced, tools_used, &total_usage, security_hold),
                        updated_history,
                    ));
                }
            }
        }
    }

    if config.turn.finalize_without_tools_on_limit {
        let forced = force_finalize_without_tools(
            &llm,
            &mut messages,
            &mut updated_history,
            &mut total_usage,
            &format!("max iterations reached ({})", config.turn.max_iterations),
        )
        .await?;

        updated_history.push(ChatMessage::assistant(&forced));
        return Ok((
            make_outcome(forced, tools_used, &total_usage, security_hold),
            updated_history,
        ));
    }

    let fallback = "Max iterations reached without final response.".to_string();
    updated_history.push(ChatMessage::assistant(&fallback));
    Ok((
        make_outcome(fallback, tools_used, &total_usage, security_hold),
        updated_history,
    ))
}

async fn review_boundary_crossing(
    policy: &GatePolicy,
    reviewer: &SecurityReviewer,
    source_path: &Path,
    target_path: &Path,
    revision_count: &mut u32,
    security_hold: &mut Option<SecurityHold>,
    execute_on_allow: bool,
) -> Result<BoundaryReviewOutcome, String> {
    let Some(stage) = mindlock_stage(source_path, reviewer.mindlock_dir()) else {
        return Ok(BoundaryReviewOutcome::Message(
            format!(
                "SECURITY REVIEW: source_path must be under {0}/in, {0}/out, or {0}/work",
                reviewer.mindlock_dir().display()
            ),
        ));
    };

    let promote_action = Action::PromoteFromMindlock {
        source_path: source_path.to_path_buf(),
        target_path: target_path.to_path_buf(),
    };

    let preview = artifact_preview(source_path, 2000);

    let mut decision = if stage == "in" || stage == "work" {
        reviewer
            .review_ingress(
                IntegrityTier::Untrusted,
                &promote_action,
                &format!(
                    "mindlock ingress crossing: {} -> {}",
                    source_path.display(),
                    target_path.display()
                ),
            )
            .await
            .unwrap_or_else(|err| SecurityReviewDecision {
                verdict: SecurityVerdict::RequireZarApproval,
                reason: format!("security review failed: {err}"),
                feedback: None,
                safe_rewrite: None,
            })
    } else {
        let sink_context = target_context_for_path(policy, target_path);
        let provenance = vec![
            format!("source_stage={stage}"),
            format!("source={}", source_path.display()),
            format!("target={}", target_path.display()),
        ];
        reviewer
            .review_egress(sink_context, ContextTier::Private, &preview, &provenance)
            .await
            .unwrap_or_else(|err| SecurityReviewDecision {
                verdict: SecurityVerdict::RequireZarApproval,
                reason: format!("security review failed: {err}"),
                feedback: None,
                safe_rewrite: None,
            })
    };

    if matches!(decision.verdict, SecurityVerdict::RequestRevision)
        && *revision_count >= reviewer.max_revision_attempts()
    {
        decision.verdict = SecurityVerdict::RequireZarApproval;
        decision.reason = format!(
            "revision limit reached ({}): {}",
            reviewer.max_revision_attempts(),
            decision.reason
        );
    }

    match decision.verdict {
        SecurityVerdict::Allow => {
            let _approved_hash = reviewer.record_artifact_review_receipt(
                source_path,
                stage,
                target_path,
                &decision.reason,
            )?;
            reviewer
                .record_precedent(
                    &format!(
                        "Mindlock {} crossing {} -> {} — allowed. Reason: {}",
                        stage,
                        source_path.display(),
                        target_path.display(),
                        decision.reason
                    ),
                    "allow",
                    "reviewer",
                )
                .await;
            if execute_on_allow {
                Ok(BoundaryReviewOutcome::Execute)
            } else {
                Ok(BoundaryReviewOutcome::Message(
                    "SECURITY REVIEW: allow (receipt recorded)".to_string(),
                ))
            }
        }
        SecurityVerdict::RequestRevision => {
            *revision_count = revision_count.saturating_add(1);
            let feedback = decision.feedback.unwrap_or(decision.reason);
            Ok(BoundaryReviewOutcome::Message(format!(
                "SECURITY REVIEW: request_revision ({feedback})"
            )))
        }
        SecurityVerdict::RequireZarApproval => {
            let pending = reviewer.hold_artifact_for_zar(source_path, &decision.reason)?;
            merge_hold(
                security_hold,
                Some(SecurityHold {
                    gate: format!("boundary:{stage}"),
                    reason: decision.reason.clone(),
                    held_content: format!(
                        "promote_from_mindlock {} -> {}",
                        source_path.display(),
                        target_path.display()
                    ),
                    mindlock_path: Some(pending.display().to_string()),
                }),
            );
            Ok(BoundaryReviewOutcome::Message(format!(
                "SECURITY HOLD: waiting for Zar approval ({}) [mindlock: {}]",
                decision.reason,
                pending.display()
            )))
        }
        SecurityVerdict::Reject => {
            let rejected = reviewer.reject_artifact(source_path, &decision.reason)?;
            reviewer
                .record_precedent(
                    &format!(
                        "Mindlock {} crossing {} -> {} — rejected. Reason: {}",
                        stage,
                        source_path.display(),
                        target_path.display(),
                        decision.reason
                    ),
                    "reject",
                    "reviewer",
                )
                .await;
            Ok(BoundaryReviewOutcome::Message(format!(
                "SECURITY REVIEW: reject ({}) [rejected: {}]",
                decision.reason,
                rejected.display()
            )))
        }
    }
}

fn mindlock_stage(path: &Path, mindlock_root: &Path) -> Option<&'static str> {
    if path.starts_with(mindlock_root.join("in")) {
        Some("in")
    } else if path.starts_with(mindlock_root.join("out")) {
        Some("out")
    } else if path.starts_with(mindlock_root.join("work")) {
        Some("work")
    } else {
        None
    }
}

fn target_context_for_path(policy: &GatePolicy, target_path: &Path) -> ContextTier {
    let probe = Action::WriteFile {
        path: target_path.to_path_buf(),
        content: String::new(),
    };
    policy.output_label_for_action(ContextTier::Private, &probe)
}

fn artifact_preview(path: &Path, max_chars: usize) -> String {
    match fs::read(path) {
        Ok(bytes) => truncate_chars(&String::from_utf8_lossy(&bytes), max_chars),
        Err(e) => format!("(unable to read artifact {}: {e})", path.display()),
    }
}

async fn self_escalate_artifact(
    reviewer: &SecurityReviewer,
    source_path: &Path,
    reason: &str,
    security_hold: &mut Option<SecurityHold>,
) -> String {
    if !reviewer.is_mindlock_artifact_path(source_path) {
        return format!(
            "SECURITY REVIEW: self_escalate source must be under {0}/in, {0}/out, or {0}/work: {1}",
            reviewer.mindlock_dir().display(),
            source_path.display()
        );
    }

    match reviewer.hold_artifact_for_zar(source_path, reason) {
        Ok(pending) => {
            // Enqueue first; reviewer follows in the background so manual escalation never stalls.
            let reviewer = reviewer.clone();
            let pending_for_review = pending.clone();
            let source_description = source_path.display().to_string();
            let reason_for_review = reason.to_string();

            // Write initial pending reviewer state so /review shows status immediately.
            {
                let meta_path = pending.with_extension("meta.json");
                if let Ok(raw) = tokio::fs::read(&meta_path).await {
                    if let Ok(mut meta) = serde_json::from_slice::<Value>(&raw) {
                        meta["reviewer_assessment"] = json!({
                            "status": "pending",
                            "source": "manual"
                        });
                        if let Ok(updated) = serde_json::to_string_pretty(&meta) {
                            let _ = tokio::fs::write(&meta_path, updated.as_bytes()).await;
                        }
                    }
                }
            }

            tokio::spawn(async move {
                reviewer
                    .auto_review_for_pending(
                        &pending_for_review,
                        &source_description,
                        &reason_for_review,
                    )
                    .await;
            });

            merge_hold(
                security_hold,
                Some(SecurityHold {
                    gate: "self_escalate".to_string(),
                    reason: reason.to_string(),
                    held_content: format!("self_escalate {}", source_path.display()),
                    mindlock_path: Some(pending.display().to_string()),
                }),
            );
            format!(
                "SECURITY HOLD: self-escalated to Zar review ({}) [mindlock: {}]",
                reason,
                pending.display()
            )
        }
        Err(err) => format!("SECURITY REVIEW: self_escalate failed ({err})"),
    }
}

fn maybe_guard_exec_artifact(reviewer: &SecurityReviewer, command: &str) -> Result<(), String> {
    let mindlock_root = reviewer.mindlock_dir();
    if let Some(verdict) = validate_mindlock_housekeeping_command(command, mindlock_root) {
        return verdict;
    }

    let Some(artifact_path) = extract_exec_artifact_path(command) else {
        return Ok(());
    };

    if !reviewer.is_mindlock_artifact_path(&artifact_path) {
        return Ok(());
    }

    reviewer
        .verify_artifact_receipt(&artifact_path, None)
        .map_err(|err| {
            format!("SECURITY REVIEW: artifact exec requires approved receipt first ({err})")
        })
}

fn validate_mindlock_housekeeping_command(command: &str, mindlock_root: &Path) -> Option<Result<(), String>> {
    let trimmed = command.trim();
    if trimmed.is_empty() {
        return None;
    }

    if contains_shell_metacharacters(trimmed) {
        return Some(Err(
            "SECURITY REVIEW: mindlock housekeeping commands must be a single simple command (no pipes/chains/redirections).".to_string(),
        ));
    }

    let tokens: Vec<&str> = trimmed.split_whitespace().collect();
    if tokens.is_empty() {
        return None;
    }

    let command_name = tokens[0];
    let is_housekeeping = matches!(
        command_name,
        "mkdir" | "rmdir" | "rm" | "mv" | "cp" | "touch"
    );
    if !is_housekeeping {
        return None;
    }

    let path_tokens: Vec<&str> = tokens
        .iter()
        .skip(1)
        .copied()
        .filter(|token| !token.starts_with('-'))
        .collect();
    if path_tokens.is_empty() {
        return None;
    }

    let touches_mindlock = path_tokens
        .iter()
        .any(|token| Path::new(token).starts_with(mindlock_root));
    if !touches_mindlock {
        return None;
    }

    for token in path_tokens {
        if !token.starts_with('/') {
            return Some(Err(format!(
                "SECURITY REVIEW: use absolute mindlock paths for housekeeping commands (invalid: {token})"
            )));
        }

        if !is_safe_mindlock_path(Path::new(token), mindlock_root) {
            return Some(Err(format!(
                "SECURITY REVIEW: mindlock housekeeping path must stay under {} (invalid: {token})",
                mindlock_root.display()
            )));
        }
    }

    Some(Ok(()))
}

fn contains_shell_metacharacters(command: &str) -> bool {
    command.contains("&&")
        || command.contains("||")
        || command.contains('|')
        || command.contains(';')
        || command.contains('>')
        || command.contains('<')
        || command.contains('`')
        || command.contains("$(")
}

fn is_safe_mindlock_path(path: &Path, mindlock_root: &Path) -> bool {
    if !path.starts_with(mindlock_root) {
        return false;
    }

    !path
        .components()
        .any(|component| matches!(component, std::path::Component::ParentDir))
}

fn extract_exec_artifact_path(command: &str) -> Option<PathBuf> {
    let tokens: Vec<&str> = command.split_whitespace().collect();
    if tokens.is_empty() {
        return None;
    }

    let interpreters = ["bash", "sh", "python", "python3", "node", "ruby", "perl"];

    if tokens[0].starts_with('/') {
        return Some(PathBuf::from(tokens[0]));
    }

    if interpreters.contains(&tokens[0]) && tokens.len() >= 2 && tokens[1].starts_with('/') {
        return Some(PathBuf::from(tokens[1]));
    }

    None
}

async fn force_finalize_without_tools(
    llm: &LlmClient,
    messages: &mut Vec<ChatMessage>,
    updated_history: &mut Vec<ChatMessage>,
    total_usage: &mut LlmUsage,
    reason: &str,
) -> Result<String, String> {
    let nudge = ChatMessage::user(&format!(
        "Stop using tools. {}. Provide your best final response now, with no tool calls.",
        reason
    ));
    messages.push(nudge.clone());
    updated_history.push(nudge);

    let (result, usage) = llm.chat(messages, &[]).await.map_err(|e| e.to_string())?;
    total_usage.prompt_tokens += usage.prompt_tokens;
    total_usage.completion_tokens += usage.completion_tokens;

    let content = match result {
        LlmTurnResult::FinalResponse { content } => {
            if content.trim().is_empty() {
                FINALIZE_FALLBACK_TEXT.to_string()
            } else {
                content
            }
        }
        LlmTurnResult::ToolCalls { content, .. } => content
            .filter(|c| !c.trim().is_empty())
            .unwrap_or_else(|| FINALIZE_FALLBACK_TEXT.to_string()),
    };

    Ok(content)
}

fn make_outcome(
    response: String,
    tools_used: Vec<String>,
    usage: &LlmUsage,
    security_hold: Option<SecurityHold>,
) -> TurnOutcome {
    TurnOutcome {
        response,
        tools_used,
        prompt_tokens: usage.prompt_tokens,
        completion_tokens: usage.completion_tokens,
        security_hold,
    }
}

fn merge_hold(current: &mut Option<SecurityHold>, incoming: Option<SecurityHold>) {
    if current.is_none() {
        *current = incoming;
    }
}

fn reached_tool_call_limit(attempted_tool_calls: u32, max_tool_calls: u32) -> bool {
    max_tool_calls != 0 && attempted_tool_calls >= max_tool_calls
}



#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{
        contains_shell_metacharacters, extract_exec_artifact_path, reached_tool_call_limit,
        validate_mindlock_housekeeping_command,
    };

    const TEST_MINDLOCK: &str = "/home/zarclaw/mindlock";

    fn test_mindlock_root() -> &'static Path {
        Path::new(TEST_MINDLOCK)
    }

    #[test]
    fn tool_call_limit_counts_attempts() {
        assert!(reached_tool_call_limit(5, 5));
        assert!(reached_tool_call_limit(6, 5));
        assert!(!reached_tool_call_limit(4, 5));
    }

    #[test]
    fn extract_exec_artifact_path_supports_direct_and_interpreter_forms() {
        let direct = extract_exec_artifact_path("/home/zarclaw/mindlock/in/test.sh --flag");
        assert_eq!(
            direct.as_deref(),
            Some(std::path::Path::new("/home/zarclaw/mindlock/in/test.sh"))
        );

        let interpreted = extract_exec_artifact_path("bash /home/zarclaw/mindlock/work/run.py");
        assert_eq!(
            interpreted.as_deref(),
            Some(std::path::Path::new("/home/zarclaw/mindlock/work/run.py"))
        );

        let non_artifact = extract_exec_artifact_path("git status");
        assert!(non_artifact.is_none());
    }

    #[test]
    fn housekeeping_validator_allows_mindlock_only_paths() {
        let verdict =
            validate_mindlock_housekeeping_command("mkdir -p /home/zarclaw/mindlock/work/scratch", test_mindlock_root());
        assert!(matches!(verdict, Some(Ok(()))));
    }

    #[test]
    fn housekeeping_validator_rejects_outside_paths_when_touching_mindlock() {
        let verdict = validate_mindlock_housekeeping_command(
            "rm -rf /home/zarclaw/mindlock/work/scratch /tmp/outside",
            test_mindlock_root(),
        );
        assert!(matches!(verdict, Some(Err(_))));
    }

    #[test]
    fn housekeeping_validator_ignores_non_mindlock_housekeeping_commands() {
        let verdict =
            validate_mindlock_housekeeping_command("rm -rf /home/zarclaw/repos/godelclaw/tmp", test_mindlock_root());
        assert!(verdict.is_none());
    }

    #[test]
    fn shell_metacharacters_are_detected() {
        assert!(contains_shell_metacharacters(
            "mkdir -p /home/zarclaw/mindlock/work/a && rm -rf /tmp"
        ));
        assert!(!contains_shell_metacharacters(
            "mkdir -p /home/zarclaw/mindlock/work/a"
        ));
    }

    #[test]
    fn zero_tool_call_limit_disables_cap() {
        assert!(!reached_tool_call_limit(0, 0));
        assert!(!reached_tool_call_limit(999_999, 0));
    }
}
