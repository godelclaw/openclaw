use std::fmt;
use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

use crate::config::LlmConfig;

/// What kind of LLM call is being made (for TS-side logging/routing).
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CallKind {
    DriverTurn,
    SecurityReview,
    MemoryRefine,
}

/// Whether the caller is the driver agent or the security reviewer.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptMode {
    Driver,
    Reviewer,
}

#[derive(Clone)]
pub struct GatewayLlmClient {
    socket_path: PathBuf,
    timeout: Duration,
    call_kind: CallKind,
    prompt_mode: PromptMode,
    session_key: Option<String>,
    max_tokens: u32,
    temperature: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCallWire>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallWire {
    pub id: String,
    #[serde(rename = "type")]
    pub call_type: String,
    pub function: ToolCallFunction,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallFunction {
    pub name: String,
    pub arguments: String, // JSON string
}

#[derive(Debug, Clone)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

#[derive(Debug, Clone)]
pub enum LlmTurnResult {
    ToolCalls {
        content: Option<String>,
        tool_calls: Vec<ToolCall>,
    },
    FinalResponse {
        content: String,
    },
}

#[derive(Debug, Clone, Default)]
pub struct LlmUsage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
}

#[derive(Debug)]
pub enum LlmError {
    Socket(std::io::Error),
    Bridge { message: String },
    Parse(String),
    NoGatewaySocket,
}

impl fmt::Display for LlmError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LlmError::Socket(e) => write!(f, "gateway socket error: {e}"),
            LlmError::Bridge { message } => write!(f, "gateway bridge error: {message}"),
            LlmError::Parse(msg) => write!(f, "parse error: {msg}"),
            LlmError::NoGatewaySocket => write!(
                f,
                "no gateway_socket configured in [llm] — direct HTTP LLM calls are disabled"
            ),
        }
    }
}

impl ChatMessage {
    pub fn system(content: &str) -> Self {
        Self {
            role: "system".into(),
            content: Some(content.into()),
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }
    }
    pub fn user(content: &str) -> Self {
        Self {
            role: "user".into(),
            content: Some(content.into()),
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }
    }
    pub fn assistant(content: &str) -> Self {
        Self {
            role: "assistant".into(),
            content: Some(content.into()),
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }
    }
    pub fn assistant_with_tools(content: Option<&str>, calls: &[ToolCall]) -> Self {
        let wire_calls: Vec<ToolCallWire> = calls
            .iter()
            .map(|c| ToolCallWire {
                id: c.id.clone(),
                call_type: "function".into(),
                function: ToolCallFunction {
                    name: c.name.clone(),
                    arguments: serde_json::to_string(&c.arguments).unwrap_or_default(),
                },
            })
            .collect();
        Self {
            role: "assistant".into(),
            content: content.map(String::from),
            tool_calls: Some(wire_calls),
            tool_call_id: None,
            name: None,
        }
    }
    pub fn tool_result(tool_call_id: &str, name: &str, content: &str) -> Self {
        Self {
            role: "tool".into(),
            content: Some(content.into()),
            tool_calls: None,
            tool_call_id: Some(tool_call_id.into()),
            name: Some(name.into()),
        }
    }
}

// ── Bridge request/response types ──────────────────────────────────────────

#[derive(Debug, Serialize)]
struct BridgeRequest<'a> {
    request_id: String,
    session_key: Option<&'a str>,
    call_kind: CallKind,
    prompt_mode: PromptMode,
    messages: &'a [ChatMessage],
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<&'a [serde_json::Value]>,
    max_tokens: u32,
    temperature: f64,
}

#[derive(Debug, Deserialize)]
struct BridgeResponse {
    ok: bool,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    configured_model: Option<String>,
    #[serde(default)]
    resolved_model: Option<String>,
    #[serde(default)]
    resolved_provider: Option<String>,
    #[serde(default)]
    resolved_profile: Option<String>,
    #[serde(default)]
    fallback_used: Option<bool>,
    #[serde(default)]
    provider_changed: Option<bool>,
    #[serde(default)]
    choices: Option<Vec<BridgeChoice>>,
    #[serde(default)]
    usage: Option<BridgeUsage>,
}

#[derive(Debug, Deserialize)]
struct BridgeChoice {
    message: BridgeChoiceMessage,
}

#[derive(Debug, Deserialize)]
struct BridgeChoiceMessage {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<BridgeToolCall>>,
}

#[derive(Debug, Deserialize)]
struct BridgeToolCall {
    id: String,
    function: BridgeToolCallFunction,
}

#[derive(Debug, Deserialize)]
struct BridgeToolCallFunction {
    name: String,
    arguments: String,
}

#[derive(Debug, Deserialize)]
struct BridgeUsage {
    #[serde(default)]
    prompt_tokens: u64,
    #[serde(default)]
    completion_tokens: u64,
}

// ── GatewayLlmClient ──────────────────────────────────────────────────────

