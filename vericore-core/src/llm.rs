use std::fmt;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::config::LlmConfig;

#[derive(Clone)]
pub struct LlmClient {
    http: reqwest::Client,
    api_base: String,
    api_key: String,
    model: String,
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
    Http(reqwest::Error),
    Api { status: u16, body: String },
    Parse(String),
    NoApiKey(String),
}

impl fmt::Display for LlmError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LlmError::Http(e) => write!(f, "HTTP error: {e}"),
            LlmError::Api { status, body } => write!(f, "API error ({status}): {body}"),
            LlmError::Parse(msg) => write!(f, "parse error: {msg}"),
            LlmError::NoApiKey(env) => write!(f, "API key not found in env var: {env}"),
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

impl LlmClient {
    pub fn from_config(config: &LlmConfig) -> Result<Self, LlmError> {
        let api_key = std::env::var(&config.api_key_env)
            .map_err(|_| LlmError::NoApiKey(config.api_key_env.clone()))?;

        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(config.timeout_secs))
            .build()
            .map_err(LlmError::Http)?;

        Ok(Self {
            http,
            api_base: config.api_base.clone(),
            api_key,
            model: config.model.clone(),
            max_tokens: config.max_tokens,
            temperature: config.temperature,
        })
    }

    pub async fn chat(
        &self,
        messages: &[ChatMessage],
        tools: &[serde_json::Value],
    ) -> Result<(LlmTurnResult, LlmUsage), LlmError> {
        let mut body = serde_json::json!({
            "model": self.model,
            "messages": messages,
            "max_tokens": self.max_tokens,
            "temperature": self.temperature,
        });

        if !tools.is_empty() {
            body["tools"] = serde_json::json!(tools);
            body["tool_choice"] = serde_json::json!("auto");
        }

        let resp = self
            .http
            .post(format!("{}/chat/completions", self.api_base))
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .header("HTTP-Referer", "https://github.com/godelclaw/vericore")
            .header("X-Title", "VeriCore")
            .json(&body)
            .send()
            .await
            .map_err(LlmError::Http)?;

        let status = resp.status().as_u16();
        if status >= 400 {
            let body = resp.text().await.unwrap_or_default();
            return Err(LlmError::Api { status, body });
        }

        let json: serde_json::Value = resp.json().await.map_err(LlmError::Http)?;

        let usage = LlmUsage {
            prompt_tokens: json["usage"]["prompt_tokens"].as_u64().unwrap_or(0),
            completion_tokens: json["usage"]["completion_tokens"].as_u64().unwrap_or(0),
        };

        let choice = &json["choices"][0]["message"];
        let content = choice["content"].as_str().map(String::from);
        let tool_calls_raw = choice.get("tool_calls").and_then(|v| v.as_array());

        if let Some(calls) = tool_calls_raw
            && !calls.is_empty()
        {
            let mut parsed = Vec::new();
            for call in calls {
                let id = call["id"].as_str().unwrap_or("").to_string();
                let name = call["function"]["name"].as_str().unwrap_or("").to_string();
                let args_str = call["function"]["arguments"].as_str().unwrap_or("{}");
                let arguments: serde_json::Value =
                    serde_json::from_str(args_str).unwrap_or(serde_json::json!({}));
                parsed.push(ToolCall {
                    id,
                    name,
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

        let text = content.unwrap_or_default();
        Ok((LlmTurnResult::FinalResponse { content: text }, usage))
    }
}
