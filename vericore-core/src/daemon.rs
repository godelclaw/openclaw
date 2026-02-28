use std::collections::{HashMap, HashSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use adclaw_memory::{
    CreateMemoryInput, MemoryCortex, MemoryId, MemoryRefineReport, MemoryTier, MemoryType,
    TurnEvent,
};
use chrono::{DateTime, Datelike, NaiveDate, Utc};
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
const OLLAMA_EMBED_TIMEOUT_SECS: u64 = 2;
const OLLAMA_EMBED_DEFAULT_MODEL: &str = "nomic-embed-text";
const OLLAMA_EMBED_DEFAULT_URL: &str = "http://127.0.0.1:11434/api/embeddings";
const BOOTSTRAP_MAX_LINE_CHARS: usize = 320;
const BOOTSTRAP_MAX_ITEMS_PER_FILE: usize = 200;
const HISTORY_MAX_USER_CHARS: usize = 800;
const HISTORY_MAX_ASSISTANT_CHARS: usize = 1200;
const MEMORY_EMBED_ON_INGEST_DEFAULT: bool = false;
const MEMORY_EMBED_ON_STARTUP_BACKFILL_DEFAULT: bool = false;
const MEMORY_SLEEP_INTERVAL_SECS_DEFAULT: u64 = 0;
const MEMORY_SLEEP_EMBED_DEFAULT: bool = false;

#[derive(Debug, Deserialize)]
pub struct DaemonRequest {
    pub method: String,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub stimulus: Option<StimulusInput>,
    #[serde(default)]
    pub query: Option<String>,
    #[serde(default)]
    pub embed: Option<bool>,
    #[serde(default)]
    pub memory_id: Option<MemoryId>,
    #[serde(default)]
    pub tier: Option<String>,
    #[serde(default)]
    pub operator_approved: Option<bool>,
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
    history_root: PathBuf,
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

    let history_root = resolve_history_root(&config, config_path);
    ensure_history_dirs(&history_root)?;

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
        eprintln!("[vericore] history root {}", history_root.display());

        let shared = Arc::new(SharedState {
            config,
            policy,
            system_prompt,
            sessions: Mutex::new(HashMap::new()),
            memory_path,
            memory: Mutex::new(memory),
            history_root,
        });

        if let Err(err) = bootstrap_memory_from_markdown(&shared).await {
            eprintln!("[vericore] memory bootstrap warning: {err}");
        }
        if should_embed_on_startup_backfill() {
            if let Err(err) = backfill_missing_embeddings(&shared).await {
                eprintln!("[vericore] embedding backfill warning: {err}");
            }
        }

        if let Some(interval) = sleep_refine_interval() {
            let shared_for_sleep = Arc::clone(&shared);
            let embed_in_sleep = sleep_refine_embed();
            eprintln!(
                "[vericore] memory sleep refine enabled: every {}s (embed={})",
                interval.as_secs(),
                embed_in_sleep
            );
            tokio::spawn(async move {
                run_sleep_refine_loop(shared_for_sleep, interval, embed_in_sleep).await;
            });
        }

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
        "memory_status" => {
            let (stats, missing_embeddings, missing_source_dates) = {
                let memory = shared.memory.lock().await;
                (
                    memory.stats(),
                    memory.ids_missing_embeddings().len(),
                    memory.ids_missing_source_date().len(),
                )
            };

            let history_daily = count_markdown_files(&shared.history_root.join("daily"));
            let history_weekly = count_markdown_files(&shared.history_root.join("weekly"));
            let history_daily_merged =
                count_markdown_files(&shared.history_root.join("daily-merged"));

            DaemonResponse::ok(
                request.id,
                json!({
                    "memory": {
                        "file": shared.memory_path.display().to_string(),
                        "total_items": stats.total_items,
                        "by_tier": stats.by_tier,
                        "by_type": stats.by_type,
                        "missing_embeddings": missing_embeddings,
                        "missing_source_dates": missing_source_dates,
                    },
                    "history": {
                        "root": shared.history_root.display().to_string(),
                        "daily_files": history_daily,
                        "weekly_files": history_weekly,
                        "daily_merged_files": history_daily_merged,
                    }
                }),
            )
        }
        "memory_query" => {
            let Some(query) = request.query.filter(|q| !q.trim().is_empty()) else {
                return DaemonResponse::err(request.id, "missing query for method=memory_query");
            };

            let tier = request
                .stimulus
                .as_ref()
                .and_then(|s| s.to_stimulus().ok())
                .map(|s| shared.policy.context_for_channel(s.channel))
                .map(memory_tier_for_context)
                .unwrap_or(MemoryTier::Private);

            let query_embedding = fetch_ollama_embedding(&query).await.ok();

            let hits = {
                let memory = shared.memory.lock().await;
                memory.search_hybrid(&query, query_embedding.as_deref(), tier, 10)
            };

            let payload: Vec<Value> = hits
                .into_iter()
                .map(|hit| {
                    json!({
                        "id": hit.item.id,
                        "score": hit.score,
                        "tier": hit.item.tier,
                        "type": hit.item.memory_type,
                        "summary": hit.item.summary,
                        "categories": hit.item.categories,
                        "source_date": hit.item.source_date,
                        "reinforcement_count": hit.item.reinforcement_count,
                    })
                })
                .collect();

            DaemonResponse::ok(
                request.id,
                json!({"query": query, "tier": tier, "hits": payload}),
            )
        }
        "memory_refine" => {
            let include_embeddings = request.embed.unwrap_or(false);
            match run_memory_refine_once(shared, include_embeddings).await {
                Ok(result) => DaemonResponse::ok(request.id, result),
                Err(err) => DaemonResponse::err(request.id, format!("memory_refine failed: {err}")),
            }
        }
        "memory_set_tier" => {
            let Some(stimulus_input) = request.stimulus.as_ref() else {
                return DaemonResponse::err(
                    request.id,
                    "missing stimulus for method=memory_set_tier",
                );
            };

            let stimulus = match stimulus_input.to_stimulus() {
                Ok(stimulus) => stimulus,
                Err(err) => return DaemonResponse::err(request.id, err),
            };

            let context = shared.policy.context_for_channel(stimulus.channel);
            if context != ContextTier::Private {
                return DaemonResponse::err(
                    request.id,
                    "memory_set_tier is only allowed from private context",
                );
            }

            let Some(id) = request.memory_id else {
                return DaemonResponse::err(
                    request.id,
                    "missing memory_id for method=memory_set_tier",
                );
            };
            let Some(raw_tier) = request.tier.as_ref() else {
                return DaemonResponse::err(request.id, "missing tier for method=memory_set_tier");
            };
            let Some(new_tier) = parse_memory_tier(raw_tier) else {
                return DaemonResponse::err(
                    request.id,
                    format!("unknown memory tier: {}", raw_tier),
                );
            };

            let operator_approved = request.operator_approved.unwrap_or(false);

            let changed = {
                let mut memory = shared.memory.lock().await;
                let changed = match memory.set_tier(id, new_tier, operator_approved) {
                    Ok(changed) => changed,
                    Err(err) => {
                        return DaemonResponse::err(
                            request.id,
                            format!("memory_set_tier failed: {err}"),
                        );
                    }
                };

                if changed {
                    if let Err(err) = memory.save_json(&shared.memory_path) {
                        eprintln!("[vericore] failed to save memory after memory_set_tier: {err}");
                    }
                }

                changed
            };

            DaemonResponse::ok(
                request.id,
                json!({
                    "id": id,
                    "tier": new_tier,
                    "changed": changed,
                    "operator_approved": operator_approved
                }),
            )
        }
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
                let query_embedding = fetch_ollama_embedding(&stimulus.content).await.ok();
                let (memory_block, memory_context_items, memory_total_before) = {
                    let memory = shared.memory.lock().await;
                    let block = build_memory_context_block(
                        &memory,
                        &stimulus.content,
                        query_embedding.as_deref(),
                        memory_tier,
                    );
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
                        let turn_event = TurnEvent {
                            session_key: route.session_key.clone(),
                            timestamp: stimulus.timestamp as i64,
                            user_text: input.content.clone(),
                            assistant_text: response_text,
                            tier: memory_tier,
                        };

                        let (
                            created_ids,
                            created_memory_items,
                            memory_total_after,
                            memory_save_error,
                        ) = {
                            let mut memory = shared.memory.lock().await;
                            let created = memory.ingest_turn(turn_event.clone());
                            if !created.is_empty() {
                                let source_day = ts_to_utc(turn_event.timestamp)
                                    .date_naive()
                                    .format("%Y-%m-%d")
                                    .to_string();
                                for id in &created {
                                    let _ = memory.set_source_date(*id, Some(source_day.clone()));
                                }
                            }
                            let created_count = created.len();
                            let save_error = memory.save_json(&shared.memory_path).err();
                            let total = memory.stats().total_items;
                            (created, created_count, total, save_error)
                        };

                        let mut embedded_created_items = 0usize;
                        if should_embed_on_ingest() && !created_ids.is_empty() {
                            match embed_and_attach_items(shared, &created_ids).await {
                                Ok(count) => embedded_created_items = count,
                                Err(err) => {
                                    eprintln!("[vericore] embed created memory warning: {err}")
                                }
                            }
                        }

                        let history_write =
                            append_and_compact_history(&shared.history_root, &turn_event)
                                .map_err(|err| err.to_string())
                                .ok();

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

                                result["memory"]["embed_on_ingest"] =
                                    json!(should_embed_on_ingest());
                                result["memory"]["embedded_items"] = json!(embedded_created_items);

                                if let Some(err) = memory_save_error {
                                    result["memory"]["save_error"] = json!(err);
                                }

                                if let Some(history) = history_write {
                                    result["history"] = json!({
                                        "daily_file": history.daily_file,
                                        "rolled_up_files": history.rolled_up_files,
                                    });
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

fn should_embed_on_ingest() -> bool {
    env_bool("VERICORE_EMBED_ON_INGEST", MEMORY_EMBED_ON_INGEST_DEFAULT)
}

fn should_embed_on_startup_backfill() -> bool {
    env_bool(
        "VERICORE_EMBED_BACKFILL_ON_START",
        MEMORY_EMBED_ON_STARTUP_BACKFILL_DEFAULT,
    )
}

fn sleep_refine_interval() -> Option<Duration> {
    let secs = env_u64(
        "VERICORE_MEMORY_SLEEP_INTERVAL_SECS",
        MEMORY_SLEEP_INTERVAL_SECS_DEFAULT,
    );
    (secs > 0).then(|| Duration::from_secs(secs))
}

fn sleep_refine_embed() -> bool {
    env_bool("VERICORE_MEMORY_SLEEP_EMBED", MEMORY_SLEEP_EMBED_DEFAULT)
}

fn env_bool(name: &str, default: bool) -> bool {
    let Ok(raw) = std::env::var(name) else {
        return default;
    };
    match raw.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => true,
        "0" | "false" | "no" | "off" => false,
        _ => default,
    }
}

fn env_u64(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|raw| raw.trim().parse::<u64>().ok())
        .unwrap_or(default)
}

async fn run_sleep_refine_loop(
    shared: Arc<SharedState>,
    interval: Duration,
    include_embeddings: bool,
) {
    loop {
        time::sleep(interval).await;
        match run_memory_refine_once(shared.as_ref(), include_embeddings).await {
            Ok(result) => {
                eprintln!("[vericore] memory sleep refine: {}", result);
            }
            Err(err) => {
                eprintln!("[vericore] memory sleep refine warning: {err}");
            }
        }
    }
}

async fn run_memory_refine_once(
    shared: &SharedState,
    include_embeddings: bool,
) -> Result<Value, String> {
    let (refine, source_dates_filled, missing_embedding_ids, save_error) = {
        let mut memory = shared.memory.lock().await;

        let refine: MemoryRefineReport = memory.sleep_refine();

        let missing_source: Vec<(MemoryId, i64)> = memory
            .ids_missing_source_date()
            .into_iter()
            .filter_map(|id| memory.get(id).map(|item| (id, item.created_at)))
            .collect();

        let mut source_dates_filled = 0usize;
        for (id, created_at) in missing_source {
            let day = ts_to_utc(created_at)
                .date_naive()
                .format("%Y-%m-%d")
                .to_string();
            if memory.set_source_date(id, Some(day)) {
                source_dates_filled = source_dates_filled.saturating_add(1);
            }
        }

        let missing_embedding_ids = if include_embeddings {
            memory.ids_missing_embeddings()
        } else {
            Vec::new()
        };
        let save_error = memory.save_json(&shared.memory_path).err();

        (
            refine,
            source_dates_filled,
            missing_embedding_ids,
            save_error,
        )
    };

    let embedded_items = if include_embeddings && !missing_embedding_ids.is_empty() {
        embed_and_attach_items(shared, &missing_embedding_ids).await?
    } else {
        0
    };

    let stats_after = {
        let memory = shared.memory.lock().await;
        memory.stats()
    };

    Ok(json!({
        "refine": {
            "before_items": refine.before_items,
            "after_items": refine.after_items,
            "removed_items": refine.removed_items,
            "merged_groups": refine.merged_groups,
            "source_dates_filled": source_dates_filled,
            "embedded_items": embedded_items,
            "embed_requested": include_embeddings,
        },
        "memory": {
            "file": shared.memory_path.display().to_string(),
            "total_items": stats_after.total_items,
            "by_tier": stats_after.by_tier,
            "by_type": stats_after.by_type,
        },
        "save_error": save_error,
    }))
}

fn trim_history(mut history: Vec<ChatMessage>, limit: usize) -> Vec<ChatMessage> {
    if limit == 0 {
        return Vec::new();
    }
    if history.len() <= limit {
        return history;
    }

    let mut start = history.len().saturating_sub(limit);

    // Never start inside a tool-result block; providers require each tool_result
    // to follow the matching assistant tool_use in the immediately previous message.
    while start < history.len() && history[start].role == "tool" {
        start += 1;
    }

    history.drain(0..start);
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

#[derive(Debug, Clone)]
struct HistoryWriteResult {
    daily_file: String,
    rolled_up_files: usize,
}

fn resolve_history_root(config: &Config, config_path: &Path) -> PathBuf {
    if let Ok(raw) = std::env::var("VERICORE_HISTORY_DIR") {
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }

    if config.paths.home_root.is_absolute() {
        return config.paths.home_root.join("memory").join("history");
    }

    config_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("history")
}

fn ensure_history_dirs(root: &Path) -> Result<(), String> {
    fs::create_dir_all(root.join("daily"))
        .map_err(|e| format!("failed to create history daily dir {}: {e}", root.display()))?;
    fs::create_dir_all(root.join("weekly")).map_err(|e| {
        format!(
            "failed to create history weekly dir {}: {e}",
            root.display()
        )
    })?;
    fs::create_dir_all(root.join("daily-merged")).map_err(|e| {
        format!(
            "failed to create history daily-merged dir {}: {e}",
            root.display()
        )
    })?;
    Ok(())
}

fn append_and_compact_history(
    root: &Path,
    event: &TurnEvent,
) -> Result<HistoryWriteResult, String> {
    ensure_history_dirs(root)?;

    let dt = ts_to_utc(event.timestamp);
    let day = dt.format("%Y-%m-%d").to_string();
    let iso_ts = dt.to_rfc3339();
    let daily_path = root.join("daily").join(format!("{day}.md"));

    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&daily_path)
        .map_err(|e| format!("open daily history {}: {e}", daily_path.display()))?;

    if file
        .metadata()
        .map_err(|e| format!("stat daily history {}: {e}", daily_path.display()))?
        .len()
        == 0
    {
        writeln!(file, "# History {day}\n")
            .map_err(|e| format!("write daily history header {}: {e}", daily_path.display()))?;
    }

    writeln!(
        file,
        "## {iso_ts} | session={}\n- user: {}\n- assistant: {}\n",
        event.session_key,
        truncate_chars(event.user_text.trim(), HISTORY_MAX_USER_CHARS),
        truncate_chars(event.assistant_text.trim(), HISTORY_MAX_ASSISTANT_CHARS),
    )
    .map_err(|e| format!("append daily history {}: {e}", daily_path.display()))?;

    let rolled_up_files = compact_daily_history_into_weekly(root, dt.date_naive())?;

    Ok(HistoryWriteResult {
        daily_file: daily_path.display().to_string(),
        rolled_up_files,
    })
}

fn compact_daily_history_into_weekly(root: &Path, now: NaiveDate) -> Result<usize, String> {
    let daily_dir = root.join("daily");
    let weekly_dir = root.join("weekly");
    let merged_dir = root.join("daily-merged");

    let now_week = now.iso_week();
    let mut moved = 0usize;

    let entries = fs::read_dir(&daily_dir)
        .map_err(|e| format!("read history daily dir {}: {e}", daily_dir.display()))?;

    for entry in entries {
        let entry = entry.map_err(|e| format!("iterate history daily dir: {e}"))?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !name.ends_with(".md") {
            continue;
        }

        let day_str = name.trim_end_matches(".md");
        let Ok(day) = NaiveDate::parse_from_str(day_str, "%Y-%m-%d") else {
            continue;
        };

        let week = day.iso_week();
        if week.year() == now_week.year() && week.week() == now_week.week() {
            continue;
        }

        let week_key = format!("{}-W{:02}", week.year(), week.week());
        let weekly_path = weekly_dir.join(format!("{week_key}.md"));
        let content = fs::read_to_string(&path)
            .map_err(|e| format!("read daily history {}: {e}", path.display()))?;

        let mut weekly = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&weekly_path)
            .map_err(|e| format!("open weekly history {}: {e}", weekly_path.display()))?;

        if weekly
            .metadata()
            .map_err(|e| format!("stat weekly history {}: {e}", weekly_path.display()))?
            .len()
            == 0
        {
            writeln!(weekly, "# Weekly History {week_key}\n").map_err(|e| {
                format!("write weekly history header {}: {e}", weekly_path.display())
            })?;
        }

        writeln!(
            weekly,
            "\n---\n<!-- merged-from: {day_str} -->\n## Day {day_str}\n\n{content}"
        )
        .map_err(|e| format!("append weekly history {}: {e}", weekly_path.display()))?;

        let merged_target = merged_dir.join(name);
        fs::rename(&path, &merged_target).map_err(|e| {
            format!(
                "move merged daily history {} -> {}: {e}",
                path.display(),
                merged_target.display()
            )
        })?;
        moved = moved.saturating_add(1);
    }

    Ok(moved)
}

fn ts_to_utc(ts: i64) -> DateTime<Utc> {
    DateTime::from_timestamp(ts, 0).unwrap_or_else(Utc::now)
}

fn now_ts() -> i64 {
    Utc::now().timestamp()
}

fn count_markdown_files(dir: &Path) -> usize {
    let Ok(entries) = fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .filter_map(|entry| entry.file_name().to_str().map(str::to_string))
        .filter(|name| name.ends_with(".md"))
        .count()
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

fn parse_memory_tier(raw: &str) -> Option<MemoryTier> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "public" => Some(MemoryTier::Public),
        "family" => Some(MemoryTier::Family),
        "private" => Some(MemoryTier::Private),
        "top_secret" | "top-secret" | "topsecret" => Some(MemoryTier::TopSecret),
        _ => None,
    }
}

