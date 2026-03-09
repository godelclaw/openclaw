use std::fmt;
use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

use crate::config::ToolBrokerConfig;

// ── Types ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDescriptor {
    pub capability_id: String,
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
    pub source: String,
    #[serde(default)]
    pub mcp_server: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ToolCallResult {
    pub result: String,
    pub is_error: bool,
    pub execution_ms: Option<u64>,
}

#[derive(Debug)]
pub enum ToolBrokerError {
    Socket(std::io::Error),
    Parse(String),
    ServerError(String),
}

impl fmt::Display for ToolBrokerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ToolBrokerError::Socket(e) => write!(f, "tool broker socket error: {e}"),
            ToolBrokerError::Parse(msg) => write!(f, "tool broker parse error: {msg}"),
            ToolBrokerError::ServerError(msg) => write!(f, "tool broker server error: {msg}"),
        }
    }
}

// ── Wire protocol types ────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
struct ListToolsRequest<'a> {
    request_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    session_key: Option<&'a str>,
    call_kind: &'static str,
}

#[derive(Debug, Serialize)]
struct CallToolRequest<'a> {
    request_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    session_key: Option<&'a str>,
    call_kind: &'static str,
    capability_id: &'a str,
    arguments: &'a serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    audit_id: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct ListToolsResponse {
    ok: bool,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    tools: Vec<ToolDescriptor>,
}

#[derive(Debug, Deserialize)]
struct CallToolResponse {
    ok: bool,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    result: Option<String>,
    #[serde(default)]
    is_error: Option<bool>,
    #[serde(default)]
    execution_ms: Option<u64>,
}

// ── Client ─────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct GatewayToolBrokerClient {
    socket_path: PathBuf,
    timeout: Duration,
    session_key: Option<String>,
}

impl GatewayToolBrokerClient {
    pub fn new(socket_path: PathBuf, timeout: Duration, session_key: Option<String>) -> Self {
        Self {
            socket_path,
            timeout,
            session_key,
        }
    }

    /// Create from config, resolving the socket path.
    pub fn from_config(
        config: &ToolBrokerConfig,
        session_key: Option<String>,
    ) -> Result<Self, ToolBrokerError> {
        let socket_path = if let Some(ref path) = config.gateway_socket {
            PathBuf::from(path)
        } else {
            let uid = std::env::var("XDG_RUNTIME_DIR")
                .ok()
                .and_then(|d| d.strip_prefix("/run/user/").map(|s| s.to_string()))
                .unwrap_or_else(|| "1001".to_string());
            PathBuf::from(format!("/run/user/{uid}/openclaw-toolbroker.sock"))
        };
        Ok(Self {
            socket_path,
            timeout: Duration::from_secs(config.timeout_secs),
            session_key,
        })
    }

    /// List all tools available from the TS tool broker.
    pub async fn list_tools(&self) -> Result<Vec<ToolDescriptor>, ToolBrokerError> {
        let request = ListToolsRequest {
            request_id: generate_request_id(),
            session_key: self.session_key.as_deref(),
            call_kind: "list_tools",
        };

        let request_json =
            serde_json::to_vec(&request).map_err(|e| ToolBrokerError::Parse(e.to_string()))?;

        let response_json = self.send_to_broker(&request_json).await?;

        let resp: ListToolsResponse = serde_json::from_slice(&response_json).map_err(|e| {
            ToolBrokerError::Parse(format!("failed to parse list_tools response: {e}"))
        })?;

        if !resp.ok {
            return Err(ToolBrokerError::ServerError(
                resp.error.unwrap_or_else(|| "unknown broker error".into()),
            ));
        }

        Ok(resp.tools)
    }

    /// Call a specific tool via the TS tool broker.
    pub async fn call_tool(
        &self,
        capability_id: &str,
        args: serde_json::Value,
        audit_id: Option<u64>,
    ) -> Result<ToolCallResult, ToolBrokerError> {
        let request = CallToolRequest {
            request_id: generate_request_id(),
            session_key: self.session_key.as_deref(),
            call_kind: "call_tool",
            capability_id,
            arguments: &args,
            audit_id,
        };

        let request_json =
            serde_json::to_vec(&request).map_err(|e| ToolBrokerError::Parse(e.to_string()))?;

        let response_json = self.send_to_broker(&request_json).await?;

        let resp: CallToolResponse = serde_json::from_slice(&response_json).map_err(|e| {
            ToolBrokerError::Parse(format!("failed to parse call_tool response: {e}"))
        })?;

        if !resp.ok {
            return Err(ToolBrokerError::ServerError(
                resp.error.unwrap_or_else(|| "unknown broker error".into()),
            ));
        }

        Ok(ToolCallResult {
            result: resp.result.unwrap_or_default(),
            is_error: resp.is_error.unwrap_or(false),
            execution_ms: resp.execution_ms,
        })
    }

    /// Send a length-prefixed JSON request to the broker socket and read the response.
    /// Wire protocol: 4-byte big-endian length + JSON payload (same as llm.rs).
    async fn send_to_broker(&self, request_json: &[u8]) -> Result<Vec<u8>, ToolBrokerError> {
        // Connect with 5s timeout
        let connect_fut = UnixStream::connect(&self.socket_path);
        let mut stream = tokio::time::timeout(Duration::from_secs(5), connect_fut)
            .await
            .map_err(|_| {
                ToolBrokerError::Socket(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    format!(
                        "timeout connecting to tool broker socket at {}",
                        self.socket_path.display()
                    ),
                ))
            })?
            .map_err(ToolBrokerError::Socket)?;

        // Write: 4-byte big-endian length + JSON payload, then shutdown write side
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
                ToolBrokerError::Socket(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "timeout writing to tool broker socket",
                ))
            })?
            .map_err(ToolBrokerError::Socket)?;

        // Read: 4-byte big-endian length + JSON response
        let read_fut = async {
            let mut len_buf = [0u8; 4];
            stream.read_exact(&mut len_buf).await?;
            let resp_len = u32::from_be_bytes(len_buf) as usize;
            if resp_len > 10 * 1024 * 1024 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("tool broker response too large: {resp_len} bytes"),
                ));
            }
            let mut buf = vec![0u8; resp_len];
            stream.read_exact(&mut buf).await?;
            Ok(buf)
        };

        tokio::time::timeout(self.timeout, read_fut)
            .await
            .map_err(|_| {
                ToolBrokerError::Socket(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    format!(
                        "timeout waiting for tool broker response ({}s)",
                        self.timeout.as_secs()
                    ),
                ))
            })?
            .map_err(ToolBrokerError::Socket)
    }
}

fn generate_request_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("tb-{nanos:x}")
}
