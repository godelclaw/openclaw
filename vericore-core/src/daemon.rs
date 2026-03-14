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
use crate::llm::{CallKind, ChatMessage, GatewayLlmClient, PromptMode};
use crate::policy::GatePolicy;
use crate::turn::run_turn_with_history_with_reviewer_memory;
use crate::types::ContextTier;
use crate::utils::sha256_hex;

const MAX_REQUEST_BYTES: usize = 1_048_576; // 1 MiB
const IO_TIMEOUT: Duration = Duration::from_secs(15);
const MEMORY_QUERY_LIMIT: usize = 6;
const MEMORY_HOT_LIMIT: usize = 4;
const MEMORY_HOT_MIN_REINFORCEMENT: u32 = 2;
const OLLAMA_EMBED_TIMEOUT_SECS: u64 = 2;
const OLLAMA_EMBED_DEFAULT_MODEL: &str = "nomic-embed-text";
const OLLAMA_EMBED_DEFAULT_URL: &str = "http://127.0.0.1:11434/api/embeddings";
const BOOTSTRAP_MAX_LINE_CHARS: usize = 320;
const BOOTSTRAP_MAX_ITEMS_PER_FILE: usize = 200;
const HISTORY_MAX_USER_CHARS: usize = 800;
const HISTORY_MAX_ASSISTANT_CHARS: usize = 1200;
const MIDTERM_CHUNK_MAX_CHARS: usize = 6000;
const MIDTERM_MAX_TURNS_PER_CHUNK: usize = 8;
const MINDLOCK_VIEW_PREVIEW_MAX_CHARS: usize = 2000;
const MEMORY_EMBED_ON_INGEST_DEFAULT: bool = false;
const MEMORY_EMBED_ON_STARTUP_BACKFILL_DEFAULT: bool = false;
const MEMORY_SLEEP_INTERVAL_SECS_DEFAULT: u64 = 0;
const MEMORY_SLEEP_EMBED_DEFAULT: bool = false;
const MEMORY_HEURISTIC_INGEST_DEFAULT: bool = false;

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
    pub history_date: Option<String>,
    #[serde(default)]
    pub operator_approved: Option<bool>,
    #[serde(default)]
    pub artifact_id: Option<String>,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub stage: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub agents_available: Option<bool>,
    #[serde(default)]
    pub agents_injected: Option<bool>,
    #[serde(default)]
    pub soul_available: Option<bool>,
    #[serde(default)]
    pub soul_injected: Option<bool>,
    #[serde(default)]
    pub heartbeat_canonical_available: Option<bool>,
    #[serde(default)]
    pub heartbeat_mirror_available: Option<bool>,
    #[serde(default)]
    pub heartbeat_mirror_matches: Option<bool>,
    #[serde(default)]
    pub heartbeat_prompt_uses_file_reference: Option<bool>,
    #[serde(default)]
    pub heartbeat_prompt_mentions_legacy_gas: Option<bool>,
    #[serde(default)]
    pub heartbeat_context_recent_turn_count: Option<usize>,
    #[serde(default)]
    pub heartbeat_context_recent_turn_limit: Option<usize>,
    #[serde(default)]
    pub heartbeat_context_reserved_candidate_present: Option<bool>,
    #[serde(default)]
    pub heartbeat_context_reserved_included: Option<bool>,
    #[serde(default)]
    pub heartbeat_context_content_heartbeat_candidate_present: Option<bool>,
    #[serde(default)]
    pub heartbeat_context_content_heartbeat_included: Option<bool>,
    #[serde(default)]
    pub heartbeat_context_noop_heartbeat_in_recent_turns: Option<bool>,
    #[serde(default)]
    pub heartbeat_context_trace_count: Option<usize>,
    #[serde(default)]
    pub heartbeat_context_trace_limit: Option<usize>,
    #[serde(default)]
    pub heartbeat_context_trace_oldest_first: Option<bool>,
    #[serde(default)]
    pub heartbeat_context_trace_only_heartbeat_entries: Option<bool>,
    #[serde(default)]
    pub energy_noop_heartbeat_count: Option<usize>,
    #[serde(default)]
    pub energy_acted_heartbeat_count: Option<usize>,
    #[serde(default)]
    pub energy_conversation_burst_count: Option<usize>,
    #[serde(default)]
    pub energy_conversation_gap_ms: Option<usize>,
    #[serde(default)]
    pub energy_value: Option<i64>,
    #[serde(default)]
    pub affect_gamma: Option<usize>,
    #[serde(default)]
    pub affect_prev: Option<Vec<i64>>,
    #[serde(default)]
    pub affect_obs: Option<Vec<i64>>,
    #[serde(default)]
    pub affect_result: Option<Vec<i64>>,
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
    memory: Arc<Mutex<MemoryCortex>>,
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
            memory: Arc::new(Mutex::new(memory)),
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
        "startup_identity_validate" => {
            let Some(agents_available) = request.agents_available else {
                return DaemonResponse::err(
                    request.id,
                    "missing agents_available for method=startup_identity_validate",
                );
            };
            let Some(agents_injected) = request.agents_injected else {
                return DaemonResponse::err(
                    request.id,
                    "missing agents_injected for method=startup_identity_validate",
                );
            };
            let Some(soul_available) = request.soul_available else {
                return DaemonResponse::err(
                    request.id,
                    "missing soul_available for method=startup_identity_validate",
                );
            };
            let Some(soul_injected) = request.soul_injected else {
                return DaemonResponse::err(
                    request.id,
                    "missing soul_injected for method=startup_identity_validate",
                );
            };

            DaemonResponse::ok(
                request.id,
                build_startup_identity_contract_result(
                    agents_available,
                    agents_injected,
                    soul_available,
                    soul_injected,
                ),
            )
        }
        "heartbeat_sync_validate" => {
            let Some(canonical_available) = request.heartbeat_canonical_available else {
                return DaemonResponse::err(
                    request.id,
                    "missing heartbeat_canonical_available for method=heartbeat_sync_validate",
                );
            };
            let Some(mirror_available) = request.heartbeat_mirror_available else {
                return DaemonResponse::err(
                    request.id,
                    "missing heartbeat_mirror_available for method=heartbeat_sync_validate",
                );
            };
            let Some(mirror_matches) = request.heartbeat_mirror_matches else {
                return DaemonResponse::err(
                    request.id,
                    "missing heartbeat_mirror_matches for method=heartbeat_sync_validate",
                );
            };
            let Some(prompt_uses_file_reference) = request.heartbeat_prompt_uses_file_reference
            else {
                return DaemonResponse::err(
                    request.id,
                    "missing heartbeat_prompt_uses_file_reference for method=heartbeat_sync_validate",
                );
            };
            let Some(prompt_mentions_legacy_gas) = request.heartbeat_prompt_mentions_legacy_gas
            else {
                return DaemonResponse::err(
                    request.id,
                    "missing heartbeat_prompt_mentions_legacy_gas for method=heartbeat_sync_validate",
                );
            };

            DaemonResponse::ok(
                request.id,
                build_heartbeat_sync_contract_result(
                    canonical_available,
                    mirror_available,
                    mirror_matches,
                    prompt_uses_file_reference,
                    prompt_mentions_legacy_gas,
                ),
            )
        }
        "heartbeat_context_validate" => {
            let Some(recent_turn_count) = request.heartbeat_context_recent_turn_count else {
                return DaemonResponse::err(
                    request.id,
                    "missing heartbeat_context_recent_turn_count for method=heartbeat_context_validate",
                );
            };
            let Some(recent_turn_limit) = request.heartbeat_context_recent_turn_limit else {
                return DaemonResponse::err(
                    request.id,
                    "missing heartbeat_context_recent_turn_limit for method=heartbeat_context_validate",
                );
            };
            let Some(reserved_candidate_present) =
                request.heartbeat_context_reserved_candidate_present
            else {
                return DaemonResponse::err(
                    request.id,
                    "missing heartbeat_context_reserved_candidate_present for method=heartbeat_context_validate",
                );
            };
            let Some(reserved_included) = request.heartbeat_context_reserved_included else {
                return DaemonResponse::err(
                    request.id,
                    "missing heartbeat_context_reserved_included for method=heartbeat_context_validate",
                );
            };
            let Some(content_heartbeat_candidate_present) =
                request.heartbeat_context_content_heartbeat_candidate_present
            else {
                return DaemonResponse::err(
                    request.id,
                    "missing heartbeat_context_content_heartbeat_candidate_present for method=heartbeat_context_validate",
                );
            };
            let Some(content_heartbeat_included) =
                request.heartbeat_context_content_heartbeat_included
            else {
                return DaemonResponse::err(
                    request.id,
                    "missing heartbeat_context_content_heartbeat_included for method=heartbeat_context_validate",
                );
            };
            let Some(noop_heartbeat_in_recent_turns) =
                request.heartbeat_context_noop_heartbeat_in_recent_turns
            else {
                return DaemonResponse::err(
                    request.id,
                    "missing heartbeat_context_noop_heartbeat_in_recent_turns for method=heartbeat_context_validate",
                );
            };
            let Some(heartbeat_trace_count) = request.heartbeat_context_trace_count else {
                return DaemonResponse::err(
                    request.id,
                    "missing heartbeat_context_trace_count for method=heartbeat_context_validate",
                );
            };
            let Some(heartbeat_trace_limit) = request.heartbeat_context_trace_limit else {
                return DaemonResponse::err(
                    request.id,
                    "missing heartbeat_context_trace_limit for method=heartbeat_context_validate",
                );
            };
            let Some(heartbeat_trace_oldest_first) =
                request.heartbeat_context_trace_oldest_first
            else {
                return DaemonResponse::err(
                    request.id,
                    "missing heartbeat_context_trace_oldest_first for method=heartbeat_context_validate",
                );
            };
            let Some(heartbeat_trace_only_heartbeat_entries) =
                request.heartbeat_context_trace_only_heartbeat_entries
            else {
                return DaemonResponse::err(
                    request.id,
                    "missing heartbeat_context_trace_only_heartbeat_entries for method=heartbeat_context_validate",
                );
            };

            DaemonResponse::ok(
                request.id,
                build_heartbeat_context_contract_result(
                    recent_turn_count,
                    recent_turn_limit,
                    reserved_candidate_present,
                    reserved_included,
                    content_heartbeat_candidate_present,
                    content_heartbeat_included,
                    noop_heartbeat_in_recent_turns,
                    heartbeat_trace_count,
                    heartbeat_trace_limit,
                    heartbeat_trace_oldest_first,
                    heartbeat_trace_only_heartbeat_entries,
                ),
            )
        }
        "energy_validate" => {
            let Some(noop_heartbeat_count) = request.energy_noop_heartbeat_count else {
                return DaemonResponse::err(
                    request.id,
                    "missing energy_noop_heartbeat_count for method=energy_validate",
                );
            };
            let Some(acted_heartbeat_count) = request.energy_acted_heartbeat_count else {
                return DaemonResponse::err(
                    request.id,
                    "missing energy_acted_heartbeat_count for method=energy_validate",
                );
            };
            let Some(conversation_burst_count) = request.energy_conversation_burst_count else {
                return DaemonResponse::err(
                    request.id,
                    "missing energy_conversation_burst_count for method=energy_validate",
                );
            };
            let Some(conversation_gap_ms) = request.energy_conversation_gap_ms else {
                return DaemonResponse::err(
                    request.id,
                    "missing energy_conversation_gap_ms for method=energy_validate",
                );
            };
            let Some(energy_value) = request.energy_value else {
                return DaemonResponse::err(
                    request.id,
                    "missing energy_value for method=energy_validate",
                );
            };

            DaemonResponse::ok(
                request.id,
                build_energy_contract_result(
                    noop_heartbeat_count,
                    acted_heartbeat_count,
                    conversation_burst_count,
                    conversation_gap_ms,
                    energy_value,
                ),
            )
        }
        "affect_validate" => {
            let Some(gamma) = request.affect_gamma else {
                return DaemonResponse::err(
                    request.id,
                    "missing affect_gamma for method=affect_validate",
                );
            };
            let prev = request.affect_prev.as_deref();
            let Some(ref obs) = request.affect_obs else {
                return DaemonResponse::err(
                    request.id,
                    "missing affect_obs for method=affect_validate",
                );
            };
            let Some(ref result) = request.affect_result else {
                return DaemonResponse::err(
                    request.id,
                    "missing affect_result for method=affect_validate",
                );
            };
            if prev.is_some_and(|values| values.len() != 7) || obs.len() != 7 || result.len() != 7 {
                return DaemonResponse::err(
                    request.id,
                    "affect vectors must each have exactly 7 elements",
                );
            }

            DaemonResponse::ok(
                request.id,
                build_affect_contract_result(gamma, prev, obs, result),
            )
        }
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
            let audit_daily = count_jsonl_files(&shared.history_root.join("audit"));

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
                    },
                    "audit": {
                        "root": shared.history_root.join("audit").display().to_string(),
                        "daily_files": audit_daily,
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
        "memory_extract_history" => {
            let date = request
                .history_date
                .as_deref()
                .and_then(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok())
                .unwrap_or_else(|| Utc::now().date_naive());
            match run_nightly_history_extract(shared, date).await {
                Ok(created) => DaemonResponse::ok(
                    request.id,
                    json!({"date": date.to_string(), "memories_created": created}),
                ),
                Err(err) => {
                    DaemonResponse::err(request.id, format!("memory_extract_history failed: {err}"))
                }
            }
        }
        "memory_refresh_midterm" => {
            match run_memory_refresh_midterm(shared).await {
                Ok(result) => DaemonResponse::ok(request.id, result),
                Err(err) => DaemonResponse::err(request.id, format!("memory_refresh_midterm failed: {err}")),
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
        "mindlock_status" => {
            let Some(stimulus_input) = request.stimulus.as_ref() else {
                return DaemonResponse::err(
                    request.id,
                    "missing stimulus for method=mindlock_status",
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
                    "mindlock_status is only allowed from private context",
                );
            }

            let mindlock_dir = shared.config.security_review.mindlock_dir.clone();
            let in_dir = mindlock_dir.join("in");
            let out_dir = mindlock_dir.join("out");
            let pending_dir = mindlock_dir.join("pending-zar");
            let rejected_dir = mindlock_dir.join("rejected");

            let in_count = match count_mindlock_items(&in_dir) {
                Ok(v) => v,
                Err(err) => {
                    return DaemonResponse::err(
                        request.id,
                        format!("mindlock_status failed: {err}"),
                    );
                }
            };
            let out_count = match count_mindlock_items(&out_dir) {
                Ok(v) => v,
                Err(err) => {
                    return DaemonResponse::err(
                        request.id,
                        format!("mindlock_status failed: {err}"),
                    );
                }
            };
            let pending_count = match count_mindlock_items(&pending_dir) {
                Ok(v) => v,
                Err(err) => {
                    return DaemonResponse::err(
                        request.id,
                        format!("mindlock_status failed: {err}"),
                    );
                }
            };
            let rejected_count = match count_mindlock_items(&rejected_dir) {
                Ok(v) => v,
                Err(err) => {
                    return DaemonResponse::err(
                        request.id,
                        format!("mindlock_status failed: {err}"),
                    );
                }
            };

            DaemonResponse::ok(
                request.id,
                json!({
                    "mindlock_dir": mindlock_dir.display().to_string(),
                    "counts": {
                        "in": in_count,
                        "out": out_count,
                        "pending": pending_count,
                        "rejected": rejected_count,
                    },
                    "as_of_ts": Utc::now().timestamp(),
                }),
            )
        }
        "mindlock_list" => {
            let Some(stimulus_input) = request.stimulus.as_ref() else {
                return DaemonResponse::err(
                    request.id,
                    "missing stimulus for method=mindlock_list",
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
                    "mindlock_list is only allowed from private context",
                );
            }

            let box_name = match normalize_mindlock_box(request.stage.as_deref()) {
                Some(v) => v,
                None => {
                    return DaemonResponse::err(
                        request.id,
                        "mindlock_list stage must be one of: in, out, pending, rejected",
                    );
                }
            };
            let list_limit = request.limit.unwrap_or(25).clamp(1, 200);

            let mindlock_dir = shared.config.security_review.mindlock_dir.clone();
            let stage_dir = match box_name {
                "in" => mindlock_dir.join("in"),
                "out" => mindlock_dir.join("out"),
                "pending" => mindlock_dir.join("pending-zar"),
                "rejected" => mindlock_dir.join("rejected"),
                _ => unreachable!(),
            };

            let items = match collect_mindlock_artifacts(&stage_dir, list_limit) {
                Ok(v) => v,
                Err(err) => {
                    return DaemonResponse::err(request.id, format!("mindlock_list failed: {err}"));
                }
            };

            DaemonResponse::ok(
                request.id,
                json!({
                    "mindlock_dir": mindlock_dir.display().to_string(),
                    "box": box_name,
                    "dir": stage_dir.display().to_string(),
                    "count": items.len(),
                    "items": items,
                    "as_of_ts": Utc::now().timestamp(),
                }),
            )
        }
        "mindlock_pending" => {
            let Some(stimulus_input) = request.stimulus.as_ref() else {
                return DaemonResponse::err(
                    request.id,
                    "missing stimulus for method=mindlock_pending",
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
                    "mindlock_pending is only allowed from private context",
                );
            }

            let pending_dir = shared
                .config
                .security_review
                .mindlock_dir
                .join("pending-zar");
            let items = match collect_pending_artifacts(&pending_dir) {
                Ok(items) => items,
                Err(err) => {
                    return DaemonResponse::err(
                        request.id,
                        format!("mindlock_pending failed: {err}"),
                    );
                }
            };

            DaemonResponse::ok(
                request.id,
                json!({
                    "mindlock_dir": shared.config.security_review.mindlock_dir.display().to_string(),
                    "pending_dir": pending_dir.display().to_string(),
                    "count": items.len(),
                    "items": items,
                    "as_of_ts": Utc::now().timestamp(),
                }),
            )
        }
        "mindlock_view" => {
            let Some(stimulus_input) = request.stimulus.as_ref() else {
                return DaemonResponse::err(
                    request.id,
                    "missing stimulus for method=mindlock_view",
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
                    "mindlock_view is only allowed from private context",
                );
            }

            let Some(artifact_id) = request.artifact_id.as_deref() else {
                return DaemonResponse::err(
                    request.id,
                    "missing artifact_id for method=mindlock_view",
                );
            };

            let pending_dir = shared
                .config
                .security_review
                .mindlock_dir
                .join("pending-zar");
            let result = match build_mindlock_pending_view(&pending_dir, artifact_id) {
                Ok(result) => result,
                Err(err) => {
                    return DaemonResponse::err(request.id, format!("mindlock_view failed: {err}"));
                }
            };

            DaemonResponse::ok(request.id, result)
        }
        "mindlock_approve" => {
            let Some(stimulus_input) = request.stimulus.as_ref() else {
                return DaemonResponse::err(
                    request.id,
                    "missing stimulus for method=mindlock_approve",
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
                    "mindlock_approve is only allowed from private context",
                );
            }

            let Some(artifact_id) = request.artifact_id.as_deref() else {
                return DaemonResponse::err(
                    request.id,
                    "missing artifact_id for method=mindlock_approve",
                );
            };

            let reason = request
                .reason
                .as_deref()
                .filter(|r| !r.trim().is_empty())
                .unwrap_or("approved by zar");

            let pending_dir = shared
                .config
                .security_review
                .mindlock_dir
                .join("pending-zar");
            let rejected_dir = shared.config.security_review.mindlock_dir.join("rejected");
            let audit_dir = shared.config.security_review.mindlock_dir.join("audit");

            if let Err(err) = fs::create_dir_all(&audit_dir) {
                return DaemonResponse::err(
                    request.id,
                    format!("mindlock_approve failed to create audit dir: {err}"),
                );
            }
            if let Err(err) = fs::create_dir_all(&rejected_dir) {
                return DaemonResponse::err(
                    request.id,
                    format!("mindlock_approve failed to create rejected dir: {err}"),
                );
            }

            let resolved_selector = match resolve_pending_selector_id(&pending_dir, artifact_id) {
                Ok(id) => id,
                Err(err) => {
                    return DaemonResponse::err(
                        request.id,
                        format!("mindlock_approve failed: {err}"),
                    );
                }
            };
            let source_path = match resolve_pending_artifact_path(&pending_dir, &resolved_selector)
            {
                Ok(path) => path,
                Err(err) => {
                    return DaemonResponse::err(
                        request.id,
                        format!("mindlock_approve failed: {err}"),
                    );
                }
            };
            let meta_path = source_path.with_extension("meta.json");
            if !meta_path.exists() {
                return DaemonResponse::err(
                    request.id,
                    format!(
                        "mindlock_approve failed: missing target metadata {} (use /reject or re-stage with review metadata)",
                        meta_path.display()
                    ),
                );
            }
            let target_raw = match read_target_path_from_meta(&meta_path) {
                Ok(target) => target,
                Err(err) => {
                    return DaemonResponse::err(
                        request.id,
                        format!("mindlock_approve failed: {err}"),
                    );
                }
            };
            let target_path = match canonicalize_write_target_under_home(
                Path::new(&target_raw),
                &shared.config.paths.home_root,
            ) {
                Ok(path) => path,
                Err(err) => {
                    return DaemonResponse::err(
                        request.id,
                        format!("mindlock_approve failed: {err}"),
                    );
                }
            };

            let bytes = match fs::read(&source_path) {
                Ok(bytes) => bytes,
                Err(err) => {
                    return DaemonResponse::err(
                        request.id,
                        format!(
                            "mindlock_approve failed to read source {}: {err}",
                            source_path.display()
                        ),
                    );
                }
            };

            if let Some(parent) = target_path.parent()
                && let Err(err) = fs::create_dir_all(parent)
            {
                return DaemonResponse::err(
                    request.id,
                    format!(
                        "mindlock_approve failed to create target parent {}: {err}",
                        parent.display()
                    ),
                );
            }

            let content_hash = sha256_hex(&bytes);

            if let Err(err) = fs::write(&target_path, bytes) {
                return DaemonResponse::err(
                    request.id,
                    format!(
                        "mindlock_approve failed to write target {}: {err}",
                        target_path.display()
                    ),
                );
            }

            let archived_artifact =
                match archive_artifact_file(&source_path, &audit_dir, "approved") {
                    Ok(path) => path,
                    Err(err) => {
                        return DaemonResponse::err(
                            request.id,
                            format!("mindlock_approve failed to archive source: {err}"),
                        );
                    }
                };
            let archived_meta = archive_meta_file(&meta_path, &audit_dir, "approved").ok();

            // Write zar_approval provenance into the archived meta.
            if let Some(ref archived_meta_path) = archived_meta {
                if let Ok(meta_bytes) = fs::read(archived_meta_path) {
                    if let Ok(mut meta_json) = serde_json::from_slice::<Value>(&meta_bytes) {
                        meta_json["zar_approval"] = json!({
                            "hash": content_hash,
                            "approved_at": now_ts(),
                            "reason": reason,
                        });
                        if let Ok(updated) = serde_json::to_string_pretty(&meta_json) {
                            let _ = fs::write(archived_meta_path, updated.as_bytes());
                        }
                    }
                }
            }

            let event_id = source_path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or(artifact_id)
                .to_string();
            if let Err(err) = append_mindlock_audit_event(
                &audit_dir,
                "zar_approved",
                &event_id,
                &source_path,
                Some(&target_path),
                reason,
                Some(&content_hash),
            ) {
                eprintln!("[vericore] mindlock approve audit warning: {err}");
            }

            // Record security precedent for manual approval.
            {
                let mut memory = shared.memory.lock().await;
                let cap = shared.config.security_review.max_precedent_items;
                let count = memory.count_by_type(&MemoryType::SecurityPrecedent);
                if count >= cap {
                    memory.prune_oldest_by_type(&MemoryType::SecurityPrecedent, count - cap + 1);
                }
                memory.remember(CreateMemoryInput {
                    tier: MemoryTier::Private,
                    memory_type: MemoryType::SecurityPrecedent,
                    summary: format!(
                        "Mindlock artifact {} -> {} \u{2014} Zar approved. Reason: {}",
                        source_path.display(),
                        target_path.display(),
                        reason
                    ),
                    categories: vec![
                        "security".into(),
                        "mindlock".into(),
                        "zar_approved".into(),
                        "manual".into(),
                    ],
                    source_session: None,
                });
                if let Err(err) = memory.save_json(&shared.memory_path) {
                    eprintln!("[vericore] precedent save warning: {err}");
                }
            }

            DaemonResponse::ok(
                request.id,
                json!({
                    "id": event_id,
                    "status": "approved",
                    "source": source_path.display().to_string(),
                    "target": target_path.display().to_string(),
                    "archived_artifact": archived_artifact.display().to_string(),
                    "archived_meta": archived_meta.map(|p| p.display().to_string()),
                }),
            )
        }
        "mindlock_reject" => {
            let Some(stimulus_input) = request.stimulus.as_ref() else {
                return DaemonResponse::err(
                    request.id,
                    "missing stimulus for method=mindlock_reject",
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
                    "mindlock_reject is only allowed from private context",
                );
            }

            let Some(artifact_id) = request.artifact_id.as_deref() else {
                return DaemonResponse::err(
                    request.id,
                    "missing artifact_id for method=mindlock_reject",
                );
            };

            let reason = request
                .reason
                .as_deref()
                .filter(|r| !r.trim().is_empty())
                .unwrap_or("rejected by zar");

            let pending_dir = shared
                .config
                .security_review
                .mindlock_dir
                .join("pending-zar");
            let rejected_dir = shared.config.security_review.mindlock_dir.join("rejected");
            let audit_dir = shared.config.security_review.mindlock_dir.join("audit");

            if let Err(err) = fs::create_dir_all(&rejected_dir) {
                return DaemonResponse::err(
                    request.id,
                    format!("mindlock_reject failed to create rejected dir: {err}"),
                );
            }
            if let Err(err) = fs::create_dir_all(&audit_dir) {
                return DaemonResponse::err(
                    request.id,
                    format!("mindlock_reject failed to create audit dir: {err}"),
                );
            }

            let resolved_selector = match resolve_pending_selector_id(&pending_dir, artifact_id) {
                Ok(id) => id,
                Err(err) => {
                    return DaemonResponse::err(
                        request.id,
                        format!("mindlock_reject failed: {err}"),
                    );
                }
            };
            let source_path = match resolve_pending_artifact_path(&pending_dir, &resolved_selector)
            {
                Ok(path) => path,
                Err(err) => {
                    return DaemonResponse::err(
                        request.id,
                        format!("mindlock_reject failed: {err}"),
                    );
                }
            };
            let meta_path = source_path.with_extension("meta.json");

            let rejected_artifact =
                match archive_artifact_file(&source_path, &rejected_dir, "rejected") {
                    Ok(path) => path,
                    Err(err) => {
                        return DaemonResponse::err(
                            request.id,
                            format!("mindlock_reject failed to move source: {err}"),
                        );
                    }
                };
            let rejected_meta = archive_meta_file(&meta_path, &rejected_dir, "rejected").ok();

            let event_id = source_path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or(artifact_id)
                .to_string();
            if let Err(err) = append_mindlock_audit_event(
                &audit_dir,
                "zar_rejected",
                &event_id,
                &source_path,
                None,
                reason,
                None,
            ) {
                eprintln!("[vericore] mindlock reject audit warning: {err}");
            }

            // Record security precedent for manual rejection.
            {
                let mut memory = shared.memory.lock().await;
                let cap = shared.config.security_review.max_precedent_items;
                let count = memory.count_by_type(&MemoryType::SecurityPrecedent);
                if count >= cap {
                    memory.prune_oldest_by_type(&MemoryType::SecurityPrecedent, count - cap + 1);
                }
                memory.remember(CreateMemoryInput {
                    tier: MemoryTier::Private,
                    memory_type: MemoryType::SecurityPrecedent,
                    summary: format!(
                        "Mindlock artifact {} \u{2014} Zar rejected. Reason: {}",
                        source_path.display(),
                        reason
                    ),
                    categories: vec![
                        "security".into(),
                        "mindlock".into(),
                        "zar_rejected".into(),
                        "manual".into(),
                    ],
                    source_session: None,
                });
                if let Err(err) = memory.save_json(&shared.memory_path) {
                    eprintln!("[vericore] precedent save warning: {err}");
                }
            }

            DaemonResponse::ok(
                request.id,
                json!({
                    "id": event_id,
                    "status": "rejected",
                    "source": source_path.display().to_string(),
                    "rejected_artifact": rejected_artifact.display().to_string(),
                    "rejected_meta": rejected_meta.map(|p| p.display().to_string()),
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

                match run_turn_with_history_with_reviewer_memory(
                    &shared.config,
                    &shared.policy,
                    &stimulus,
                    &system_prompt_for_turn,
                    &history_before,
                    Some(Arc::clone(&shared.memory)),
                    Some(shared.memory_path.clone()),
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
                            assistant_text: response_text.clone(),
                            tier: memory_tier,
                        };

                        let (
                            created_ids,
                            created_memory_items,
                            memory_total_after,
                            memory_save_error,
                        ) = {
                            let mut memory = shared.memory.lock().await;
                            let created = if should_heuristic_ingest() {
                                memory.ingest_turn(turn_event.clone())
                            } else {
                                vec![]
                            };
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
                        let driver_audit_entry = json!({
                            "kind": "driver_turn",
                            "ts": stimulus.timestamp as i64,
                            "ts_iso": ts_to_utc(stimulus.timestamp as i64).to_rfc3339(),
                            "session_key": route.session_key.clone(),
                            "channel": route.channel.clone(),
                            "context": route.context.clone(),
                            "actor": input.actor.clone(),
                            "user_text": input.content.clone(),
                            "assistant_text": response_text,
                            "tier": memory_tier,
                            "tools_used": outcome.tools_used.clone(),
                            "prompt_tokens": outcome.prompt_tokens,
                            "completion_tokens": outcome.completion_tokens,
                            "security_hold": outcome.security_hold.clone(),
                            "model_resolution": outcome.model_resolution.clone(),
                        });
                        let driver_audit_write = append_driver_audit(
                            &shared.history_root,
                            stimulus.timestamp as i64,
                            &driver_audit_entry,
                        )
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

                                if let Some(audit) = driver_audit_write {
                                    result["audit"] = json!({
                                        "driver_daily_file": audit.daily_file,
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

fn should_heuristic_ingest() -> bool {
    env_bool("VERICORE_HEURISTIC_INGEST", MEMORY_HEURISTIC_INGEST_DEFAULT)
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

// ── Nightly history extraction ──────────────────────────────────────────────

#[derive(Clone)]
struct HistoryTurn {
    session_key: String,
    timestamp: i64,
    user_text: String,
    assistant_text: String,
}

fn parse_history_turns(content: &str) -> Vec<HistoryTurn> {
    let mut turns: Vec<HistoryTurn> = Vec::new();
    let mut current: Option<HistoryTurn> = None;
    let mut collecting: Option<&str> = None; // "user" | "assistant"

    for line in content.lines() {
        if line.starts_with("## ") && line.contains("| session=") {
            // Emit previous turn
            if let Some(t) = current.take() {
                turns.push(t);
            }
            collecting = None;

            // Parse timestamp and session key
            // Format: "## 2026-02-28T15:04:00+00:00 | session=telegram_dm:foo"
            let rest = &line[3..];
            let (ts_part, session_part) = if let Some(idx) = rest.find(" | session=") {
                (&rest[..idx], &rest[idx + " | session=".len()..])
            } else {
                continue;
            };

            let timestamp = DateTime::parse_from_rfc3339(ts_part.trim())
                .map(|dt| dt.timestamp())
                .unwrap_or(0);

            current = Some(HistoryTurn {
                session_key: session_part.trim().to_string(),
                timestamp,
                user_text: String::new(),
                assistant_text: String::new(),
            });
        } else if let Some(ref mut t) = current {
            if let Some(rest) = line.strip_prefix("- user: ") {
                collecting = Some("user");
                t.user_text = rest.to_string();
            } else if let Some(rest) = line.strip_prefix("- assistant: ") {
                collecting = Some("assistant");
                t.assistant_text = rest.to_string();
            } else if line.starts_with("- ") {
                // New bullet, unknown field — stop collecting
                collecting = None;
            } else if let Some(field) = collecting {
                // Continuation line
                match field {
                    "user" => {
                        t.user_text.push('\n');
                        t.user_text.push_str(line);
                    }
                    "assistant" => {
                        t.assistant_text.push('\n');
                        t.assistant_text.push_str(line);
                    }
                    _ => {}
                }
            }
        }
    }

    if let Some(t) = current {
        turns.push(t);
    }

    turns
}

fn history_session_key_to_tier(session_key: &str) -> MemoryTier {
    if session_key.starts_with("telegram_family:") {
        MemoryTier::Family
    } else if session_key.starts_with("telegram_public:") || session_key.starts_with("moltbook:") {
        MemoryTier::Public
    } else {
        MemoryTier::Private
    }
}

async fn run_nightly_history_extract(
    shared: &SharedState,
    date: NaiveDate,
) -> Result<usize, String> {
    let history_path = shared
        .history_root
        .join("daily-merged")
        .join(format!("{date}.md"));

    let content = match fs::read_to_string(&history_path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            eprintln!("[vericore] history extract: no file for {date}, skipping");
            return Ok(0);
        }
        Err(e) => return Err(format!("read history file {}: {e}", history_path.display())),
    };

    let turns = parse_history_turns(&content);
    eprintln!(
        "[vericore] history extract {date}: {} turns to process",
        turns.len()
    );

    let llm = GatewayLlmClient::from_config(
        &shared.config.llm,
        CallKind::MemoryRefine,
        PromptMode::Driver,
        None,
    )
    .map_err(|e| e.to_string())?;
    let system_msg = ChatMessage::system(
        "You extract compact factual memory snippets. Be terse and specific. No filler. No meta-commentary. Return ONLY a JSON array of strings, nothing else.",
    );

    let mut new_ids: Vec<MemoryId> = Vec::new();

    for turn in &turns {
        let combined_len = turn.user_text.len() + turn.assistant_text.len();
        if combined_len < 60 {
            continue; // Too short to be worth extracting
        }

        let tier = history_session_key_to_tier(&turn.session_key);
        let date_str = date.to_string();

        let prompt = format!(
            "Extract 0-5 tight factual snippets from this conversation turn. \
             Each snippet: one sentence, durable fact/preference/event/insight. \
             Skip greetings, small talk, time queries. \
             IMPORTANT: return ONLY a raw JSON array of strings, e.g. [\"fact one\", \"fact two\"]. \
             No object keys, no markdown, no explanation. Just the array. Return [] if nothing worth keeping. \
             Turn - user: {} - assistant: {}",
            truncate_chars(turn.user_text.trim(), 600),
            truncate_chars(turn.assistant_text.trim(), 800),
        );

        let messages = vec![system_msg.clone(), ChatMessage::user(&prompt)];

        let (result, _usage, _meta) = match llm.chat(&messages, &[]).await {
            Ok(r) => r,
            Err(e) => {
                eprintln!(
                    "[vericore] history extract: LLM call failed for turn {}: {e}",
                    turn.session_key
                );
                continue;
            }
        };

        let text = match result {
            crate::llm::LlmTurnResult::FinalResponse { content } => content,
            crate::llm::LlmTurnResult::ToolCalls { content, .. } => content.unwrap_or_default(),
        };

        // Parse JSON array of strings
        let snippets: Vec<String> = match serde_json::from_str::<Vec<String>>(text.trim()) {
            Ok(v) => v,
            Err(_) => {
                // Try extracting a JSON array if wrapped in other text
                let trimmed = text.trim();
                let start = trimmed.find('[').unwrap_or(0);
                let end = trimmed.rfind(']').map(|i| i + 1).unwrap_or(trimmed.len());
                match serde_json::from_str::<Vec<String>>(&trimmed[start..end]) {
                    Ok(v) => v,
                    Err(_) => {
                        eprintln!(
                            "[vericore] history extract: could not parse LLM output as JSON array: {trimmed}"
                        );
                        continue;
                    }
                }
            }
        };

        for snippet in snippets {
            let snippet = snippet.trim().to_string();
            if snippet.is_empty() {
                continue;
            }

            let (id, created) = {
                let mut memory = shared.memory.lock().await;
                let (id, created) = memory.remember_if_new_at(
                    CreateMemoryInput {
                        tier,
                        memory_type: MemoryType::Event,
                        summary: snippet,
                        categories: vec![
                            "history-extract".to_string(),
                            format!("date:{date_str}"),
                            format!("session:{}", &turn.session_key),
                        ],
                        source_session: Some(turn.session_key.clone()),
                    },
                    turn.timestamp,
                );
                (id, created)
            };

            if created {
                new_ids.push(id);
            }
        }
    }

    // Embed all new items and save
    let embedded = if !new_ids.is_empty() {
        embed_and_attach_items(shared, &new_ids).await?
    } else {
        0
    };

    {
        let memory = shared.memory.lock().await;
        if let Err(e) = memory.save_json(&shared.memory_path) {
            eprintln!("[vericore] history extract: failed to save memory: {e}");
        }
    }

    eprintln!(
        "[vericore] history extract {date}: {} new memories created, {embedded} embedded",
        new_ids.len()
    );

    Ok(new_ids.len())
}


fn canonical_midterm_memory_path(home_root: &Path) -> PathBuf {
    home_root.join("MIDTERMMEMORY.md")
}

fn midterm_memory_path_candidates(home_root: &Path) -> Vec<PathBuf> {
    vec![
        canonical_midterm_memory_path(home_root),
        home_root.join("midtermmemory.md"),
        home_root.join("DAILYMEMORY.md"),
        home_root.join("dailymemory.md"),
        home_root.join("MEMORY.md"),
        home_root.join("memory.md"),
    ]
}

fn resolve_existing_midterm_memory_path(home_root: &Path) -> Option<PathBuf> {
    midterm_memory_path_candidates(home_root)
        .into_iter()
        .find(|path| path.is_file())
}

fn parse_midterm_covered_through(content: &str) -> Option<NaiveDate> {
    content.lines().find_map(|line| {
        line.strip_prefix("Covered through: ")
            .and_then(|raw| NaiveDate::parse_from_str(raw.trim(), "%Y-%m-%d").ok())
    })
}

fn strip_midterm_wrapper(content: &str) -> String {
    let trimmed = content.trim();
    if !trimmed.starts_with("# MIDTERMMEMORY") {
        return trimmed.to_string();
    }

    let mut lines = trimmed.lines();
    let _ = lines.next();
    let mut body_started = false;
    let mut body_lines: Vec<&str> = Vec::new();
    for line in lines {
        if !body_started {
            if line.starts_with("Last refreshed:")
                || line.starts_with("Covered through:")
                || line.trim().is_empty()
            {
                continue;
            }
            body_started = true;
        }
        body_lines.push(line);
    }
    body_lines.join("
").trim().to_string()
}

fn render_midterm_document(body: &str, covered_through: Option<NaiveDate>) -> String {
    let cleaned_body = body.trim();
    let covered = covered_through
        .map(|date| date.to_string())
        .unwrap_or_else(|| "unknown".to_string());
    let refreshed = Utc::now().to_rfc3339();
    let body = if cleaned_body.is_empty() {
        "## Active threads

- No active mid-term memory yet.".to_string()
    } else {
        cleaned_body.to_string()
    };
    format!(
        "# MIDTERMMEMORY
Last refreshed: {refreshed}
Covered through: {covered}

{body}
"
    )
}

fn parse_history_date_from_path(path: &Path) -> Option<NaiveDate> {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .and_then(|raw| NaiveDate::parse_from_str(raw.trim(), "%Y-%m-%d").ok())
}

fn history_turn_is_meaningful_for_midterm(turn: &HistoryTurn) -> bool {
    if turn.session_key.starts_with("testbus:") {
        return false;
    }
    let user = turn.user_text.trim();
    let assistant = turn.assistant_text.trim();
    if user.is_empty() && assistant.is_empty() {
        return false;
    }
    if user.eq_ignore_ascii_case("testbus ping") && assistant.contains("Pong") {
        return false;
    }
    user.chars().count() + assistant.chars().count() >= 40
}

fn render_midterm_history_chunk(date: NaiveDate, turns: &[HistoryTurn]) -> String {
    let mut out = format!("# Daily history chunk {date}
");
    for turn in turns {
        let rendered_ts = DateTime::from_timestamp(turn.timestamp, 0)
            .map(|dt| dt.to_rfc3339())
            .unwrap_or_else(|| date.to_string());
        out.push_str(&format!(
            "
## {rendered_ts} | session={}
- user: {}
- assistant: {}
",
            turn.session_key,
            truncate_chars(turn.user_text.trim(), HISTORY_MAX_USER_CHARS),
            truncate_chars(turn.assistant_text.trim(), HISTORY_MAX_ASSISTANT_CHARS),
        ));
    }
    out
}

fn split_midterm_history_chunks(date: NaiveDate, turns: &[HistoryTurn]) -> Vec<String> {
    let mut chunks: Vec<String> = Vec::new();
    let mut current: Vec<HistoryTurn> = Vec::new();
    let mut current_chars = 0usize;

    for turn in turns {
        let estimate = turn.user_text.chars().count().min(HISTORY_MAX_USER_CHARS)
            + turn.assistant_text.chars().count().min(HISTORY_MAX_ASSISTANT_CHARS)
            + turn.session_key.chars().count()
            + 96;
        if !current.is_empty()
            && (current_chars + estimate > MIDTERM_CHUNK_MAX_CHARS
                || current.len() >= MIDTERM_MAX_TURNS_PER_CHUNK)
        {
            chunks.push(render_midterm_history_chunk(date, &current));
            current.clear();
            current_chars = 0;
        }
        current.push(turn.clone());
        current_chars = current_chars.saturating_add(estimate);
    }

    if !current.is_empty() {
        chunks.push(render_midterm_history_chunk(date, &current));
    }

    chunks
}

fn extract_midterm_markdown_body(raw: &str) -> String {
    let trimmed = raw.trim();
    let unfenced = if trimmed.starts_with("```") && trimmed.ends_with("```") {
        let mut lines = trimmed.lines();
        let _ = lines.next();
        let mut collected: Vec<&str> = lines.collect();
        if matches!(collected.last(), Some(line) if line.trim() == "```") {
            let _ = collected.pop();
        }
        collected.join("
")
    } else {
        trimmed.to_string()
    };
    strip_midterm_wrapper(&unfenced)
}

async fn rewrite_midterm_memory_chunk(
    client: &GatewayLlmClient,
    system_msg: &ChatMessage,
    current_body: &str,
    chunk: &str,
    date: NaiveDate,
) -> Result<String, String> {
    for attempt in 0..2 {
        let attempt_suffix = if attempt == 0 {
            "Keep section headings. Output only markdown body."
        } else {
            "Keep section headings. Output only markdown body. You must return non-empty markdown content. If the existing body is still mostly right, rewrite it with the new facts integrated instead of omitting content."
        };
        let prompt = format!(
            "Current rolling mid-term memory body:

{}

New daily history chunk:

{}

Rewrite the rolling mid-term memory body so it incorporates the new chunk while staying concise and faithful. {}",
            truncate_chars(current_body.trim(), 12_000),
            chunk,
            attempt_suffix,
        );
        let messages = vec![system_msg.clone(), ChatMessage::user(&prompt)];
        let (result, _usage, _meta) = client
            .chat(&messages, &[])
            .await
            .map_err(|err| format!("midterm refresh LLM call failed for {date}: {err}"))?;
        let text = match result {
            crate::llm::LlmTurnResult::FinalResponse { content } => content,
            crate::llm::LlmTurnResult::ToolCalls { content, .. } => content.unwrap_or_default(),
        };
        let updated = extract_midterm_markdown_body(&text);
        if !updated.trim().is_empty() {
            return Ok(updated.trim().to_string());
        }
        let preview = truncate_chars(text.trim(), 400);
        eprintln!(
            "[vericore] midterm refresh empty output for {} attempt {} preview={} ",
            date,
            attempt + 1,
            serde_json::to_string(&preview).unwrap_or_else(|_| "<preview-encode-failed>".to_string())
        );
    }
    Err(format!("midterm refresh returned empty content for {date}"))
}

async fn run_memory_refresh_midterm(shared: &SharedState) -> Result<Value, String> {
    let home_root = &shared.config.paths.home_root;
    let target_path = canonical_midterm_memory_path(home_root);
    let source_path = resolve_existing_midterm_memory_path(home_root);
    let existing_text = match source_path.as_ref() {
        Some(path) => fs::read_to_string(path)
            .map_err(|err| format!("read {}: {err}", path.display()))?,
        None => String::new(),
    };
    let mut current_body = if existing_text.trim().is_empty() {
        String::new()
    } else {
        strip_midterm_wrapper(&existing_text)
    };
    let covered_through = parse_midterm_covered_through(&existing_text);

    let daily_dir = shared.history_root.join("daily");
    let mut history_files: Vec<(NaiveDate, PathBuf)> = Vec::new();
    match fs::read_dir(&daily_dir) {
        Ok(entries) => {
            for entry in entries {
                let entry = entry.map_err(|err| format!("read_dir {}: {err}", daily_dir.display()))?;
                let path = entry.path();
                if path.extension().and_then(|ext| ext.to_str()) != Some("md") {
                    continue;
                }
                let Some(date) = parse_history_date_from_path(&path) else {
                    continue;
                };
                if let Some(covered) = covered_through
                    && date <= covered
                {
                    continue;
                }
                history_files.push((date, path));
            }
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => {
            return Err(format!("read daily history dir {}: {err}", daily_dir.display()));
        }
    }
    history_files.sort_by_key(|(date, _)| *date);

    let system_msg = ChatMessage::system(
        "You maintain Oruzi's rolling mid-term memory. Rewrite the memory body in concise markdown with stable section headings. Preserve durable active threads, open commitments, recent important developments, risks, and stable background. Integrate new facts from the history chunk. Remove duplicates, stale resolved trivia, and fluff. Do not invent facts. Output markdown body only. No title. No code fences.",
    );
    let mut llm: Option<GatewayLlmClient> = None;
    let mut processed_dates: Vec<String> = Vec::new();
    let mut llm_calls = 0usize;
    let mut meaningful_chunks = 0usize;
    let mut final_covered = covered_through;

    for (date, path) in history_files {
        let content = fs::read_to_string(&path)
            .map_err(|err| format!("read history chunk {}: {err}", path.display()))?;
        let turns: Vec<HistoryTurn> = parse_history_turns(&content)
            .into_iter()
            .filter(history_turn_is_meaningful_for_midterm)
            .collect();
        processed_dates.push(date.to_string());
        final_covered = Some(date);
        if turns.is_empty() {
            continue;
        }

        let chunks = split_midterm_history_chunks(date, &turns);
        if chunks.is_empty() {
            continue;
        }
        if llm.is_none() {
            llm = Some(
                GatewayLlmClient::from_config(
                    &shared.config.llm,
                    CallKind::MemoryRefine,
                    PromptMode::Driver,
                    None,
                )
                .map_err(|err| err.to_string())?,
            );
        }
        let client = llm.as_ref().expect("midterm llm client initialized");

        for chunk in chunks {
            current_body = rewrite_midterm_memory_chunk(client, &system_msg, &current_body, &chunk, date)
                .await?;
            llm_calls = llm_calls.saturating_add(1);
            meaningful_chunks = meaningful_chunks.saturating_add(1);
        }
    }

    if existing_text.trim().is_empty() && current_body.trim().is_empty() && final_covered.is_none() {
        return Ok(json!({
            "path": target_path.display().to_string(),
            "changed": false,
            "covered_through": Value::Null,
            "processed_dates": processed_dates,
            "llm_calls": llm_calls,
            "meaningful_chunks": meaningful_chunks,
        }));
    }

    let final_document = render_midterm_document(&current_body, final_covered);
    let changed = existing_text.trim() != final_document.trim()
        || source_path.as_ref() != Some(&target_path);
    if changed {
        if let Some(parent) = target_path.parent() {
            fs::create_dir_all(parent).map_err(|err| {
                format!("create midterm parent {}: {err}", parent.display())
            })?;
        }
        fs::write(&target_path, final_document.as_bytes())
            .map_err(|err| format!("write {}: {err}", target_path.display()))?;
    }

    Ok(json!({
        "path": target_path.display().to_string(),
        "source_path": source_path.as_ref().map(|path| path.display().to_string()),
        "changed": changed,
        "covered_through": final_covered.map(|date| date.to_string()),
        "processed_dates": processed_dates,
        "llm_calls": llm_calls,
        "meaningful_chunks": meaningful_chunks,
        "body_chars": current_body.chars().count(),
    }))
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

fn build_startup_identity_contract_result(
    agents_available: bool,
    agents_injected: bool,
    soul_available: bool,
    soul_injected: bool,
) -> Value {
    let sources_available =
        vericore_policy::startup_context_policy::startup_identity_sources_available(
            agents_available,
            soul_available,
        );
    let identity_injected = vericore_policy::startup_context_policy::startup_identity_injected(
        agents_injected,
        soul_injected,
    );
    let contract_ok = vericore_policy::startup_context_policy::startup_identity_contract_ok(
        sources_available,
        identity_injected,
    );

    json!({
        "checked": true,
        "contract_ok": contract_ok,
        "agents_available": agents_available,
        "agents_injected": agents_injected,
        "soul_available": soul_available,
        "soul_injected": soul_injected,
    })
}

fn build_heartbeat_sync_contract_result(
    canonical_available: bool,
    mirror_available: bool,
    mirror_matches: bool,
    prompt_uses_file_reference: bool,
    prompt_mentions_legacy_gas: bool,
) -> Value {
    let sources_available = vericore_policy::heartbeat_sync_policy::heartbeat_sources_available(
        canonical_available,
        mirror_available,
    );
    let mirror_consistent =
        vericore_policy::heartbeat_sync_policy::heartbeat_mirror_consistent(mirror_matches);
    let prompt_ok = vericore_policy::heartbeat_sync_policy::heartbeat_prompt_ok(
        prompt_uses_file_reference,
        prompt_mentions_legacy_gas,
    );
    let contract_ok = vericore_policy::heartbeat_sync_policy::heartbeat_contract_ok(
        sources_available,
        mirror_consistent,
        prompt_ok,
    );

    json!({
        "checked": true,
        "contract_ok": contract_ok,
        "canonical_available": canonical_available,
        "mirror_available": mirror_available,
        "mirror_matches": mirror_matches,
        "prompt_uses_file_reference": prompt_uses_file_reference,
        "prompt_mentions_legacy_gas": prompt_mentions_legacy_gas,
    })
}

fn build_heartbeat_context_contract_result(
    recent_turn_count: usize,
    recent_turn_limit: usize,
    reserved_candidate_present: bool,
    reserved_included: bool,
    content_heartbeat_candidate_present: bool,
    content_heartbeat_included: bool,
    noop_heartbeat_in_recent_turns: bool,
    heartbeat_trace_count: usize,
    heartbeat_trace_limit: usize,
    heartbeat_trace_oldest_first: bool,
    heartbeat_trace_only_heartbeat_entries: bool,
) -> Value {
    let recent_turns_ok = vericore_policy::heartbeat_context_policy::recent_turns_within_limit(
        recent_turn_count,
        recent_turn_limit,
    );
    let reserved_main_ok =
        vericore_policy::heartbeat_context_policy::reserved_main_turn_preserved(
            reserved_candidate_present,
            reserved_included,
        );
    let content_heartbeat_ok =
        vericore_policy::heartbeat_context_policy::content_heartbeat_preserved(
            content_heartbeat_candidate_present,
            content_heartbeat_included,
        );
    let noop_recent_turns_ok =
        vericore_policy::heartbeat_context_policy::noop_heartbeat_excluded_from_recent_turns(
            noop_heartbeat_in_recent_turns,
        );
    let heartbeat_trace_ok = vericore_policy::heartbeat_context_policy::heartbeat_trace_ok(
        heartbeat_trace_count,
        heartbeat_trace_limit,
        heartbeat_trace_oldest_first,
    );
    let heartbeat_trace_separate =
        vericore_policy::heartbeat_context_policy::heartbeat_trace_separate(
            heartbeat_trace_only_heartbeat_entries,
        );
    let contract_ok = vericore_policy::heartbeat_context_policy::heartbeat_context_contract_ok(
        recent_turns_ok,
        reserved_main_ok,
        content_heartbeat_ok,
        noop_recent_turns_ok,
        heartbeat_trace_ok,
        heartbeat_trace_separate,
    );

    json!({
        "checked": true,
        "contract_ok": contract_ok,
        "recent_turn_count": recent_turn_count,
        "recent_turn_limit": recent_turn_limit,
        "reserved_candidate_present": reserved_candidate_present,
        "reserved_included": reserved_included,
        "content_heartbeat_candidate_present": content_heartbeat_candidate_present,
        "content_heartbeat_included": content_heartbeat_included,
        "noop_heartbeat_in_recent_turns": noop_heartbeat_in_recent_turns,
        "heartbeat_trace_count": heartbeat_trace_count,
        "heartbeat_trace_limit": heartbeat_trace_limit,
        "heartbeat_trace_oldest_first": heartbeat_trace_oldest_first,
        "heartbeat_trace_only_heartbeat_entries": heartbeat_trace_only_heartbeat_entries,
    })
}

fn build_energy_contract_result(
    noop_heartbeat_count: usize,
    acted_heartbeat_count: usize,
    conversation_burst_count: usize,
    conversation_gap_ms: usize,
    energy_value: i64,
) -> Value {
    let expected_energy = match (
        i128::try_from(noop_heartbeat_count),
        i128::try_from(acted_heartbeat_count),
        i128::try_from(conversation_burst_count),
    ) {
        (Ok(noop), Ok(acted), Ok(conversation)) => {
            i64::try_from(noop - acted - conversation).ok()
        }
        _ => None,
    };
    let gap_locked = vericore_policy::energy_policy::conversation_gap_locked(
        conversation_gap_ms,
    );
    let energy_matches = expected_energy
        .map(|expected| {
            vericore_policy::energy_policy::energy_value_matches(
                expected,
                energy_value,
            )
        })
        .unwrap_or(false);
    let contract_ok = vericore_policy::energy_policy::energy_contract_ok(
        gap_locked,
        energy_matches,
    );

    json!({
        "checked": true,
        "contract_ok": contract_ok,
        "noop_heartbeat_count": noop_heartbeat_count,
        "acted_heartbeat_count": acted_heartbeat_count,
        "conversation_burst_count": conversation_burst_count,
        "conversation_gap_ms": conversation_gap_ms,
        "energy_value": energy_value,
        "expected_energy": expected_energy,
        "gap_locked": gap_locked,
        "energy_matches": energy_matches,
    })
}

fn build_affect_contract_result(
    gamma: usize,
    prev: Option<&[i64]>,
    obs: &[i64],
    result: &[i64],
) -> Value {
    let gamma_locked = vericore_policy::affect_policy::affect_gamma_locked(gamma);

    let mut all_bounded = true;
    let mut fold_matches = true;
    let mut expected: Vec<i64> = Vec::with_capacity(7);

    for i in 0..7 {
        let p = prev.map(|values| values[i]);
        let o = obs[i];
        let r = result[i];

        if p.is_some_and(|value| !vericore_policy::affect_policy::affect_dimension_bounded(value))
            || !vericore_policy::affect_policy::affect_dimension_bounded(o)
            || !vericore_policy::affect_policy::affect_dimension_bounded(r)
        {
            all_bounded = false;
        }

        let o_clamped = o.max(-100).min(100);
        let e = if let Some(p_value) = p {
            let p_clamped = p_value.max(-100).min(100);
            vericore_policy::affect_policy::affect_fold_step(
                p_clamped,
                o_clamped,
                gamma as i64,
            )
        } else {
            o_clamped
        };
        expected.push(e);

        if !vericore_policy::affect_policy::affect_fold_matches(e, r) {
            fold_matches = false;
        }
    }

    let contract_ok = vericore_policy::affect_policy::affect_contract_ok(
        gamma_locked,
        all_bounded,
        fold_matches,
    );

    json!({
        "checked": true,
        "contract_ok": contract_ok,
        "gamma_locked": gamma_locked,
        "all_bounded": all_bounded,
        "fold_matches": fold_matches,
        "affect_gamma": gamma,
        "affect_prev": prev,
        "affect_obs": obs,
        "affect_result": result,
        "expected_result": expected,
    })
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

#[derive(Debug, Clone)]
struct DriverAuditWriteResult {
    daily_file: String,
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
    fs::create_dir_all(root.join("audit")).map_err(|e| {
        format!(
            "failed to create history audit dir {}: {e}",
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

fn append_driver_audit(
    root: &Path,
    timestamp: i64,
    entry: &Value,
) -> Result<DriverAuditWriteResult, String> {
    ensure_history_dirs(root)?;

    let day = ts_to_utc(timestamp).format("%Y-%m-%d").to_string();
    let audit_path = root.join("audit").join(format!("{day}.jsonl"));

    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&audit_path)
        .map_err(|e| format!("open driver audit {}: {e}", audit_path.display()))?;

    let mut encoded = serde_json::to_string(entry)
        .map_err(|e| format!("encode driver audit {}: {e}", audit_path.display()))?;
    encoded.push('\n');
    file.write_all(encoded.as_bytes())
        .map_err(|e| format!("append driver audit {}: {e}", audit_path.display()))?;

    Ok(DriverAuditWriteResult {
        daily_file: audit_path.display().to_string(),
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

fn count_jsonl_files(dir: &Path) -> usize {
    let Ok(entries) = fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .filter_map(|entry| entry.file_name().to_str().map(str::to_string))
        .filter(|name| name.ends_with(".jsonl"))
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

fn canonicalize_write_target_under_home(path: &Path, home_root: &Path) -> Result<PathBuf, String> {
    if !path.is_absolute() {
        return Err(format!("target path must be absolute: {}", path.display()));
    }

    let file_name = path
        .file_name()
        .ok_or_else(|| format!("target path must include file name: {}", path.display()))?;
    let parent = path
        .parent()
        .ok_or_else(|| format!("target path must have parent directory: {}", path.display()))?;
    let canonical_parent = fs::canonicalize(parent).map_err(|err| {
        format!(
            "failed to canonicalize target parent '{}': {err}",
            parent.display()
        )
    })?;
    let canonical = canonical_parent.join(file_name);

    if !canonical.starts_with(home_root) {
        return Err(format!(
            "target path outside home root {}: {}",
            home_root.display(),
            canonical.display()
        ));
    }

    Ok(canonical)
}

fn read_target_path_from_meta(meta_path: &Path) -> Result<String, String> {
    let raw = fs::read(meta_path)
        .map_err(|e| format!("failed to read meta {}: {e}", meta_path.display()))?;
    let parsed: Value = serde_json::from_slice(&raw)
        .map_err(|e| format!("failed to parse meta {}: {e}", meta_path.display()))?;

    parsed
        .get("target_path")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| format!("meta {} missing target_path", meta_path.display()))
}

fn resolve_pending_selector_id(pending_dir: &Path, selector: &str) -> Result<String, String> {
    let trimmed = selector.trim();
    if trimmed.is_empty() {
        return Err("artifact id cannot be empty".to_string());
    }

    if let Ok(index) = trimmed.parse::<usize>() {
        if index == 0 {
            return Err("artifact index must be >= 1".to_string());
        }
        let items = collect_pending_artifacts(pending_dir)?;
        let Some(item) = items.get(index - 1) else {
            return Err(format!(
                "artifact index {} out of range (pending count={})",
                index,
                items.len()
            ));
        };
        let id = item
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "pending item missing id".to_string())?;
        return Ok(id.to_string());
    }

    Ok(trimmed.to_string())
}

fn resolve_pending_artifact_path(pending_dir: &Path, artifact_id: &str) -> Result<PathBuf, String> {
    let id = artifact_id.trim();
    if id.is_empty() {
        return Err("artifact id cannot be empty".to_string());
    }
    if id.contains('/') || id.contains('\\') {
        return Err("artifact id must not contain path separators".to_string());
    }

    let direct = pending_dir.join(id);
    if direct.is_file() {
        return Ok(direct);
    }

    let with_txt = pending_dir.join(format!("{id}.txt"));
    if with_txt.is_file() {
        return Ok(with_txt);
    }

    let entries = fs::read_dir(pending_dir)
        .map_err(|e| format!("failed to read pending dir {}: {e}", pending_dir.display()))?;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        if name.ends_with(".meta.json") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if stem == id {
            return Ok(path);
        }
    }

    Err(format!(
        "artifact '{}' not found in {}",
        id,
        pending_dir.display()
    ))
}

fn archive_artifact_file(
    source_path: &Path,
    dest_dir: &Path,
    prefix: &str,
) -> Result<PathBuf, String> {
    let file_name = source_path
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| format!("invalid artifact filename: {}", source_path.display()))?;
    let mut destination = dest_dir.join(format!("{}-{}", prefix, file_name));
    if destination.exists() {
        destination = dest_dir.join(format!("{}-{}-{}", prefix, now_ts(), file_name));
    }
    fs::rename(source_path, &destination).map_err(|e| {
        format!(
            "failed to move {} -> {}: {e}",
            source_path.display(),
            destination.display()
        )
    })?;
    Ok(destination)
}

fn archive_meta_file(meta_path: &Path, dest_dir: &Path, prefix: &str) -> Result<PathBuf, String> {
    if !meta_path.exists() {
        return Err(format!("meta not found: {}", meta_path.display()));
    }
    let file_name = meta_path
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| format!("invalid meta filename: {}", meta_path.display()))?;
    let mut destination = dest_dir.join(format!("{}-{}", prefix, file_name));
    if destination.exists() {
        destination = dest_dir.join(format!("{}-{}-{}", prefix, now_ts(), file_name));
    }
    fs::rename(meta_path, &destination).map_err(|e| {
        format!(
            "failed to move {} -> {}: {e}",
            meta_path.display(),
            destination.display()
        )
    })?;
    Ok(destination)
}

fn append_mindlock_audit_event(
    audit_dir: &Path,
    event: &str,
    id: &str,
    source_path: &Path,
    target_path: Option<&Path>,
    reason: &str,
    hash: Option<&str>,
) -> Result<(), String> {
    let audit_path = audit_dir.join("events.jsonl");
    let mut line = json!({
        "ts": now_ts(),
        "event": event,
        "id": id,
        "source_path": source_path.display().to_string(),
        "target_path": target_path.map(|p| p.display().to_string()),
        "reason": reason,
    });
    if let Some(h) = hash {
        line["hash"] = Value::String(h.to_string());
    }

    let mut buf = serde_json::to_string(&line)
        .map_err(|e| format!("failed to serialize mindlock audit line: {e}"))?;
    buf.push('\n');

    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&audit_path)
        .map_err(|e| format!("failed to open audit log {}: {e}", audit_path.display()))?;
    file.write_all(buf.as_bytes())
        .map_err(|e| format!("failed to write audit log {}: {e}", audit_path.display()))
}

fn normalize_mindlock_box(raw: Option<&str>) -> Option<&'static str> {
    let value = raw?.trim().to_ascii_lowercase();
    match value.as_str() {
        "in" => Some("in"),
        "out" => Some("out"),
        "pending" | "pending-zar" => Some("pending"),
        "rejected" => Some("rejected"),
        _ => None,
    }
}

fn count_mindlock_items(dir: &Path) -> Result<usize, String> {
    fs::create_dir_all(dir)
        .map_err(|e| format!("failed to create mindlock dir {}: {e}", dir.display()))?;

    let entries = fs::read_dir(dir)
        .map_err(|e| format!("failed to read mindlock dir {}: {e}", dir.display()))?;
    let mut count = 0usize;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        if name.ends_with(".meta.json") {
            continue;
        }
        count = count.saturating_add(1);
    }
    Ok(count)
}

fn collect_mindlock_artifacts(dir: &Path, limit: usize) -> Result<Vec<Value>, String> {
    fs::create_dir_all(dir)
        .map_err(|e| format!("failed to create mindlock dir {}: {e}", dir.display()))?;

    let mut items: Vec<Value> = Vec::new();
    let entries = fs::read_dir(dir)
        .map_err(|e| format!("failed to read mindlock dir {}: {e}", dir.display()))?;

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }

        let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        if name.ends_with(".meta.json") {
            continue;
        }

        let metadata = match fs::metadata(&path) {
            Ok(meta) => meta,
            Err(_) => continue,
        };

        let modified_ts = metadata
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        let id = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(name)
            .to_string();

        let meta_path = path.with_extension("meta.json");
        let meta_value = fs::read(&meta_path)
            .ok()
            .and_then(|raw| serde_json::from_slice::<Value>(&raw).ok());
        let target_path = meta_value
            .as_ref()
            .and_then(|v| v.get("target_path"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let reason = meta_value
            .as_ref()
            .and_then(|v| v.get("reason"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let reviewer_assessment = meta_value
            .as_ref()
            .and_then(|v| v.get("reviewer_assessment").cloned());

        let mut item_json = json!({
            "id": id,
            "name": name,
            "path": path.display().to_string(),
            "size_bytes": metadata.len(),
            "modified_ts": modified_ts,
            "target_path": target_path,
            "reason": reason,
        });
        if let Some(assessment) = reviewer_assessment {
            item_json["reviewer_assessment"] = assessment;
        }
        items.push(item_json);
    }

    items.sort_by(|a, b| {
        let am = a.get("modified_ts").and_then(|v| v.as_i64()).unwrap_or(0);
        let bm = b.get("modified_ts").and_then(|v| v.as_i64()).unwrap_or(0);
        bm.cmp(&am)
    });

    if items.len() > limit {
        items.truncate(limit);
    }

    Ok(items)
}

fn collect_pending_artifacts(pending_dir: &Path) -> Result<Vec<Value>, String> {
    collect_mindlock_artifacts(pending_dir, usize::MAX)
}

fn build_mindlock_pending_view(pending_dir: &Path, artifact_id: &str) -> Result<Value, String> {
    fs::create_dir_all(pending_dir).map_err(|e| {
        format!(
            "failed to create mindlock pending dir {}: {e}",
            pending_dir.display()
        )
    })?;

    let resolved_selector = resolve_pending_selector_id(pending_dir, artifact_id)?;
    let artifact_path = resolve_pending_artifact_path(pending_dir, &resolved_selector)?;
    let metadata = fs::metadata(&artifact_path).map_err(|e| {
        format!(
            "failed to read mindlock artifact metadata {}: {e}",
            artifact_path.display()
        )
    })?;
    let modified_ts = metadata
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    let name = artifact_path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_string();
    let id = artifact_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(name.as_str())
        .to_string();

    let meta_path = artifact_path.with_extension("meta.json");
    let meta_value = fs::read(&meta_path)
        .ok()
        .and_then(|raw| serde_json::from_slice::<Value>(&raw).ok());
    let target_path = meta_value
        .as_ref()
        .and_then(|v| v.get("target_path"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let reason = meta_value
        .as_ref()
        .and_then(|v| v.get("reason"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let reviewer_assessment = meta_value
        .as_ref()
        .and_then(|v| v.get("reviewer_assessment").cloned());

    let (preview, preview_truncated, preview_binary) =
        render_mindlock_preview(&artifact_path, MINDLOCK_VIEW_PREVIEW_MAX_CHARS);

    let mut result = json!({
        "id": id,
        "name": name,
        "path": artifact_path.display().to_string(),
        "stage": "pending",
        "size_bytes": metadata.len(),
        "modified_ts": modified_ts,
        "target_path": target_path,
        "reason": reason,
        "preview": preview,
        "preview_truncated": preview_truncated,
        "preview_binary": preview_binary,
    });
    if let Some(assessment) = reviewer_assessment {
        result["reviewer_assessment"] = assessment;
    }
    Ok(result)
}

fn render_mindlock_preview(path: &Path, max_chars: usize) -> (String, bool, bool) {
    let Ok(bytes) = fs::read(path) else {
        return (
            format!("(unable to read artifact {})", path.display()),
            false,
            false,
        );
    };
    if bytes.contains(&0) {
        return (
            "(binary artifact; preview suppressed)".to_string(),
            false,
            true,
        );
    }

    let text = String::from_utf8_lossy(&bytes);
    let total_chars = text.chars().count();
    let preview = truncate_chars(&text, max_chars);
    (preview, total_chars > max_chars, false)
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
            lines.push(format_memory_line(&hit.item));
        }
    }

    for item in memory.hot_items(tier, MEMORY_HOT_MIN_REINFORCEMENT, MEMORY_HOT_LIMIT) {
        if seen.insert(item.id) {
            lines.push(format_memory_line(&item));
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

fn format_memory_line(item: &adclaw_memory::MemoryItem) -> String {
    // Use full summary — items should already be concise from LLM extraction.
    // No hard char clip; trust the memory quality from sleep refinement.
    let mut line = item.summary.trim().to_string();

    // Source pointer: lets the agent know when/where the memory came from.
    let source = match (&item.source_date, &item.source_session) {
        (Some(date), Some(session)) => {
            // Show date + channel prefix only (not full session key which can be verbose)
            let channel = session.split(':').next().unwrap_or(session.as_str());
            Some(format!("{date} {channel}"))
        }
        (Some(date), None) => Some(date.clone()),
        (None, Some(session)) => {
            let channel = session.split(':').next().unwrap_or(session.as_str());
            Some(channel.to_string())
        }
        (None, None) => None,
    };

    if let Some(src) = source {
        line.push_str(" [");
        line.push_str(&src);
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
        eprintln!("[vericore] imported {imported} bootstrap memory items, embedded {embedded}");
    }

    Ok(())
}

fn bootstrap_markdown_files(home_root: &Path, history_root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let canonical_midterm = home_root.join("MIDTERMMEMORY.md");
    let canonical_midterm_alt = home_root.join("midtermmemory.md");
    let legacy_midterm = home_root.join("DAILYMEMORY.md");
    let legacy_midterm_alt = home_root.join("dailymemory.md");
    let super_legacy_midterm = home_root.join("MEMORY.md");
    let super_legacy_midterm_alt = home_root.join("memory.md");
    if canonical_midterm.is_file() {
        files.push(canonical_midterm);
    } else if canonical_midterm_alt.is_file() {
        files.push(canonical_midterm_alt);
    } else if legacy_midterm.is_file() {
        files.push(legacy_midterm);
    } else if legacy_midterm_alt.is_file() {
        files.push(legacy_midterm_alt);
    } else if super_legacy_midterm.is_file() {
        files.push(super_legacy_midterm);
    } else {
        files.push(super_legacy_midterm_alt);
    }
    push_markdown_files(&home_root.join("memory"), &mut files, false);
    // Private long-term notes that should remain private but be searchable in DM/family contexts.
    push_markdown_files(
        &home_root.join("private").join("memories"),
        &mut files,
        true,
    );
    push_markdown_files(
        &home_root.join("private").join("CzechStudy"),
        &mut files,
        true,
    );
    push_csv_files(
        &home_root.join("private").join("CzechStudy"),
        &mut files,
        true,
    );
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

fn push_csv_files(dir: &Path, out: &mut Vec<PathBuf>, recursive: bool) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file()
            && path
                .extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| ext.eq_ignore_ascii_case("csv"))
        {
            out.push(path);
            continue;
        }
        if recursive && path.is_dir() {
            push_csv_files(&path, out, true);
        }
    }
}

fn extract_bootstrap_entries(path: &Path) -> Result<Vec<BootstrapEntry>, String> {
    let extension = path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase())
        .unwrap_or_default();
    if extension == "csv" {
        return extract_bootstrap_entries_csv(path);
    }
    extract_bootstrap_entries_markdown(path)
}

fn extract_bootstrap_entries_markdown(path: &Path) -> Result<Vec<BootstrapEntry>, String> {
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

fn extract_bootstrap_entries_csv(path: &Path) -> Result<Vec<BootstrapEntry>, String> {
    let text = fs::read_to_string(path)
        .map_err(|e| format!("read bootstrap csv file {}: {e}", path.display()))?;

    let default_day = infer_day_from_filename(path);
    let mut current_ts = default_day
        .as_deref()
        .and_then(day_to_ts)
        .unwrap_or_else(now_ts);
    let mut current_day = default_day.clone();

    let mut lines = text.lines().map(str::trim).filter(|line| !line.is_empty());
    let header_line = lines.next().unwrap_or("");
    let headers: Vec<String> = header_line
        .split(',')
        .map(|h| h.trim().trim_matches('"').to_string())
        .collect();

    let mut entries = Vec::new();
    let mut seen = HashSet::new();

    for raw in lines {
        if raw.starts_with('#') {
            continue;
        }

        if let Some(ts) = parse_ts_from_text(raw) {
            current_ts = ts;
            current_day = Some(ts_to_utc(ts).date_naive().format("%Y-%m-%d").to_string());
        } else if let Some(day) = find_day_in_text(raw) {
            current_ts = day_to_ts(&day).unwrap_or(current_ts);
            current_day = Some(day);
        }

        let cols: Vec<String> = raw
            .split(',')
            .map(|c| c.trim().trim_matches('"').to_string())
            .collect();

        let candidate = if !headers.is_empty() && headers.len() == cols.len() {
            headers
                .iter()
                .zip(cols.iter())
                .filter(|(_, v)| !v.is_empty())
                .take(6)
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join("; ")
        } else {
            raw.to_string()
        };

        if candidate.len() < 18 {
            continue;
        }

        let normalized = candidate.to_lowercase();
        if !seen.insert(normalized) {
            continue;
        }

        let mut categories = vec!["bootstrap".to_string(), "csv".to_string()];
        let path_str = path.display().to_string();
        if path_str.contains("CzechStudy") {
            categories.push("czech-study".to_string());
        }
        if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
            categories.push(format!("file:{stem}"));
        }
        if let Some(day) = &current_day {
            categories.push(format!("date:{day}"));
        }

        entries.push(BootstrapEntry {
            summary: truncate_chars(&candidate, BOOTSTRAP_MAX_LINE_CHARS),
            ts: current_ts,
            source_date: current_day.clone(),
            memory_type: MemoryType::ProjectState,
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
    use super::{
        append_driver_audit, build_affect_contract_result,
        build_heartbeat_context_contract_result,
        build_energy_contract_result,
        build_heartbeat_sync_contract_result, build_startup_identity_contract_result,
        trim_history,
    };
    use crate::llm::{ChatMessage, ToolCall};
    use serde_json::json;
    use std::fs;
    use std::path::PathBuf;

    #[test]
    fn startup_identity_contract_passes_when_agents_and_soul_are_present() {
        let value = build_startup_identity_contract_result(true, true, true, true);
        assert_eq!(value["checked"], json!(true));
        assert_eq!(value["contract_ok"], json!(true));
    }

    #[test]
    fn startup_identity_contract_fails_when_soul_is_not_injected() {
        let value = build_startup_identity_contract_result(true, true, true, false);
        assert_eq!(value["contract_ok"], json!(false));
    }

    #[test]
    fn heartbeat_sync_contract_passes_when_files_match_and_prompt_follows_file() {
        let value = build_heartbeat_sync_contract_result(true, true, true, true, false);
        assert_eq!(value["checked"], json!(true));
        assert_eq!(value["contract_ok"], json!(true));
    }

    #[test]
    fn heartbeat_sync_contract_fails_when_prompt_mentions_legacy_gas() {
        let value = build_heartbeat_sync_contract_result(true, true, true, true, true);
        assert_eq!(value["contract_ok"], json!(false));
    }

    #[test]
    fn heartbeat_context_contract_passes_for_bounded_recent_turns_and_trace() {
        let value = build_heartbeat_context_contract_result(
            5, 5, true, true, true, true, false, 12, 32, true, true,
        );
        assert_eq!(value["checked"], json!(true));
        assert_eq!(value["contract_ok"], json!(true));
    }

    #[test]
    fn heartbeat_context_contract_fails_when_noop_heartbeat_enters_recent_turns() {
        let value = build_heartbeat_context_contract_result(
            5, 5, true, true, true, true, true, 12, 32, true, true,
        );
        assert_eq!(value["contract_ok"], json!(false));
    }

    #[test]
    fn energy_contract_passes_for_matching_counts_and_gap() {
        let value = build_energy_contract_result(3, 1, 3, 30 * 60 * 1000, -1);
        assert_eq!(value["checked"], json!(true));
        assert_eq!(value["contract_ok"], json!(true));
        assert_eq!(value["expected_energy"], json!(-1));
    }

    #[test]
    fn energy_contract_fails_for_wrong_gap_or_value() {
        let wrong_gap = build_energy_contract_result(3, 1, 3, 15 * 60 * 1000, -1);
        assert_eq!(wrong_gap["contract_ok"], json!(false));

        let wrong_value = build_energy_contract_result(3, 1, 3, 30 * 60 * 1000, 0);
        assert_eq!(wrong_value["contract_ok"], json!(false));
    }

    #[test]
    fn affect_contract_passes_for_correct_fold() {
        let prev = vec![50; 7];
        let obs = vec![30; 7];
        let result = vec![44; 7];
        let value = build_affect_contract_result(69, Some(&prev), &obs, &result);
        assert_eq!(value["checked"], json!(true));
        assert_eq!(value["contract_ok"], json!(true));
        assert_eq!(value["gamma_locked"], json!(true));
    }

    #[test]
    fn affect_contract_fails_for_wrong_gamma() {
        let prev = vec![50; 7];
        let obs = vec![30; 7];
        let result = vec![44; 7];
        let value = build_affect_contract_result(70, Some(&prev), &obs, &result);
        assert_eq!(value["contract_ok"], json!(false));
        assert_eq!(value["gamma_locked"], json!(false));
    }

    #[test]
    fn affect_contract_fails_for_wrong_fold() {
        let prev = vec![50; 7];
        let obs = vec![30; 7];
        let mut result = vec![44; 7];
        result[6] = 99;
        let value = build_affect_contract_result(69, Some(&prev), &obs, &result);
        assert_eq!(value["contract_ok"], json!(false));
        assert_eq!(value["fold_matches"], json!(false));
    }

    #[test]
    fn affect_contract_accepts_seed_step_without_previous_gestalt() {
        let obs = vec![50; 7];
        let result = vec![50; 7];
        let value = build_affect_contract_result(69, None, &obs, &result);
        assert_eq!(value["checked"], json!(true));
        assert_eq!(value["contract_ok"], json!(true));
        assert_eq!(value["affect_prev"], json!(null));
        assert_eq!(value["expected_result"], json!(obs));
    }

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

    #[test]
    fn append_driver_audit_writes_jsonl_entry() {
        let root = unique_temp_dir("vericore-driver-audit-test");
        fs::create_dir_all(&root).unwrap();

        let entry = json!({
            "kind": "driver_turn",
            "session_key": "agent:main:main",
            "user_text": "ping",
            "assistant_text": "pong",
        });
        let result = append_driver_audit(&root, 1_741_686_400, &entry).unwrap();
        let content = fs::read_to_string(&result.daily_file).unwrap();

        assert!(content.contains("\"kind\":\"driver_turn\""));
        assert!(content.contains("\"assistant_text\":\"pong\""));

        let _ = fs::remove_dir_all(root);
    }

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "{prefix}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }
}
