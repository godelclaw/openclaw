use std::collections::{HashMap, HashSet};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use adclaw_memory::{MemoryCortex, MemoryTier, TurnEvent};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::Mutex;
use tokio::time;

use crate::config::Config;
use crate::impetus::{StimulusDecision, StimulusInput, decide_stimulus, route_stimulus};
use crate::llm::ChatMessage;
use crate::policy::GatePolicy;
use crate::turn::run_turn_with_history;
use crate::types::ContextTier;

const MAX_REQUEST_BYTES: usize = 1_048_576; // 1 MiB
const IO_TIMEOUT: Duration = Duration::from_secs(15);
const MEMORY_QUERY_LIMIT: usize = 6;
const MEMORY_HOT_LIMIT: usize = 4;
const MEMORY_HOT_MIN_REINFORCEMENT: u32 = 2;
const MEMORY_MAX_SUMMARY_CHARS: usize = 220;

#[derive(Debug, Deserialize)]
pub struct DaemonRequest {
    pub method: String,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub stimulus: Option<StimulusInput>,
}

#[derive(Debug, Serialize)]
pub struct DaemonResponse {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl DaemonResponse {
    fn ok(id: Option<String>, result: Value) -> Self {
        Self {
            ok: true,
            id,
            result: Some(result),
            error: None,
        }
    }

    fn err(id: Option<String>, error: impl Into<String>) -> Self {
        Self {
            ok: false,
            id,
            result: None,
            error: Some(error.into()),
        }
    }
}

struct SharedState {
    config: Config,
    policy: GatePolicy,
    system_prompt: String,
    sessions: Mutex<HashMap<String, Vec<ChatMessage>>>,
    memory_path: PathBuf,
    memory: Mutex<MemoryCortex>,
}

pub fn serve(config_path: &Path, socket_path: &Path) -> Result<(), String> {
    let config = Config::load(config_path)?;
    let policy = GatePolicy::from_config(&config)?;
    let system_prompt = config.load_system_prompt()?;

    let memory_path = resolve_memory_path(config_path);
    if let Some(parent) = memory_path.parent() {
        fs::create_dir_all(parent).map_err(|e| {
            format!(
                "failed to create memory directory {}: {e}",
                parent.display()
            )
        })?;
    }
    let memory = load_memory_cortex(&memory_path);

    if let Some(parent) = socket_path.parent() {
        fs::create_dir_all(parent).map_err(|e| {
            format!(
                "failed to create socket directory {}: {e}",
                parent.display()
            )
        })?;
    }

    if socket_path.exists() {
        fs::remove_file(socket_path).map_err(|e| {
            format!(
                "failed to remove stale socket {}: {e}",
                socket_path.display()
            )
        })?;
    }

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("failed to create tokio runtime: {e}"))?;

    runtime.block_on(async move {
        let listener = UnixListener::bind(socket_path)
            .map_err(|e| format!("failed to bind socket {}: {e}", socket_path.display()))?;
        fs::set_permissions(socket_path, fs::Permissions::from_mode(0o600))
            .map_err(|e| format!("failed to set socket permissions: {e}"))?;

        eprintln!(
            "[vericore] listening on unix socket {}",
            socket_path.display()
        );
        eprintln!("[vericore] memory cortex file {}", memory_path.display());

        let shared = Arc::new(SharedState {
            config,
            policy,
            system_prompt,
            sessions: Mutex::new(HashMap::new()),
            memory_path,
            memory: Mutex::new(memory),
        });

        loop {
            let (stream, _addr) = match listener.accept().await {
                Ok(pair) => pair,
                Err(err) => {
                    eprintln!("[vericore] socket accept error: {err}");
                    continue;
                }
            };

            let shared = Arc::clone(&shared);
            tokio::spawn(async move {
                if let Err(err) = handle_client(stream, shared).await {
                    eprintln!("[vericore] request error: {err}");
                }
            });
        }
    })
}

async fn handle_client(mut stream: UnixStream, shared: Arc<SharedState>) -> Result<(), String> {
    let body = read_request_body(&mut stream).await?;

    let response = if body.trim().is_empty() {
        DaemonResponse::err(None, "empty request body")
    } else {
        let request: DaemonRequest = serde_json::from_str(body.trim())
            .map_err(|e| format!("failed to parse request json: {e}; body={}", body.trim()))?;
        dispatch_request(request, &shared).await
    };

    write_response(&mut stream, &response).await
}

