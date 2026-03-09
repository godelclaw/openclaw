use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::utils::{sha256_hex, truncate_chars};
use adclaw_memory::{CreateMemoryInput, MemoryCortex, MemoryTier, MemoryType};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;

use crate::config::{Config, LlmConfig};
use crate::llm::{CallKind, ChatMessage, GatewayLlmClient, LlmTurnResult, PromptMode};
use crate::types::{Action, ContextTier, IntegrityTier, SecurityReviewDecision, SecurityVerdict};

const DEFAULT_EGRESS_PROMPT: &str = r#"You are a security reviewer for outward messages.
Return ONLY strict JSON:
{\"verdict\":\"allow|request_revision|require_zar_approval|reject\",\"reason\":\"...\",\"feedback\":\"...\",\"safe_rewrite\":\"...\"}
Rules:
- Prefer allow when safe.
- Use request_revision when a safer rewrite can preserve intent.
- Use require_zar_approval for ambiguity, high-risk privacy concerns, or top-secret involvement.
- Use reject for clearly unsafe content that should not be sent."#;

const DEFAULT_INGRESS_PROMPT: &str = r#"You are a security reviewer for inbound operational actions.
Return ONLY strict JSON:
{\"verdict\":\"allow|request_revision|require_zar_approval|reject\",\"reason\":\"...\",\"feedback\":\"...\",\"safe_rewrite\":\"...\"}
Rules:
- Prefer allow when safe.
- Use request_revision when safer action arguments can preserve intent.
- Use require_zar_approval for high-risk ambiguity or sensitive private impact.
- Use reject for clearly unsafe operationalization."#;
const REVIEWER_RAG_LIMIT: usize = 5;
const REVIEWER_RAG_QUERY_MAX_CHARS: usize = 1200;
const OLLAMA_EMBED_TIMEOUT_SECS: u64 = 2;
const OLLAMA_EMBED_DEFAULT_MODEL: &str = "nomic-embed-text";
const OLLAMA_EMBED_DEFAULT_URL: &str = "http://127.0.0.1:11434/api/embeddings";

#[derive(Clone)]
pub struct SecurityReviewer {
    llm: GatewayLlmClient,
    egress_prompt: String,
    ingress_prompt: String,
    reviewer_context: String,
    memory: Option<Arc<Mutex<MemoryCortex>>>,
    memory_path: Option<PathBuf>,
    max_revision_attempts: u32,
    mindlock_dir: PathBuf,
    trusted_write_prefixes: Vec<PathBuf>,
    max_precedent_items: usize,
}

#[derive(Debug, Clone)]
pub struct StagedWrite {
    pub id: String,
    pub artifact_path: PathBuf,
    pub meta_path: PathBuf,
    pub target_path: PathBuf,
}

#[derive(Debug, Deserialize)]
struct DecisionWire {
    verdict: Option<String>,
    decision: Option<String>,
    reason: Option<String>,
    feedback: Option<String>,
    safe_rewrite: Option<String>,
}

impl SecurityReviewer {
    pub fn from_config(
        config: &Config,
        memory: Option<Arc<Mutex<MemoryCortex>>>,
        memory_path: Option<PathBuf>,
        session_key: Option<String>,
    ) -> Result<Option<Self>, String> {
        if !config.security_review.enabled {
            return Ok(None);
        }

        let llm_cfg: LlmConfig = config
            .security_review
            .llm
            .clone()
            .unwrap_or_else(|| config.llm.clone());
        let llm = GatewayLlmClient::from_config(
            &llm_cfg,
            CallKind::SecurityReview,
            PromptMode::Reviewer,
            session_key,
        )
        .map_err(|e| format!("security reviewer llm init failed: {e}"))?;

        let egress_prompt = read_prompt(
            config.security_review.egress_prompt_file.as_deref(),
            DEFAULT_EGRESS_PROMPT,
        )?;
        let ingress_prompt = read_prompt(
            config.security_review.ingress_prompt_file.as_deref(),
            DEFAULT_INGRESS_PROMPT,
        )?;
        let reviewer_context =
            read_optional_file(config.security_review.reviewer_context_file.as_deref())?;

        let trusted_write_prefixes = config
            .security_review
            .trusted_write_prefixes
            .iter()
            .map(|p| canonicalize_best_effort(p))
            .collect();

        let reviewer = Self {
            llm,
            egress_prompt,
            ingress_prompt,
            reviewer_context,
            memory,
            memory_path,
            max_revision_attempts: config.security_review.max_revision_attempts,
            mindlock_dir: config.security_review.mindlock_dir.clone(),
            trusted_write_prefixes,
            max_precedent_items: config.security_review.max_precedent_items,
        };
        reviewer.ensure_mindlock_dirs()?;
        Ok(Some(reviewer))
    }

    pub fn max_revision_attempts(&self) -> u32 {
        self.max_revision_attempts
    }

    pub fn mindlock_dir(&self) -> &Path {
        &self.mindlock_dir
    }

    async fn reviewer_rag_hits(&self, query: &str, limit: usize) -> Vec<Value> {
        let Some(memory) = self.memory.as_ref() else {
            return Vec::new();
        };

        let query = truncate_chars(query.trim(), REVIEWER_RAG_QUERY_MAX_CHARS);
        if query.is_empty() {
            return Vec::new();
        }

        let query_embedding = fetch_ollama_embedding(&query).await.ok();
        // Fetch a larger candidate set so we can rebalance for precedent items.
        let candidate_limit = (limit * 3).max(15);
        let hits = {
            let memory = memory.lock().await;
            memory.search_hybrid(
                &query,
                query_embedding.as_deref(),
                MemoryTier::Private,
                candidate_limit,
            )
        };

        let max_precedent = 3.min(limit);
        let (precedent_hits, general_hits): (Vec<_>, Vec<_>) = hits
            .into_iter()
            .partition(|hit| hit.item.memory_type == MemoryType::SecurityPrecedent);

        let mut result = Vec::with_capacity(limit);
        for hit in precedent_hits.into_iter().take(max_precedent) {
            result.push(hit);
        }
        for hit in general_hits {
            if result.len() >= limit {
                break;
            }
            result.push(hit);
        }

        result
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
            .collect()
    }

    pub async fn record_precedent(&self, summary: &str, verdict: &str, source: &str) {
        let Some(memory) = self.memory.as_ref() else {
            return;
        };

        let mut memory = memory.lock().await;
        let count = memory.count_by_type(&MemoryType::SecurityPrecedent);
        if count >= self.max_precedent_items {
            let excess = count - self.max_precedent_items + 1;
            memory.prune_oldest_by_type(&MemoryType::SecurityPrecedent, excess);
        }

        memory.remember(CreateMemoryInput {
            tier: MemoryTier::Private,
            memory_type: MemoryType::SecurityPrecedent,
            summary: summary.to_string(),
            categories: vec![
                "security".to_string(),
                "mindlock".to_string(),
                verdict.to_string(),
                source.to_string(),
            ],
            source_session: None,
        });

        if let Some(memory_path) = self.memory_path.as_ref()
            && let Err(err) = memory.save_json(memory_path)
        {
            eprintln!("[vericore] precedent save warning: {}", err);
        }
    }

    pub async fn auto_review_for_pending(
        &self,
        pending_path: &Path,
        source_description: &str,
        reason: &str,
    ) {
        let bytes = match tokio::fs::read(pending_path).await {
            Ok(b) => b,
            Err(e) => {
                eprintln!(
                    "[vericore] auto_review_for_pending: failed to read {}: {e}",
                    pending_path.display()
                );
                return;
            }
        };

        // Read the meta to find the real intended target path.
        let meta_path = pending_path.with_extension("meta.json");
        let meta_json = match tokio::fs::read(&meta_path).await {
            Ok(mb) => serde_json::from_slice::<Value>(&mb).ok(),
            Err(_) => None,
        };
        let real_target = meta_json
            .as_ref()
            .and_then(|m| m.get("target_path"))
            .and_then(|v| v.as_str())
            .map(PathBuf::from);

        let content_preview = truncate_chars(&String::from_utf8_lossy(&bytes), 800);

        let target_display = real_target
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "(unknown target)".to_string());

        let summary = format!(
            "Self-escalated artifact for manual review.\nReason: {reason}\nSource: {source_description}\nIntended target: {target_display}\nContent preview:\n{content_preview}"
        );

        // Review the real intended crossing, not the pending-zar parking path.
        let review_target = real_target.as_deref().unwrap_or(pending_path);
        let action = Action::PromoteFromMindlock {
            source_path: pending_path.to_path_buf(),
            target_path: review_target.to_path_buf(),
        };

        let result = self
            .review_ingress(IntegrityTier::Untrusted, &action, &summary)
            .await;

        let artifact_id = pending_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown");

        let assessment = match result {
            Ok(decision) => {
                json!({
                    "status": "complete",
                    "verdict": verdict_name(decision.verdict),
                    "reason": decision.reason,
                    "target_path": target_display,
                    "reviewed_at": now_epoch(),
                })
            }
            Err(e) => {
                eprintln!("[vericore] auto_review_for_pending: reviewer error: {e}");
                json!({
                    "status": "error",
                    "verdict": "error",
                    "reason": format!("Reviewer failed: {e}"),
                    "target_path": target_display,
                    "reviewed_at": now_epoch(),
                })
            }
        };

        // Write assessment into the meta sidecar.
        // Tolerate races: if Zar already approved/rejected, meta or artifact may be gone.
        if let Ok(fresh_raw) = tokio::fs::read(&meta_path).await {
            let mut meta_value =
                serde_json::from_slice::<Value>(&fresh_raw).unwrap_or_else(|_| json!({}));
            if !meta_value.is_object() {
                meta_value = json!({});
            }
            meta_value["reviewer_assessment"] = assessment.clone();
            if let Ok(updated) = serde_json::to_string_pretty(&meta_value) {
                let _ = tokio::fs::write(&meta_path, updated.as_bytes()).await;
            }
        }

        // Append audit event.
        let audit_dir = self.mindlock_dir.join("audit");
        let _ = tokio::fs::create_dir_all(&audit_dir).await;
        let audit_path = audit_dir.join("events.jsonl");
        let event = json!({
            "ts": now_epoch(),
            "event": "auto_review",
            "id": artifact_id,
            "source_path": pending_path.display().to_string(),
            "target_path": target_display,
            "verdict": assessment.get("verdict").and_then(|v| v.as_str()).unwrap_or("unknown"),
            "reason": assessment.get("reason").and_then(|v| v.as_str()).unwrap_or(""),
        });
        if let Ok(mut line) = serde_json::to_string(&event) {
            line.push('\n');
            if let Ok(mut file) = tokio::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&audit_path)
                .await
            {
                let _ = file.write_all(line.as_bytes()).await;
            }
        }
    }

    pub async fn review_egress(
        &self,
        sink_context: ContextTier,
        response_label: ContextTier,
        draft: &str,
        provenance: &[String],
    ) -> Result<SecurityReviewDecision, String> {
        let payload = json!({
            "sink_tier": tier_name(sink_context),
            "response_label": tier_name(response_label),
            "draft": draft,
            "provenance": provenance,
            "mindlock_dir": self.mindlock_dir.display().to_string(),
            "trusted_write_prefixes": self
                .trusted_write_prefixes
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>(),
        });

        self.review(&self.egress_prompt, payload).await
    }

    pub async fn review_ingress(
        &self,
        ingress_integrity: IntegrityTier,
        action: &Action,
        source_summary: &str,
    ) -> Result<SecurityReviewDecision, String> {
        let payload = json!({
            "ingress_integrity": integrity_name(ingress_integrity),
            "source_summary": source_summary,
            "action": action_summary(action),
            "mindlock_dir": self.mindlock_dir.display().to_string(),
            "trusted_write_prefixes": self
                .trusted_write_prefixes
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>(),
        });

        self.review(&self.ingress_prompt, payload).await
    }

    pub fn stage_incoming_write(
        &self,
        target_path: &Path,
        content: &str,
        source_summary: &str,
        reason: &str,
    ) -> Result<StagedWrite, String> {
        self.ensure_mindlock_dirs()?;
        let id = fresh_id();
        let in_dir = self.mindlock_dir.join("in");
        let artifact_path = in_dir.join(format!("{id}.txt"));
        let meta_path = in_dir.join(format!("{id}.meta.json"));

        fs::write(&artifact_path, content).map_err(|e| {
            format!(
                "mindlock in write failed ({}): {e}",
                artifact_path.display()
            )
        })?;

        let meta = json!({
            "id": id,
            "status": "in",
            "target_path": target_path.display().to_string(),
            "reason": reason,
            "source_summary": source_summary,
            "created_at": now_secs(),
        });
        fs::write(
            &meta_path,
            serde_json::to_vec_pretty(&meta).unwrap_or_default(),
        )
        .map_err(|e| {
            format!(
                "mindlock in meta write failed ({}): {e}",
                meta_path.display()
            )
        })?;

        self.append_audit("staged_in", &id, target_path, reason)?;

        Ok(StagedWrite {
            id,
            artifact_path,
            meta_path,
            target_path: canonicalize_write_target_best_effort(target_path),
        })
    }

    pub fn move_staged_to_out(
        &self,
        staged: &StagedWrite,
        reason: &str,
    ) -> Result<PathBuf, String> {
        let out_dir = self.mindlock_dir.join("out");
        let artifact = out_dir.join(format!("{}.txt", staged.id));
        let meta = out_dir.join(format!("{}.meta.json", staged.id));
        fs::rename(&staged.artifact_path, &artifact).map_err(|e| {
            format!(
                "mindlock move to out failed ({} -> {}): {e}",
                staged.artifact_path.display(),
                artifact.display()
            )
        })?;
        let _ = fs::rename(&staged.meta_path, &meta);
        self.append_audit("moved_to_out", &staged.id, &staged.target_path, reason)?;
        Ok(artifact)
    }

    pub fn hold_staged_for_zar(
        &self,
        staged: &StagedWrite,
        reason: &str,
    ) -> Result<PathBuf, String> {
        let pending = self.mindlock_dir.join("pending-zar");
        let artifact = pending.join(format!("{}.txt", staged.id));
        let meta = pending.join(format!("{}.meta.json", staged.id));
        fs::rename(&staged.artifact_path, &artifact).map_err(|e| {
            format!(
                "mindlock move to pending-zar failed ({} -> {}): {e}",
                staged.artifact_path.display(),
                artifact.display()
            )
        })?;
        let _ = fs::rename(&staged.meta_path, &meta);
        self.append_audit("pending_zar", &staged.id, &staged.target_path, reason)?;
        Ok(artifact)
    }

    pub fn reject_staged_write(
        &self,
        staged: &StagedWrite,
        reason: &str,
    ) -> Result<PathBuf, String> {
        let rejected = self.mindlock_dir.join("rejected");
        let artifact = rejected.join(format!("{}.txt", staged.id));
        let meta = rejected.join(format!("{}.meta.json", staged.id));
        fs::rename(&staged.artifact_path, &artifact).map_err(|e| {
            format!(
                "mindlock move to rejected failed ({} -> {}): {e}",
                staged.artifact_path.display(),
                artifact.display()
            )
        })?;
        let _ = fs::rename(&staged.meta_path, &meta);
        self.append_audit("rejected", &staged.id, &staged.target_path, reason)?;
        Ok(artifact)
    }

    pub fn hold_artifact_for_zar(
        &self,
        artifact_path: &Path,
        reason: &str,
    ) -> Result<PathBuf, String> {
        self.move_existing_artifact(artifact_path, "pending-zar", "pending_zar", reason)
    }

    pub fn reject_artifact(&self, artifact_path: &Path, reason: &str) -> Result<PathBuf, String> {
        self.move_existing_artifact(artifact_path, "rejected", "rejected", reason)
    }

    fn move_existing_artifact(
        &self,
        artifact_path: &Path,
        dest_subdir: &str,
        event: &str,
        reason: &str,
    ) -> Result<PathBuf, String> {
        self.ensure_mindlock_dirs()?;

        // Source directory validation: enforce terminal-state invariant.
        // Artifact must be under a valid mindlock subdirectory (full path prefix,
        // not just basename) to prevent path spoofing like /tmp/in/evil.txt.
        let valid_source = ["in", "out", "work", "pending-zar"]
            .iter()
            .any(|subdir| artifact_path.starts_with(self.mindlock_dir.join(subdir)));
        if !valid_source {
            let is_rejected = artifact_path.starts_with(self.mindlock_dir.join("rejected"));
            if is_rejected {
                return Err(format!(
                    "cannot move artifact from terminal state: rejected/ ({})",
                    artifact_path.display()
                ));
            }
            return Err(format!(
                "artifact path is not under a valid mindlock staging directory ({})",
                artifact_path.display()
            ));
        }

        if !artifact_path.exists() {
            return Err(format!(
                "mindlock artifact not found for move: {}",
                artifact_path.display()
            ));
        }

        let file_name = artifact_path
            .file_name()
            .ok_or_else(|| format!("invalid artifact path: {}", artifact_path.display()))?
            .to_string_lossy()
            .to_string();

        let id = artifact_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("artifact")
            .to_string();

        let dest_dir = self.mindlock_dir.join(dest_subdir);
        let mut dest_artifact = dest_dir.join(&file_name);
        if dest_artifact.exists() {
            dest_artifact = dest_dir.join(format!("{}-{}", now_millis(), file_name));
        }

        fs::rename(artifact_path, &dest_artifact).map_err(|e| {
            format!(
                "mindlock move failed ({} -> {}): {e}",
                artifact_path.display(),
                dest_artifact.display()
            )
        })?;

        let meta_src = artifact_path.with_extension("meta.json");
        if meta_src.exists() {
            let meta_name = meta_src
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| format!("{}.meta.json", id));
            let mut meta_dst = dest_dir.join(meta_name);
            if meta_dst.exists() {
                meta_dst = dest_dir.join(format!("{}-{}.meta.json", now_millis(), id));
            }
            let _ = fs::rename(meta_src, meta_dst);
        }

        self.append_audit(event, &id, &dest_artifact, reason)?;
        Ok(dest_artifact)
    }

    pub fn record_artifact_review_receipt(
        &self,
        artifact_path: &Path,
        source_stage: &str,
        target_path: &Path,
        reason: &str,
    ) -> Result<String, String> {
        let bytes = fs::read(artifact_path).map_err(|e| {
            format!(
                "failed to read artifact for receipt ({}): {e}",
                artifact_path.display()
            )
        })?;
        let approved_hash = sha256_hex(&bytes);

        let meta_path = artifact_path.with_extension("meta.json");
        let mut meta = if meta_path.exists() {
            let raw = fs::read(&meta_path).map_err(|e| {
                format!(
                    "failed to read artifact meta ({}): {e}",
                    meta_path.display()
                )
            })?;
            serde_json::from_slice::<serde_json::Value>(&raw).unwrap_or_else(|_| json!({}))
        } else {
            json!({})
        };

        if !meta.is_object() {
            meta = json!({});
        }

        meta["review"] = json!({
            "approved_hash": approved_hash,
            "source_stage": source_stage,
            "target_path": target_path.display().to_string(),
            "approved_at": now_secs(),
            "reason": reason,
        });

        fs::write(
            &meta_path,
            serde_json::to_vec_pretty(&meta).unwrap_or_default(),
        )
        .map_err(|e| {
            format!(
                "failed to write artifact meta ({}): {e}",
                meta_path.display()
            )
        })?;

        let id = artifact_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("artifact")
            .to_string();
        self.append_audit("review_receipt", &id, target_path, reason)?;

        Ok(approved_hash)
    }

    pub fn verify_artifact_receipt(
        &self,
        artifact_path: &Path,
        target_path: Option<&Path>,
    ) -> Result<(), String> {
        let bytes = fs::read(artifact_path).map_err(|e| {
            format!(
                "failed to read artifact for receipt verification ({}): {e}",
                artifact_path.display()
            )
        })?;
        let current_hash = sha256_hex(&bytes);

        let meta_path = artifact_path.with_extension("meta.json");
        let meta_bytes = fs::read(&meta_path)
            .map_err(|e| format!("missing review receipt meta ({}): {e}", meta_path.display()))?;
        let meta: serde_json::Value = serde_json::from_slice(&meta_bytes)
            .map_err(|e| format!("invalid review receipt meta ({}): {e}", meta_path.display()))?;
        let review = meta
            .get("review")
            .and_then(|v| v.as_object())
            .ok_or_else(|| format!("missing review receipt in meta ({})", meta_path.display()))?;

        let approved_hash = review
            .get("approved_hash")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                format!(
                    "review receipt missing approved_hash ({})",
                    meta_path.display()
                )
            })?;

        if approved_hash != current_hash {
            return Err(format!(
                "review receipt hash mismatch for {}",
                artifact_path.display()
            ));
        }

        if let Some(expected_target) = target_path {
            let approved_target = review
                .get("target_path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    format!(
                        "review receipt missing target_path ({})",
                        meta_path.display()
                    )
                })?;

            if approved_target != expected_target.display().to_string() {
                return Err(format!(
                    "review receipt target mismatch (approved={}, expected={})",
                    approved_target,
                    expected_target.display()
                ));
            }
        }

        Ok(())
    }

    pub fn is_mindlock_artifact_path(&self, path: &Path) -> bool {
        path.starts_with(self.mindlock_dir.join("in"))
            || path.starts_with(self.mindlock_dir.join("out"))
            || path.starts_with(self.mindlock_dir.join("work"))
    }

    fn ensure_mindlock_dirs(&self) -> Result<(), String> {
        for sub in ["in", "work", "out", "pending-zar", "rejected", "audit"] {
            let dir = self.mindlock_dir.join(sub);
            fs::create_dir_all(&dir)
                .map_err(|e| format!("failed to create mindlock dir {}: {e}", dir.display()))?;
        }
        Ok(())
    }

    async fn review(
        &self,
        system_prompt: &str,
        mut payload: Value,
    ) -> Result<SecurityReviewDecision, String> {
        let rag_query = build_reviewer_rag_query(&payload);
        let rag_hits = self.reviewer_rag_hits(&rag_query, REVIEWER_RAG_LIMIT).await;

        if let Some(obj) = payload.as_object_mut() {
            if !rag_query.is_empty() {
                obj.insert("reviewer_rag_query".to_string(), Value::String(rag_query));
            }
            if !rag_hits.is_empty() {
                obj.insert("reviewer_rag_hits".to_string(), Value::Array(rag_hits));
            }
        }

        let user_prompt = format!(
            "Analyze this request and return STRICT JSON only.\nPayload: {}",
            payload
        );
        let system_message = if self.reviewer_context.trim().is_empty() {
            system_prompt.to_string()
        } else {
            format!(
                "{}\n\nReviewer Context (policy and identity grounding):\n{}",
                system_prompt, self.reviewer_context
            )
        };
        let messages = vec![
            ChatMessage::system(&system_message),
            ChatMessage::user(&user_prompt),
        ];

        let (result, _usage) = self
            .llm
            .chat(&messages, &[])
            .await
            .map_err(|e| format!("security review LLM call failed: {e}"))?;

        let text = match result {
            LlmTurnResult::FinalResponse { content } => content,
            LlmTurnResult::ToolCalls { content, .. } => content.unwrap_or_default(),
        };

        parse_decision(&text)
    }

    fn append_audit(
        &self,
        event: &str,
        id: &str,
        target_path: &Path,
        reason: &str,
    ) -> Result<(), String> {
        let path = self.mindlock_dir.join("audit").join("events.jsonl");
        let line = json!({
            "ts": now_secs(),
            "event": event,
            "id": id,
            "target_path": target_path.display().to_string(),
            "reason": reason,
        });
        let mut buf =
            serde_json::to_string(&line).map_err(|e| format!("audit serialize failed: {e}"))?;
        buf.push('\n');
        use std::io::Write;
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|e| format!("open audit log failed ({}): {e}", path.display()))?;
        file.write_all(buf.as_bytes())
            .map_err(|e| format!("write audit log failed ({}): {e}", path.display()))
    }
}