struct MemoryContextBlock {
    text: String,
    item_count: usize,
}

fn build_memory_context_block(
    memory: &MemoryCortex,
    query: &str,
    query_embedding: Option<&[f32]>,
    tier: MemoryTier,
) -> MemoryContextBlock {
    let mut lines: Vec<String> = Vec::new();
    let mut seen: HashSet<u64> = HashSet::new();

    for hit in memory.search_hybrid(query, query_embedding, tier, MEMORY_QUERY_LIMIT) {
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

#[derive(Debug, Serialize)]
struct OllamaEmbeddingRequest<'a> {
    model: &'a str,
    prompt: &'a str,
}

#[derive(Debug, Deserialize)]
struct OllamaEmbeddingResponse {
    embedding: Vec<f32>,
}

#[derive(Debug, Clone)]
struct BootstrapEntry {
    summary: String,
    ts: i64,
    source_date: Option<String>,
    memory_type: MemoryType,
    categories: Vec<String>,
}

async fn fetch_ollama_embedding(text: &str) -> Result<Vec<f32>, String> {
    let url = std::env::var("VERICORE_EMBED_URL")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| OLLAMA_EMBED_DEFAULT_URL.to_string());
    let model = std::env::var("VERICORE_EMBED_MODEL")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| OLLAMA_EMBED_DEFAULT_MODEL.to_string());

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(OLLAMA_EMBED_TIMEOUT_SECS))
        .build()
        .map_err(|e| format!("create embedding client: {e}"))?;
    let response = client
        .post(url)
        .json(&OllamaEmbeddingRequest {
            model: &model,
            prompt: text,
        })
        .send()
        .await
        .map_err(|e| format!("embedding request failed: {e}"))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!("embedding request HTTP {}: {}", status, body));
    }

    let parsed: OllamaEmbeddingResponse = response
        .json()
        .await
        .map_err(|e| format!("parse embedding response: {e}"))?;
    if parsed.embedding.is_empty() {
        return Err("embedding response returned empty vector".to_string());
    }
    Ok(parsed.embedding)
}