async fn dispatch_request(request: DaemonRequest, shared: &SharedState) -> DaemonResponse {
    match request.method.as_str() {
        "health" => DaemonResponse::ok(request.id, json!({"status":"ok"})),
        "decide" => match request.stimulus {
            Some(input) => {
                let decision = decide_stimulus(&shared.policy, &input);
                match serde_json::to_value(decision) {
                    Ok(value) => DaemonResponse::ok(request.id, value),
                    Err(err) => {
                        DaemonResponse::err(request.id, format!("failed to encode decision: {err}"))
                    }
                }
            }
            None => DaemonResponse::err(request.id, "missing stimulus for method=decide"),
        },
        "route" => match request.stimulus {
            Some(input) => {
                let decision = route_stimulus(&shared.policy, &input);
                match serde_json::to_value(decision) {
                    Ok(value) => DaemonResponse::ok(request.id, value),
                    Err(err) => {
                        DaemonResponse::err(request.id, format!("failed to encode route: {err}"))
                    }
                }
            }
            None => DaemonResponse::err(request.id, "missing stimulus for method=route"),
        },
        "run" => match request.stimulus {
            Some(input) => {
                let route = route_stimulus(&shared.policy, &input);
                let decision = StimulusDecision {
                    allow: route.allow,
                    context: route.context.clone(),
                    channel: route.channel.clone(),
                    reason: route.reason.clone(),
                };

                if !route.allow {
                    return DaemonResponse::ok(
                        request.id,
                        json!({
                            "decision": decision,
                            "route": route,
                        }),
                    );
                }

                if route.route != crate::impetus::RouteTarget::Driver {
                    return DaemonResponse::ok(
                        request.id,
                        json!({
                            "decision": decision,
                            "route": route,
                            "note": "run skipped because route target is not driver"
                        }),
                    );
                }

                let stimulus = match input.to_stimulus() {
                    Ok(s) => s,
                    Err(err) => {
                        return DaemonResponse::err(
                            request.id,
                            format!("failed to build stimulus: {err}"),
                        );
                    }
                };

                let history_before = {
                    let sessions = shared.sessions.lock().await;
                    sessions
                        .get(&route.session_key)
                        .cloned()
                        .unwrap_or_else(Vec::new)
                };

                let context = shared.policy.context_for_channel(stimulus.channel);
                let memory_tier = memory_tier_for_context(context);
                let (memory_block, memory_context_items, memory_total_before) = {
                    let memory = shared.memory.lock().await;
                    let block = build_memory_context_block(&memory, &stimulus.content, memory_tier);
                    let total = memory.stats().total_items;
                    (block.text, block.item_count, total)
                };

                let system_prompt_for_turn = if memory_block.is_empty() {
                    shared.system_prompt.clone()
                } else {
                    format!("{}\n\n{}", shared.system_prompt, memory_block)
                };

                match run_turn_with_history(
                    &shared.config,
                    &shared.policy,
                    &stimulus,
                    &system_prompt_for_turn,
                    &history_before,
                )
                .await
                {
                    Ok((outcome, updated_history)) => {
                        let trimmed_history =
                            trim_history(updated_history, shared.config.turn.history_messages_max);
                        {
                            let mut sessions = shared.sessions.lock().await;
                            sessions.insert(route.session_key.clone(), trimmed_history.clone());
                        }

                        let response_text = outcome.response.clone();
                        let (created_memory_items, memory_total_after, memory_save_error) = {
                            let mut memory = shared.memory.lock().await;
                            let created = memory.ingest_turn(TurnEvent {
                                session_key: route.session_key.clone(),
                                timestamp: stimulus.timestamp as i64,
                                user_text: input.content.clone(),
                                assistant_text: response_text,
                                tier: memory_tier,
                            });
                            let save_error = memory.save_json(&shared.memory_path).err();
                            let total = memory.stats().total_items;
                            (created.len(), total, save_error)
                        };

                        match serde_json::to_value(outcome) {
                            Ok(value) => {
                                let mut result = json!({
                                    "decision": decision,
                                    "route": route,
                                    "outcome": value,
                                    "history_messages": trimmed_history.len(),
                                    "memory": {
                                        "file": shared.memory_path.display().to_string(),
                                        "context_items": memory_context_items,
                                        "total_items_before": memory_total_before,
                                        "created_items": created_memory_items,
                                        "total_items_after": memory_total_after,
                                    }
                                });

                                if let Some(err) = memory_save_error {
                                    result["memory"]["save_error"] = json!(err);
                                }

                                DaemonResponse::ok(request.id, result)
                            }
                            Err(err) => DaemonResponse::err(
                                request.id,
                                format!("failed to encode outcome: {err}"),
                            ),
                        }
                    }
                    Err(err) => DaemonResponse::err(request.id, format!("run failed: {err}")),
                }
            }
            None => DaemonResponse::err(request.id, "missing stimulus for method=run"),
        },
        other => DaemonResponse::err(request.id, format!("unknown method: {other}")),
    }
}

fn trim_history(mut history: Vec<ChatMessage>, limit: usize) -> Vec<ChatMessage> {
    if limit == 0 {
        return Vec::new();
    }
    if history.len() > limit {
        let drop_count = history.len().saturating_sub(limit);
        history.drain(0..drop_count);
    }
    history
}