fn action_summary(action: &Action) -> serde_json::Value {
    match action.executable_action() {
        Action::Exec { command } => json!({"kind": "exec", "command": command}),
        Action::WriteFile { path, content } => json!({
            "kind": "write_file",
            "path": path.display().to_string(),
            "content_preview": truncate_chars(content, 500),
            "content_length": content.len(),
        }),
        Action::ReadFile { path } => {
            json!({"kind": "read_file", "path": path.display().to_string()})
        }
        Action::ListDir { path } => json!({"kind": "list_dir", "path": path.display().to_string()}),
        Action::WebFetch { host, path } => json!({"kind": "web_fetch", "host": host, "path": path}),
        Action::Respond {
            channel, content, ..
        } => json!({
            "kind": "respond",
            "channel": format!("{:?}", channel),
            "content_preview": truncate_chars(content, 500),
        }),
        Action::PromoteFromMindlock {
            source_path,
            target_path,
        } => json!({
            "kind": "promote_from_mindlock",
            "source_path": source_path.display().to_string(),
            "target_path": target_path.display().to_string(),
        }),
        Action::RequestReview {
            source_path,
            target_path,
        } => json!({
            "kind": "request_review",
            "source_path": source_path.display().to_string(),
            "target_path": target_path.display().to_string(),
        }),
        Action::SelfEscalate {
            source_path,
            reason,
        } => json!({
            "kind": "self_escalate",
            "source_path": source_path.display().to_string(),
            "reason": reason,
        }),
        Action::ToolAction {
            tool_name,
            skill_name,
            action,
        } => json!({
            "kind": "tool_action",
            "tool_name": tool_name,
            "skill_name": skill_name,
            "inner": action_summary(action),
        }),
        Action::BrokeredTool {
            capability_id,
            display_name,
            arguments,
        } => json!({
            "kind": "brokered_tool",
            "capability_id": capability_id,
            "display_name": display_name,
            "arguments": arguments,
        }),
        Action::NoOp { reason } => json!({"kind": "noop", "reason": reason}),
    }
}