async fn embed_and_attach_items(shared: &SharedState, ids: &[MemoryId]) -> Result<usize, String> {
    let to_embed: Vec<(MemoryId, String)> = {
        let memory = shared.memory.lock().await;
        ids.iter()
            .filter_map(|id| memory.get(*id).map(|item| (*id, item.summary.clone())))
            .collect()
    };

    if to_embed.is_empty() {
        return Ok(0);
    }

    let mut embedded = 0usize;
    let mut pending: Vec<(MemoryId, Vec<f32>)> = Vec::new();
    for (id, summary) in to_embed {
        match fetch_ollama_embedding(&summary).await {
            Ok(vec) => pending.push((id, vec)),
            Err(err) => eprintln!("[vericore] embedding skipped for item {id}: {err}"),
        }
    }

    if pending.is_empty() {
        return Ok(0);
    }

    {
        let mut memory = shared.memory.lock().await;
        for (id, vec) in pending {
            if memory.set_embedding(id, vec) {
                embedded = embedded.saturating_add(1);
            }
        }
        if let Err(err) = memory.save_json(&shared.memory_path) {
            eprintln!("[vericore] failed to save memory after embedding: {err}");
        }
    }

    Ok(embedded)
}

async fn backfill_missing_embeddings(shared: &SharedState) -> Result<(), String> {
    let missing = {
        let memory = shared.memory.lock().await;
        memory.ids_missing_embeddings()
    };
    if missing.is_empty() {
        return Ok(());
    }
    let count = embed_and_attach_items(shared, &missing).await?;
    eprintln!("[vericore] embedded {count} existing memory items");
    Ok(())
}