impl GatewayLlmClient {
    pub fn from_config(
        config: &LlmConfig,
        call_kind: CallKind,
        prompt_mode: PromptMode,
        session_key: Option<String>,
    ) -> Result<Self, LlmError> {
        let socket_path = config
            .gateway_socket
            .as_ref()
            .ok_or(LlmError::NoGatewaySocket)?;

        Ok(Self {
            socket_path: PathBuf::from(socket_path),
            timeout: Duration::from_secs(config.timeout_secs),
            call_kind,
            prompt_mode,
            session_key,
            max_tokens: config.max_tokens,
            temperature: config.temperature,
        })
    }

    pub async fn chat(
        &self,
        messages: &[ChatMessage],
        tools: &[serde_json::Value],
    ) -> Result<(LlmTurnResult, LlmUsage), LlmError> {
        let request = BridgeRequest {
            request_id: generate_request_id(),
            session_key: self.session_key.as_deref(),
            call_kind: self.call_kind,
            prompt_mode: self.prompt_mode,
            messages,
            tools: if tools.is_empty() { None } else { Some(tools) },
            max_tokens: self.max_tokens,
            temperature: self.temperature,
        };

        let request_json =
            serde_json::to_vec(&request).map_err(|e| LlmError::Parse(e.to_string()))?;

        let response_json = self.send_to_gateway(&request_json).await?;

        let resp: BridgeResponse = serde_json::from_slice(&response_json)
            .map_err(|e| LlmError::Parse(format!("failed to parse bridge response: {e}")))?;

        if !resp.ok {
            return Err(LlmError::Bridge {
                message: resp.error.unwrap_or_else(|| "unknown bridge error".into()),
            });
        }

        // Log resolution metadata (only on actual fallback)
        if resp.fallback_used.unwrap_or(false) {
            if let (Some(configured), Some(resolved)) =
                (&resp.configured_model, &resp.resolved_model)
            {
                eprintln!(
                    "[vericore] bridge: model fallback {} -> {} (provider: {}, profile: {}, provider_changed: {})",
                    configured,
                    resolved,
                    resp.resolved_provider.as_deref().unwrap_or("unknown"),
                    resp.resolved_profile.as_deref().unwrap_or("default"),
                    resp.provider_changed.unwrap_or(false),
                );
            }
        }

        let usage = resp.usage.map_or(LlmUsage::default(), |u| LlmUsage {
            prompt_tokens: u.prompt_tokens,
            completion_tokens: u.completion_tokens,
        });

        let choices = resp.choices.unwrap_or_default();
        let choice = choices
            .first()
            .ok_or_else(|| LlmError::Parse("bridge response has no choices".into()))?;

        let content = choice.message.content.clone();
        let tool_calls_raw = choice.message.tool_calls.as_ref();

        if let Some(calls) = tool_calls_raw {
            if !calls.is_empty() {
                let mut parsed = Vec::new();
                for call in calls {
                    let args_str = &call.function.arguments;
                    let arguments: serde_json::Value = serde_json::from_str(args_str)
                        .unwrap_or(serde_json::json!({}));
                    parsed.push(ToolCall {
                        id: call.id.clone(),
                        name: call.function.name.clone(),
                        arguments,
                    });
                }
                return Ok((
                    LlmTurnResult::ToolCalls {
                        content,
                        tool_calls: parsed,
                    },
                    usage,
                ));
            }
        }

        let text = content.unwrap_or_default();
        Ok((LlmTurnResult::FinalResponse { content: text }, usage))
    }

    async fn send_to_gateway(&self, request_json: &[u8]) -> Result<Vec<u8>, LlmError> {
        let connect_fut = UnixStream::connect(&self.socket_path);
        let mut stream = tokio::time::timeout(Duration::from_secs(5), connect_fut)
            .await
            .map_err(|_| {
                LlmError::Socket(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    format!(
                        "timeout connecting to gateway socket at {}",
                        self.socket_path.display()
                    ),
                ))
            })?
            .map_err(LlmError::Socket)?;

        // Send length-prefixed JSON: 4-byte big-endian length + payload
        let len = request_json.len() as u32;
        let write_fut = async {
            stream.write_all(&len.to_be_bytes()).await?;
            stream.write_all(request_json).await?;
            stream.shutdown().await?;
            Ok::<(), std::io::Error>(())
        };
        tokio::time::timeout(Duration::from_secs(5), write_fut)
            .await
            .map_err(|_| {
                LlmError::Socket(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "timeout writing to gateway socket",
                ))
            })?
            .map_err(LlmError::Socket)?;

        // Read length-prefixed response
        let read_fut = async {
            let mut len_buf = [0u8; 4];
            stream.read_exact(&mut len_buf).await?;
            let resp_len = u32::from_be_bytes(len_buf) as usize;
            if resp_len > 10 * 1024 * 1024 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("bridge response too large: {resp_len} bytes"),
                ));
            }
            let mut buf = vec![0u8; resp_len];
            stream.read_exact(&mut buf).await?;
            Ok(buf)
        };

        tokio::time::timeout(self.timeout, read_fut)
            .await
            .map_err(|_| {
                LlmError::Socket(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    format!(
                        "timeout waiting for gateway response ({}s)",
                        self.timeout.as_secs()
                    ),
                ))
            })?
            .map_err(LlmError::Socket)
    }
}

fn generate_request_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("vc-{nanos:x}")
}