fn parse_decision(raw: &str) -> Result<SecurityReviewDecision, String> {
    let parsed =
        parse_wire(raw).map_err(|e| format!("security review parse failed: {e}; raw={raw}"))?;

    let verdict_raw = parsed
        .verdict
        .or(parsed.decision)
        .ok_or_else(|| "missing verdict/decision".to_string())?;
    let verdict = parse_verdict(&verdict_raw)?;

    let reason = parsed
        .reason
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| default_reason(verdict.clone()).to_string());

    let feedback = parsed
        .feedback
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let safe_rewrite = parsed
        .safe_rewrite
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    Ok(SecurityReviewDecision {
        verdict,
        reason,
        feedback,
        safe_rewrite,
    })
}

fn parse_wire(raw: &str) -> Result<DecisionWire, String> {
    if let Ok(v) = serde_json::from_str::<DecisionWire>(raw.trim()) {
        return Ok(v);
    }

    let trimmed = raw.trim();
    let Some(start) = trimmed.find('{') else {
        return Err("no JSON object start".to_string());
    };
    let Some(end) = trimmed.rfind('}') else {
        return Err("no JSON object end".to_string());
    };
    if end <= start {
        return Err("invalid JSON object bounds".to_string());
    }
    let slice = &trimmed[start..=end];
    serde_json::from_str::<DecisionWire>(slice).map_err(|e| format!("JSON decode error: {e}"))
}