async fn bootstrap_memory_from_markdown(shared: &SharedState) -> Result<(), String> {
    let files = bootstrap_markdown_files(&shared.config.paths.home_root, &shared.history_root);
    if files.is_empty() {
        return Ok(());
    }

    let mut created_ids: Vec<MemoryId> = Vec::new();
    let mut imported = 0usize;
    for path in files {
        let entries = extract_bootstrap_entries(&path)?;
        if entries.is_empty() {
            continue;
        }

        {
            let mut memory = shared.memory.lock().await;
            for entry in entries {
                let (id, created) = memory.remember_if_new_at(
                    CreateMemoryInput {
                        tier: MemoryTier::Private,
                        memory_type: entry.memory_type,
                        summary: entry.summary,
                        categories: entry.categories,
                        source_session: Some(format!("bootstrap:{}", path.display())),
                    },
                    entry.ts,
                );
                if memory.set_source_date(id, entry.source_date.clone()) {
                    // no-op
                }
                if created {
                    created_ids.push(id);
                    imported = imported.saturating_add(1);
                }
            }
            if let Err(err) = memory.save_json(&shared.memory_path) {
                eprintln!("[vericore] failed to save memory after bootstrap import: {err}");
            }
        }
    }

    if !created_ids.is_empty() {
        let embedded = embed_and_attach_items(shared, &created_ids).await?;
        eprintln!("[vericore] imported {imported} markdown memory items, embedded {embedded}");
    }

    Ok(())
}

