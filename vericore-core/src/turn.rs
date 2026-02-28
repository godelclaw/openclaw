use serde::Serialize;

use crate::config::Config;
use crate::core::CoreLoop;
use crate::executor::Executor;
use crate::llm::{ChatMessage, LlmClient, LlmTurnResult, LlmUsage};
use crate::policy::GatePolicy;
use crate::tools::{parse_tool_call, tool_definitions_for_context};
use crate::types::{ContextTier, Effect, Stimulus};

const FINALIZE_FALLBACK_TEXT: &str = "I hit a tool-loop limit and could not safely produce a final response. Please try a more specific request.";

#[derive(Debug, Serialize)]
pub struct TurnOutcome {
    pub response: String,
    pub tools_used: Vec<String>,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
}

/// Run a full ReAct turn without prior chat state.
pub async fn run_turn(
    config: &Config,
    policy: &GatePolicy,
    stimulus: &Stimulus,
    system_prompt: &str,
) -> Result<TurnOutcome, String> {
    let (outcome, _history) =
        run_turn_with_history(config, policy, stimulus, system_prompt, &[]).await?;
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
    let llm = LlmClient::from_config(&config.llm).map_err(|e| e.to_string())?;
    let executor = Executor::new(&config.turn);
    let context = policy.context_for_channel(stimulus.channel);
    let tool_defs = tool_definitions_for_context(policy, context);

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

    // Minimal flow-tracking (Option A): join data labels of executed tools in this turn.
    let mut response_label = ContextTier::Public;
    let mut attempted_tool_calls: u32 = 0;

    for _iteration in 0..config.turn.max_iterations {
        let (result, usage) = llm
            .chat(&messages, &tool_defs)
            .await
            .map_err(|e| e.to_string())?;
        total_usage.prompt_tokens += usage.prompt_tokens;
        total_usage.completion_tokens += usage.completion_tokens;

        match result {
            LlmTurnResult::FinalResponse { content } => {
                let final_content = apply_sink_policy(policy, stimulus, response_label, content);
                updated_history.push(ChatMessage::assistant(&final_content));
                return Ok((
                    make_outcome(final_content, tools_used, &total_usage),
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
                            tools_used.push(call.name.clone());
                            let action_label = policy.output_label_for_action(context, action);
                            response_label = join_label(response_label, action_label);
                            executor.execute_tool(call).await.content
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

                    let final_content = apply_sink_policy(policy, stimulus, response_label, forced);
                    updated_history.push(ChatMessage::assistant(&final_content));
                    return Ok((
                        make_outcome(final_content, tools_used, &total_usage),
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

        let final_content = apply_sink_policy(policy, stimulus, response_label, forced);
        updated_history.push(ChatMessage::assistant(&final_content));
        return Ok((
            make_outcome(final_content, tools_used, &total_usage),
            updated_history,
        ));
    }

    let fallback = "Max iterations reached without final response.".to_string();
    updated_history.push(ChatMessage::assistant(&fallback));
    Ok((
        make_outcome(fallback, tools_used, &total_usage),
        updated_history,
    ))
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

fn make_outcome(response: String, tools_used: Vec<String>, usage: &LlmUsage) -> TurnOutcome {
    TurnOutcome {
        response,
        tools_used,
        prompt_tokens: usage.prompt_tokens,
        completion_tokens: usage.completion_tokens,
    }
}

fn reached_tool_call_limit(attempted_tool_calls: u32, max_tool_calls: u32) -> bool {
    attempted_tool_calls >= max_tool_calls
}

fn apply_sink_policy(
    policy: &GatePolicy,
    stimulus: &Stimulus,
    response_label: ContextTier,
    content: String,
) -> String {
    let sink_context = policy.context_for_channel(stimulus.channel);
    if sink_allows_label(sink_context, response_label) {
        content
    } else {
        sink_denial_message(response_label, sink_context)
    }
}

fn join_label(a: ContextTier, b: ContextTier) -> ContextTier {
    if a.rank() >= b.rank() { a } else { b }
}

fn sink_allows_label(sink_context: ContextTier, response_label: ContextTier) -> bool {
    response_label.can_flow_to(sink_context)
}

fn sink_denial_message(response_label: ContextTier, sink_context: ContextTier) -> String {
    format!(
        "I used {}-labeled data this turn, so I cannot send that to a {} sink without explicit declassification.",
        tier_name(response_label),
        tier_name(sink_context),
    )
}

fn tier_name(tier: ContextTier) -> &'static str {
    match tier {
        ContextTier::Public => "public",
        ContextTier::Family => "family",
        ContextTier::Private => "private",
    }
}

#[cfg(test)]
mod tests {
    use super::{join_label, reached_tool_call_limit, sink_allows_label};
    use crate::types::ContextTier;

    #[test]
    fn join_label_is_max_rank() {
        assert_eq!(
            join_label(ContextTier::Public, ContextTier::Family),
            ContextTier::Family
        );
        assert_eq!(
            join_label(ContextTier::Private, ContextTier::Family),
            ContextTier::Private
        );
    }

    #[test]
    fn private_label_cannot_flow_to_public_sink() {
        assert!(!sink_allows_label(
            ContextTier::Public,
            ContextTier::Private
        ));
    }

    #[test]
    fn public_label_can_flow_to_private_sink() {
        assert!(sink_allows_label(ContextTier::Private, ContextTier::Public));
    }

    #[test]
    fn tool_call_limit_counts_attempts() {
        assert!(reached_tool_call_limit(5, 5));
        assert!(reached_tool_call_limit(6, 5));
        assert!(!reached_tool_call_limit(4, 5));
    }
}