fn parse_verdict(raw: &str) -> Result<SecurityVerdict, String> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "allow" => Ok(SecurityVerdict::Allow),
        "request_revision" | "request-revision" => Ok(SecurityVerdict::RequestRevision),
        "require_zar_approval" | "require-zar-approval" | "zar" => {
            Ok(SecurityVerdict::RequireZarApproval)
        }
        "reject" => Ok(SecurityVerdict::Reject),
        other => Err(format!("unknown verdict: {other}")),
    }
}

fn default_reason(verdict: SecurityVerdict) -> &'static str {
    match verdict {
        SecurityVerdict::Allow => "security review allowed",
        SecurityVerdict::RequestRevision => "security review requested revision",
        SecurityVerdict::RequireZarApproval => "security review requires Zar approval",
        SecurityVerdict::Reject => "security review rejected",
    }
}

fn build_reviewer_rag_query(payload: &Value) -> String {
    let mut parts: Vec<String> = Vec::new();

    if let Some(source_summary) = payload.get("source_summary").and_then(Value::as_str) {
        if !source_summary.trim().is_empty() {
            parts.push(source_summary.trim().to_string());
        }
    }

    if let Some(draft) = payload.get("draft").and_then(Value::as_str) {
        let draft = truncate_chars(draft.trim(), 700);
        if !draft.is_empty() {
            parts.push(draft);
        }
    }

    if let Some(provenance) = payload.get("provenance").and_then(Value::as_array) {
        let mut provenance_bits: Vec<String> = Vec::new();
        for value in provenance.iter().take(6) {
            if let Some(s) = value.as_str() {
                let trimmed = s.trim();
                if !trimmed.is_empty() {
                    provenance_bits.push(trimmed.to_string());
                }
            }
        }
        if !provenance_bits.is_empty() {
            parts.push(provenance_bits.join(" | "));
        }
    }

    if let Some(action) = payload.get("action") {
        let action_text = truncate_chars(&action.to_string(), 700);
        if !action_text.trim().is_empty() {
            parts.push(action_text);
        }
    }

    truncate_chars(&parts.join("\n"), REVIEWER_RAG_QUERY_MAX_CHARS)
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

fn read_prompt(path: Option<&Path>, fallback: &str) -> Result<String, String> {
    if let Some(path) = path {
        return fs::read_to_string(path)
            .map_err(|e| format!("failed to read security prompt {}: {e}", path.display()));
    }
    Ok(fallback.to_string())
}

fn read_optional_file(path: Option<&Path>) -> Result<String, String> {
    let Some(path) = path else {
        return Ok(String::new());
    };

    match fs::read_to_string(path) {
        Ok(content) => Ok(content),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(err) => Err(format!(
            "failed to read security reviewer context {}: {err}",
            path.display()
        )),
    }
}

fn canonicalize_best_effort(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn canonicalize_write_target_best_effort(path: &Path) -> PathBuf {
    if !path.is_absolute() {
        return path.to_path_buf();
    }

    let Some(file_name) = path.file_name() else {
        return path.to_path_buf();
    };
    let Some(parent) = path.parent() else {
        return path.to_path_buf();
    };

    match fs::canonicalize(parent) {
        Ok(canonical_parent) => canonical_parent.join(file_name),
        Err(_) => path.to_path_buf(),
    }
}

fn tier_name(tier: ContextTier) -> &'static str {
    match tier {
        ContextTier::Public => "public",
        ContextTier::Family => "family",
        ContextTier::Private => "private",
    }
}

fn verdict_name(verdict: SecurityVerdict) -> &'static str {
    match verdict {
        SecurityVerdict::Allow => "allow",
        SecurityVerdict::RequestRevision => "request_revision",
        SecurityVerdict::RequireZarApproval => "require_zar_approval",
        SecurityVerdict::Reject => "reject",
    }
}

fn integrity_name(tier: IntegrityTier) -> &'static str {
    match tier {
        IntegrityTier::Untrusted => "untrusted",
        IntegrityTier::Reviewed => "reviewed",
        IntegrityTier::Trusted => "trusted",
    }
}

fn fresh_id() -> String {
    format!("{}-{}", now_millis(), std::process::id())
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn now_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

fn now_epoch() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