fn bootstrap_markdown_files(home_root: &Path, history_root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    files.push(home_root.join("MEMORY.md"));
    push_markdown_files(&home_root.join("memory"), &mut files, false);
    // Private long-term notes that should remain private but be searchable in DM/family contexts.
    push_markdown_files(&home_root.join("private").join("memories"), &mut files, true);
    push_markdown_files(&home_root.join("private").join("CzechStudy"), &mut files, true);
    push_markdown_files(&history_root.join("daily"), &mut files, true);
    push_markdown_files(&history_root.join("daily-merged"), &mut files, true);
    files.retain(|p| p.is_file());
    files.sort();
    files.dedup();
    files
}

fn push_markdown_files(dir: &Path, out: &mut Vec<PathBuf>, recursive: bool) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file()
            && path
                .extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| ext.eq_ignore_ascii_case("md"))
        {
            out.push(path);
            continue;
        }
        if recursive && path.is_dir() {
            push_markdown_files(&path, out, true);
        }
    }
}

fn extract_bootstrap_entries(path: &Path) -> Result<Vec<BootstrapEntry>, String> {
    let text = fs::read_to_string(path)
        .map_err(|e| format!("read bootstrap memory file {}: {e}", path.display()))?;

    let default_day = infer_day_from_filename(path);
    let mut current_ts = default_day
        .as_deref()
        .and_then(day_to_ts)
        .unwrap_or_else(|| now_ts());
    let mut current_day = default_day.clone();
    let mut entries = Vec::new();
    let mut seen = HashSet::new();

    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }

        if line.starts_with("## ") || line.starts_with("# ") {
            if let Some(ts) = parse_ts_from_text(line) {
                current_ts = ts;
                current_day = Some(ts_to_utc(ts).date_naive().format("%Y-%m-%d").to_string());
            } else if let Some(day) = find_day_in_text(line) {
                current_ts = day_to_ts(&day).unwrap_or(current_ts);
                current_day = Some(day);
            }
            continue;
        }

        let candidate = if let Some(body) = line.strip_prefix("- ") {
            body.trim()
        } else if let Some(body) = line.strip_prefix("* ") {
            body.trim()
        } else {
            continue;
        };

        if candidate.len() < 18 {
            continue;
        }
        let normalized = candidate.to_lowercase();
        if !seen.insert(normalized) {
            continue;
        }

        let trimmed = truncate_chars(candidate, BOOTSTRAP_MAX_LINE_CHARS);
        let mut categories = vec!["bootstrap".to_string()];
        let path_str = path.display().to_string();
        if path_str.contains("/history/") {
            categories.push("history".to_string());
        }
        if path_str.contains("/daily/") {
            categories.push("daily".to_string());
        }
        if path_str.contains("/daily-merged/") {
            categories.push("daily-merged".to_string());
        }
        if let Some(day) = &current_day {
            categories.push(format!("date:{day}"));
        }

        let memory_type = if path_str.contains("/history/") {
            MemoryType::Event
        } else {
            MemoryType::ProjectState
        };

        entries.push(BootstrapEntry {
            summary: trimmed,
            ts: current_ts,
            source_date: current_day.clone(),
            memory_type,
            categories,
        });

        if entries.len() >= BOOTSTRAP_MAX_ITEMS_PER_FILE {
            break;
        }
    }

    Ok(entries)
}

