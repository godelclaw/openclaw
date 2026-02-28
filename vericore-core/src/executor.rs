use std::path::Path;
use std::time::Duration;

use crate::config::TurnConfig;
use crate::llm::ToolCall;

const MAX_OUTPUT: usize = 10_000;

pub struct Executor {
    exec_timeout: Duration,
}

impl Executor {
    pub fn new(config: &TurnConfig) -> Self {
        Self {
            exec_timeout: Duration::from_secs(config.exec_timeout_secs),
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
}

pub struct ToolResult {
    pub tool_call_id: String,
    pub tool_name: String,
    pub content: String,
    pub is_error: bool,
}

fn truncate(s: &str) -> String {
    if s.len() <= MAX_OUTPUT {
        s.to_string()
    } else {
        let mut result = s[..MAX_OUTPUT].to_string();
        result.push_str("\n... (truncated)");
        result
    }
}
