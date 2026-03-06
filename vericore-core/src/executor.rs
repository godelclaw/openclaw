use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::utils::sha256_hex;

use crate::config::TurnConfig;
use crate::llm::ToolCall;

const MAX_OUTPUT: usize = 10_000;

pub struct Executor {
    exec_timeout: Duration,
    mindlock_dir: PathBuf,
}

impl Executor {
    pub fn new(config: &TurnConfig, mindlock_dir: PathBuf) -> Self {
        Self {
            exec_timeout: Duration::from_secs(config.exec_timeout_secs),
            mindlock_dir,
        }
    }

    pub async fn execute_tool(&self, tool_call: &ToolCall) -> ToolResult {
        let content = match tool_call.name.as_str() {
            "read_file" => {
                let path = tool_call.arguments["path"].as_str().unwrap_or("");
                self.exec_read_file(Path::new(path)).await
            }
            "write_file" => {
                let path = tool_call.arguments["path"].as_str().unwrap_or("");
                let content = tool_call.arguments["content"].as_str().unwrap_or("");
                self.exec_write_file(Path::new(path), content).await
            }
            "list_dir" => {
                let path = tool_call.arguments["path"].as_str().unwrap_or(".");
                self.exec_list_dir(Path::new(path)).await
            }
            "exec" => {
                let command = tool_call.arguments["command"].as_str().unwrap_or("");
                self.exec_shell(command).await
            }
            "web_fetch" => {
                let url = tool_call.arguments["url"].as_str().unwrap_or("");
                self.exec_web_fetch(url).await
            }
            "promote_from_mindlock" => {
                let source_path = tool_call.arguments["source_path"].as_str().unwrap_or("");
                let target_path = tool_call.arguments["target_path"].as_str().unwrap_or("");
                self.exec_promote_from_mindlock(Path::new(source_path), Path::new(target_path))
                    .await
            }
            other => format!("Error: unknown tool '{other}'"),
        };

        let is_error = content.starts_with("Error:");
        ToolResult {
            tool_call_id: tool_call.id.clone(),
            tool_name: tool_call.name.clone(),
            content,
            is_error,
        }
    }

    async fn exec_read_file(&self, path: &Path) -> String {
        match tokio::fs::read_to_string(path).await {
            Ok(content) => truncate(&content),
            Err(e) => format!("Error: {e}"),
        }
    }

    async fn exec_write_file(&self, path: &Path, content: &str) -> String {
        if let Some(parent) = path.parent()
            && let Err(e) = tokio::fs::create_dir_all(parent).await
        {
            return format!("Error: failed to create parent dirs: {e}");
        }

        match tokio::fs::write(path, content).await {
            Ok(()) => format!("Written {} bytes to {}", content.len(), path.display()),
            Err(e) => format!("Error: {e}"),
        }
    }

    async fn exec_list_dir(&self, path: &Path) -> String {
        let mut entries = match tokio::fs::read_dir(path).await {
            Ok(rd) => rd,
            Err(e) => return format!("Error: {e}"),
        };
        let mut lines = Vec::new();
        while let Ok(Some(entry)) = entries.next_entry().await {
            let name = entry.file_name().to_string_lossy().to_string();
            let is_dir = entry
                .file_type()
                .await
                .map(|ft| ft.is_dir())
                .unwrap_or(false);
            if is_dir {
                lines.push(format!("{name}/"));
            } else {
                lines.push(name);
            }
        }
        lines.sort();
        truncate(&lines.join("\n"))
    }