fn infer_day_from_filename(path: &Path) -> Option<String> {
    let stem = path.file_stem()?.to_str()?;
    if stem.len() == 10 && is_ymd(stem) {
        Some(stem.to_string())
    } else {
        None
    }
}

fn parse_ts_from_text(text: &str) -> Option<i64> {
    for token in text.split_whitespace() {
        let cleaned = token.trim_matches(|c: char| {
            !(c.is_ascii_alphanumeric() || c == '-' || c == ':' || c == 'T' || c == 'Z' || c == '+')
        });
        if let Ok(dt) = DateTime::parse_from_rfc3339(cleaned) {
            return Some(dt.timestamp());
        }
    }
    None
}

fn find_day_in_text(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    if bytes.len() < 10 {
        return None;
    }
    for i in 0..=bytes.len().saturating_sub(10) {
        let slice = &bytes[i..i + 10];
        let Ok(day) = std::str::from_utf8(slice) else {
            continue;
        };
        if is_ymd(day) && NaiveDate::parse_from_str(day, "%Y-%m-%d").is_ok() {
            return Some(day.to_string());
        }
    }
    None
}

fn is_ymd(day: &str) -> bool {
    let b = day.as_bytes();
    b.len() == 10
        && b[0].is_ascii_digit()
        && b[1].is_ascii_digit()
        && b[2].is_ascii_digit()
        && b[3].is_ascii_digit()
        && b[4] == b'-'
        && b[5].is_ascii_digit()
        && b[6].is_ascii_digit()
        && b[7] == b'-'
        && b[8].is_ascii_digit()
        && b[9].is_ascii_digit()
}