async fn read_request_body(stream: &mut UnixStream) -> Result<String, String> {
    let mut body = Vec::new();
    let mut buf = [0_u8; 8192];

    loop {
        let read_res = time::timeout(IO_TIMEOUT, stream.read(&mut buf))
            .await
            .map_err(|_| format!("read timed out after {}s", IO_TIMEOUT.as_secs()))?;
        let n = read_res.map_err(|e| format!("failed to read request: {e}"))?;

        if n == 0 {
            break;
        }

        body.extend_from_slice(&buf[..n]);
        if body.len() > MAX_REQUEST_BYTES {
            return Err(format!("request too large (>{MAX_REQUEST_BYTES} bytes)"));
        }
    }

    String::from_utf8(body).map_err(|e| format!("request is not valid UTF-8: {e}"))
}

async fn write_response(stream: &mut UnixStream, response: &DaemonResponse) -> Result<(), String> {
    let encoded = serde_json::to_vec(response).map_err(|e| format!("encode response: {e}"))?;

    let write_result = time::timeout(IO_TIMEOUT, stream.write_all(&encoded))
        .await
        .map_err(|_| format!("write timed out after {}s", IO_TIMEOUT.as_secs()))?;
    if let Err(err) = write_result {
        if is_peer_disconnect(&err) {
            return Ok(());
        }
        return Err(format!("write response: {err}"));
    }

    let shutdown_result = time::timeout(IO_TIMEOUT, stream.shutdown())
        .await
        .map_err(|_| format!("shutdown timed out after {}s", IO_TIMEOUT.as_secs()))?;
    if let Err(err) = shutdown_result {
        if is_peer_disconnect(&err) {
            return Ok(());
        }
        return Err(format!("shutdown stream: {err}"));
    }

    Ok(())
}

fn is_peer_disconnect(err: &std::io::Error) -> bool {
    matches!(
        err.kind(),
        std::io::ErrorKind::BrokenPipe | std::io::ErrorKind::ConnectionReset
    )
}

fn resolve_memory_path(config_path: &Path) -> PathBuf {
    if let Ok(raw) = std::env::var("VERICORE_MEMORY_FILE") {
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }

    config_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("vericore-memory.json")
}

fn load_memory_cortex(path: &Path) -> MemoryCortex {
    if !path.exists() {
        return MemoryCortex::new();
    }

    match MemoryCortex::load_json(path) {
        Ok(cortex) => {
            eprintln!("[vericore] loaded memory cortex from {}", path.display());
            cortex
        }
        Err(err) => {
            eprintln!(
                "[vericore] failed to load memory cortex from {}: {} (starting empty)",
                path.display(),
                err
            );
            MemoryCortex::new()
        }
    }
}

fn memory_tier_for_context(context: ContextTier) -> MemoryTier {
    match context {
        ContextTier::Public => MemoryTier::Public,
        ContextTier::Family => MemoryTier::Family,
        ContextTier::Private => MemoryTier::Private,
    }
}

struct MemoryContextBlock {
    text: String,
    item_count: usize,
}

fn build_memory_context_block(
    memory: &MemoryCortex,
    query: &str,
    tier: MemoryTier,
) -> MemoryContextBlock {
    let mut lines: Vec<String> = Vec::new();
    let mut seen: HashSet<u64> = HashSet::new();

    for hit in memory.search(query, tier, MEMORY_QUERY_LIMIT) {
        if seen.insert(hit.item.id) {
            lines.push(format_memory_line(&hit.item.summary, &hit.item.categories));
        }
    }

    for item in memory.hot_items(tier, MEMORY_HOT_MIN_REINFORCEMENT, MEMORY_HOT_LIMIT) {
        if seen.insert(item.id) {
            lines.push(format_memory_line(&item.summary, &item.categories));
        }
    }

    if lines.is_empty() {
        return MemoryContextBlock {
            text: String::new(),
            item_count: 0,
        };
    }

    let text = format!(
        "Recalled memory (tier-scoped):\n{}\nUse this memory only when relevant to the current request.",
        lines
            .iter()
            .map(|line| format!("- {line}"))
            .collect::<Vec<_>>()
            .join("\n")
    );

    MemoryContextBlock {
        text,
        item_count: lines.len(),
    }
}

fn format_memory_line(summary: &str, categories: &[String]) -> String {
    let mut line = truncate_chars(summary.trim(), MEMORY_MAX_SUMMARY_CHARS);
    if !categories.is_empty() {
        let category_text = categories
            .iter()
            .take(3)
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join(", ");
        line.push_str(" [");
        line.push_str(&category_text);
        line.push(']');
    }
    line
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    let mut out = String::new();
    for ch in text.chars().take(max_chars) {
        out.push(ch);
    }
    if text.chars().count() > max_chars {
        out.push_str("...");
    }
    out
}