    async fn exec_shell(&self, command: &str) -> String {
        let result = tokio::time::timeout(
            self.exec_timeout,
            tokio::process::Command::new("bash")
                .arg("-c")
                .arg(command)
                .output(),
        )
        .await;

        match result {
            Ok(Ok(output)) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let stderr = String::from_utf8_lossy(&output.stderr);
                let mut result = String::new();
                if !stdout.is_empty() {
                    result.push_str(&stdout);
                }
                if !stderr.is_empty() {
                    if !result.is_empty() {
                        result.push('\n');
                    }
                    result.push_str("STDERR: ");
                    result.push_str(&stderr);
                }
                if !output.status.success() {
                    result.push_str(&format!(
                        "\nExit code: {}",
                        output.status.code().unwrap_or(-1)
                    ));
                }
                if result.is_empty() {
                    result = "(no output)".into();
                }
                truncate(&result)
            }
            Ok(Err(e)) => format!("Error: {e}"),
            Err(_) => format!("Error: command timed out after {:?}", self.exec_timeout),
        }
    }

    async fn exec_web_fetch(&self, url: &str) -> String {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::limited(5))
            .build();
        let client = match client {
            Ok(c) => c,
            Err(e) => return format!("Error: {e}"),
        };
        match client.get(url).send().await {
            Ok(resp) => {
                let status = resp.status().as_u16();
                match resp.text().await {
                    Ok(body) => {
                        let header = format!("HTTP {status}\n\n");
                        truncate(&format!("{header}{body}"))
                    }
                    Err(e) => format!("Error reading body: {e}"),
                }
            }
            Err(e) => format!("Error: {e}"),
        }
    }

    async fn exec_promote_from_mindlock(&self, source_path: &Path, target_path: &Path) -> String {
        if !source_path.is_absolute() || !target_path.is_absolute() {
            return "Error: source_path and target_path must be absolute".to_string();
        }
        let in_root = self.mindlock_dir.join("in");
        let out_root = self.mindlock_dir.join("out");
        if !source_path.starts_with(&in_root) && !source_path.starts_with(&out_root) {
            return format!(
                "Error: source_path must be under {}/in or {}/out: {}",
                self.mindlock_dir.display(),
                self.mindlock_dir.display(),
                source_path.display()
            );
        }

        let bytes = match tokio::fs::read(source_path).await {
            Ok(b) => b,
            Err(e) => return format!("Error: failed to read source: {e}"),
        };

        let current_hash = sha256_hex(&bytes);
        let meta_path = source_path.with_extension("meta.json");
        let meta_bytes = match tokio::fs::read(&meta_path).await {
            Ok(b) => b,
            Err(e) => {
                return format!(
                    "Error: missing review receipt meta ({}): {e}",
                    meta_path.display()
                );
            }
        };
        let meta: serde_json::Value = match serde_json::from_slice(&meta_bytes) {
            Ok(v) => v,
            Err(e) => {
                return format!(
                    "Error: invalid review receipt meta ({}): {e}",
                    meta_path.display()
                );
            }
        };

        let review = match meta.get("review").and_then(|v| v.as_object()) {
            Some(r) => r,
            None => {
                return format!(
                    "Error: missing review receipt in meta ({})",
                    meta_path.display()
                );
            }
        };

        let approved_hash = match review.get("approved_hash").and_then(|v| v.as_str()) {
            Some(h) => h,
            None => {
                return format!(
                    "Error: review receipt missing approved_hash ({})",
                    meta_path.display()
                );
            }
        };

        if approved_hash != current_hash {
            return format!(
                "Error: review receipt hash mismatch for {}",
                source_path.display()
            );
        }

        let approved_target = match review.get("target_path").and_then(|v| v.as_str()) {
            Some(t) => t,
            None => {
                return format!(
                    "Error: review receipt missing target_path ({})",
                    meta_path.display()
                );
            }
        };

        if approved_target != target_path.display().to_string() {
            return format!(
                "Error: review receipt target mismatch (approved={}, requested={})",
                approved_target,
                target_path.display()
            );
        }

        if let Some(parent) = target_path.parent()
            && let Err(e) = tokio::fs::create_dir_all(parent).await
        {
            return format!("Error: failed to create target parent dirs: {e}");
        }

        if let Err(e) = tokio::fs::write(target_path, bytes).await {
            return format!("Error: failed to write target: {e}");
        }

        let _ = tokio::fs::remove_file(source_path).await;

        // Move sidecar meta to audit if present.
        if let Some(stem) = source_path.file_stem().and_then(|s| s.to_str()) {
            let meta_name = format!("{stem}.meta.json");
            if let Some(parent) = source_path.parent() {
                let meta_src = parent.join(meta_name);
                if tokio::fs::metadata(&meta_src).await.is_ok() {
                    let audit_dir = self.mindlock_dir.join("audit");
                    let _ = tokio::fs::create_dir_all(&audit_dir).await;
                    let meta_dst = audit_dir.join(
                        meta_src
                            .file_name()
                            .and_then(|s| s.to_str())
                            .map(|n| format!("manual-approve-{n}"))
                            .unwrap_or_else(|| "manual-approve.meta.json".to_string()),
                    );
                    let _ = tokio::fs::rename(&meta_src, meta_dst).await;
                }
            }
        }

        format!(
            "Promoted {} -> {}",
            source_path.display(),
            target_path.display()
        )
    }
}

pub struct ToolResult {
    pub tool_call_id: String,
    pub tool_name: String,
    pub content: String,
    pub is_error: bool,
}

fn truncate(s: &str) -> String {
    if s.len() <= MAX_OUTPUT {
        return s.to_string();
    }

    // Keep truncation UTF-8 safe to avoid panics on non-ASCII output.
    let mut end = 0usize;
    for (idx, ch) in s.char_indices() {
        let next = idx + ch.len_utf8();
        if next > MAX_OUTPUT {
            break;
        }
        end = next;
    }

    let mut result = s[..end].to_string();
    result.push_str("\n... (truncated)");
    result
}