fn day_to_ts(day: &str) -> Option<i64> {
    let date = NaiveDate::parse_from_str(day, "%Y-%m-%d").ok()?;
    date.and_hms_opt(12, 0, 0)
        .map(|dt| dt.and_utc().timestamp())
}

#[cfg(test)]
mod tests {
    use super::trim_history;
    use crate::llm::{ChatMessage, ToolCall};
    use serde_json::json;

    #[test]
    fn trim_history_never_starts_with_tool_result() {
        let history = vec![
            ChatMessage::user("u1"),
            ChatMessage::assistant_with_tools(
                Some("calling"),
                &[ToolCall {
                    id: "call_1".into(),
                    name: "read_file".into(),
                    arguments: json!({"path": "/tmp/x"}),
                }],
            ),
            ChatMessage::tool_result("call_1", "read_file", "ok"),
            ChatMessage::assistant("done"),
        ];

        let trimmed = trim_history(history, 2);
        assert_eq!(trimmed.len(), 1);
        assert_ne!(trimmed[0].role, "tool");
        assert_eq!(trimmed[0].role, "assistant");
    }

    #[test]
    fn trim_history_keeps_valid_suffix() {
        let history = vec![
            ChatMessage::user("u1"),
            ChatMessage::assistant("a1"),
            ChatMessage::user("u2"),
            ChatMessage::assistant("a2"),
        ];

        let trimmed = trim_history(history, 3);
        assert_eq!(trimmed.len(), 3);
        assert_eq!(trimmed[0].role, "assistant");
        assert_eq!(trimmed[1].role, "user");
        assert_eq!(trimmed[2].role, "assistant");
    }
}
