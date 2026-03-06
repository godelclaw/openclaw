use std::path::{Path, PathBuf};

use crate::llm::ToolCall;
use crate::policy::GatePolicy;
use crate::types::{Action, ContextTier};

/// Generate OpenAI-format tool definitions filtered by what the policy allows.
///
/// Filters based on:
/// - Primitive capability (e.g. exec only if exec_allowed_for context)
/// - Action-kind gating (if enabled, only offer tools whose action kind is in the allowlist)
/// - Tool identity gating (if enabled, only offer tools whose name is in the allowlist)
///
/// This prevents the LLM from wasting turns calling tools that will be denied at the gate.
pub fn tool_definitions_for_context(
    policy: &GatePolicy,
    context: ContextTier,
    mindlock_dir: &Path,
) -> Vec<serde_json::Value> {
    let mut defs = Vec::new();

    // Helper: check whether a tool should be offered based on action-kind + tool-identity gating.
    let should_offer = |tool_name: &str, action_kind: &str| -> bool {
        if !policy.check_action_kind(context, action_kind).is_allowed() {
            return false;
        }
        if !policy
            .check_tool_identity(context, Some(tool_name))
            .is_allowed()
        {
            return false;
        }
        true
    };

    if should_offer("read_file", "read_file") {
        defs.push(make_tool(
            "read_file",
            "Read the contents of a file at the given path.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Absolute path to the file" }
                },
                "required": ["path"]
            }),
        ));
    }

    // list_dir gates as read_file (listing is a read operation)
    if should_offer("list_dir", "read_file") {
        defs.push(make_tool(
            "list_dir",
            "List the contents of a directory.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Absolute path to the directory" }
                },
                "required": ["path"]
            }),
        ));
    }

    if should_offer("write_file", "write_file") && context.can_write(ContextTier::Public) {
        defs.push(make_tool(
            "write_file",
            "Write content to a file at the given path.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Absolute path to write" },
                    "content": { "type": "string", "description": "File content to write" }
                },
                "required": ["path", "content"]
            }),
        ));
    }

    if should_offer("exec", "exec") && policy.exec_allowed_for(context) {
        defs.push(make_tool(
            "exec",
            "Execute a shell command.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "Shell command to execute" }
                },
                "required": ["command"]
            }),
        ));
    }

    if should_offer("web_fetch", "web_fetch") {
        defs.push(make_tool(
            "web_fetch",
            "Fetch the contents of a URL.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "url": { "type": "string", "description": "URL to fetch" }
                },
                "required": ["url"]
            }),
        ));
    }
    if should_offer("promote_from_mindlock", "write_file") && context == ContextTier::Private {
        defs.push(make_tool(
            "promote_from_mindlock",
            &format!("Promote a staged file from {0}/in or {0}/out to a target path.", mindlock_dir.display()),
            serde_json::json!({
                "type": "object",
                "properties": {
                    "source_path": { "type": "string", "description": format!("Absolute path under {0}/in or {0}/out", mindlock_dir.display()) },
                    "target_path": { "type": "string", "description": "Absolute destination path" }
                },
                "required": ["source_path", "target_path"]
            }),
        ));
    }

    if should_offer("request_review", "write_file") && context == ContextTier::Private {
        defs.push(make_tool(
            "request_review",
            &format!("Request security review for a {} artifact crossing before promotion.", mindlock_dir.display()),
            serde_json::json!({
                "type": "object",
                "properties": {
                    "source_path": { "type": "string", "description": format!("Absolute path under {0}/in, {0}/out, or {0}/work", mindlock_dir.display()) },
                    "target_path": { "type": "string", "description": "Absolute intended destination path" }
                },
                "required": ["source_path", "target_path"]
            }),
        ));
    }

    if should_offer("self_escalate", "write_file") && context == ContextTier::Private {
        defs.push(make_tool(
            "self_escalate",
            "Move a mindlock artifact to pending-zar for human approval.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "source_path": { "type": "string", "description": format!("Absolute path under {0}/in, {0}/out, or {0}/work", mindlock_dir.display()) },
                    "reason": { "type": "string", "description": "Short reason for escalation" }
                },
                "required": ["source_path"]
            }),
        ));
    }

    defs
}

/// Parse an LLM tool call into a VeriCore Action.
///
/// Wraps the primitive action in a ToolAction so the gate chain can check tool identity.
/// Note: skill_name is always None in Phase 1 (VeriCore doesn't have skills yet).
/// Do not enable gate_skill_identity until skills are wired in a future phase.
pub fn parse_tool_call(call: &ToolCall) -> Action {
    let inner = match call.name.as_str() {
        "read_file" => {
            let path = call.arguments["path"].as_str().unwrap_or("");
            Action::ReadFile {
                path: PathBuf::from(path),
            }
        }
        "write_file" => {
            let path = call.arguments["path"].as_str().unwrap_or("");
            let content = call.arguments["content"].as_str().unwrap_or("");
            Action::WriteFile {
                path: PathBuf::from(path),
                content: content.to_string(),
            }
        }
        "list_dir" => {
            let path = call.arguments["path"].as_str().unwrap_or(".");
            Action::ListDir {
                path: PathBuf::from(path),
            }
        }
        "exec" => {
            let command = call.arguments["command"].as_str().unwrap_or("");
            Action::Exec {
                command: command.to_string(),
            }
        }
        "web_fetch" => {
            let url = call.arguments["url"].as_str().unwrap_or("");
            let (host, path) = parse_url_parts(url);
            Action::WebFetch { host, path }
        }
        "promote_from_mindlock" => {
            let source_path = call.arguments["source_path"].as_str().unwrap_or("");
            let target_path = call.arguments["target_path"].as_str().unwrap_or("");
            Action::PromoteFromMindlock {
                source_path: PathBuf::from(source_path),
                target_path: PathBuf::from(target_path),
            }
        }
        "request_review" => {
            let source_path = call.arguments["source_path"].as_str().unwrap_or("");
            let target_path = call.arguments["target_path"].as_str().unwrap_or("");
            Action::RequestReview {
                source_path: PathBuf::from(source_path),
                target_path: PathBuf::from(target_path),
            }
        }
        "self_escalate" => {
            let source_path = call.arguments["source_path"].as_str().unwrap_or("");
            let reason = call.arguments["reason"]
                .as_str()
                .unwrap_or("self escalation requested");
            Action::SelfEscalate {
                source_path: PathBuf::from(source_path),
                reason: reason.to_string(),
            }
        }
        _ => Action::NoOp {
            reason: format!("unknown tool: {}", call.name),
        },
    };

    // Wrap in ToolAction for tool identity gating.
    // skill_name is None: VeriCore Phase 1 has no skill concept.
    Action::ToolAction {
        tool_name: call.name.clone(),
        skill_name: None,
        action: Box::new(inner),
    }
}

fn parse_url_parts(url: &str) -> (String, String) {
    let without_scheme = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url);
    let (host_port, path) = match without_scheme.find('/') {
        Some(i) => (&without_scheme[..i], &without_scheme[i..]),
        None => (without_scheme, "/"),
    };
    let host = host_port.split(':').next().unwrap_or(host_port);
    (host.to_string(), path.to_string())
}

fn make_tool(name: &str, description: &str, parameters: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "type": "function",
        "function": {
            "name": name,
            "description": description,
            "parameters": parameters,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_read_file() {
        let call = ToolCall {
            id: "1".into(),
            name: "read_file".into(),
            arguments: serde_json::json!({"path": "/home/test/file.txt"}),
        };
        let action = parse_tool_call(&call);
        assert_eq!(action.kind(), "read_file");
        assert_eq!(action.tool_name(), Some("read_file"));
    }

    #[test]
    fn parse_exec() {
        let call = ToolCall {
            id: "2".into(),
            name: "exec".into(),
            arguments: serde_json::json!({"command": "ls -la"}),
        };
        let action = parse_tool_call(&call);
        assert_eq!(action.kind(), "exec");
    }

    #[test]
    fn parse_web_fetch_extracts_host() {
        let call = ToolCall {
            id: "3".into(),
            name: "web_fetch".into(),
            arguments: serde_json::json!({"url": "https://api.openrouter.ai/v1/models"}),
        };
        let action = parse_tool_call(&call);
        assert_eq!(action.kind(), "web_fetch");
        match action.executable_action() {
            Action::WebFetch { host, path } => {
                assert_eq!(host, "api.openrouter.ai");
                assert_eq!(path, "/v1/models");
            }
            _ => panic!("expected WebFetch"),
        }
    }

    #[test]
    fn parse_unknown_tool() {
        let call = ToolCall {
            id: "4".into(),
            name: "launch_missiles".into(),
            arguments: serde_json::json!({}),
        };
        let action = parse_tool_call(&call);
        assert_eq!(action.kind(), "noop");
    }

    #[test]
    fn url_parsing() {
        let (h, p) = parse_url_parts("https://example.com/path/to/thing");
        assert_eq!(h, "example.com");
        assert_eq!(p, "/path/to/thing");

        let (h, p) = parse_url_parts("http://localhost:8080/api");
        assert_eq!(h, "localhost");
        assert_eq!(p, "/api");

        let (h, p) = parse_url_parts("https://example.com");
        assert_eq!(h, "example.com");
        assert_eq!(p, "/");
    }

    #[test]
    fn action_kind_gating_filters_tool_offering() {
        use crate::policy::ContextRoots;
        use std::fs;

        let id = format!("vericore-tools-test-{}", std::process::id());
        let base = std::env::temp_dir().join(id);
        fs::create_dir_all(base.join("private")).unwrap();
        fs::create_dir_all(base.join("family")).unwrap();

        let roots = ContextRoots::new(&base, base.join("private"), base.join("family")).unwrap();
        let policy = GatePolicy::new(roots, vec!["openrouter.ai".into()]).with_tool_gating(
            true,
            vec!["read_file".into()], // public: only read_file
            vec!["*".into()],         // family: everything
            vec!["*".into()],         // private: everything
        );

        // Public should only get read_file and list_dir (both gate as read_file action kind)
        let public_tools = tool_definitions_for_context(&policy, ContextTier::Public, Path::new("/home/zarclaw/mindlock"));
        let public_names: Vec<&str> = public_tools
            .iter()
            .map(|t| t["function"]["name"].as_str().unwrap())
            .collect();
        assert!(public_names.contains(&"read_file"));
        assert!(public_names.contains(&"list_dir"));
        assert!(!public_names.contains(&"write_file"));
        assert!(!public_names.contains(&"exec"));
        assert!(!public_names.contains(&"web_fetch"));

        // Private should get everything (exec allowed via with_exec_in_private)
        let policy = policy.with_exec_in_private(true);
        let private_tools = tool_definitions_for_context(&policy, ContextTier::Private, Path::new("/home/zarclaw/mindlock"));
        let private_names: Vec<&str> = private_tools
            .iter()
            .map(|t| t["function"]["name"].as_str().unwrap())
            .collect();
        assert!(private_names.contains(&"read_file"));
        assert!(private_names.contains(&"write_file"));
        assert!(private_names.contains(&"exec"));
        assert!(private_names.contains(&"web_fetch"));
    }

    #[test]
    fn private_tools_include_boundary_review_tools() {
        use crate::policy::ContextRoots;
        use std::fs;

        let id = format!("vericore-tools-boundary-test-{}", std::process::id());
        let base = std::env::temp_dir().join(id);
        fs::create_dir_all(base.join("private")).unwrap();
        fs::create_dir_all(base.join("family")).unwrap();

        let roots = ContextRoots::new(&base, base.join("private"), base.join("family")).unwrap();
        let policy = GatePolicy::new(roots, vec![]).with_exec_in_private(true);

        let private_tools = tool_definitions_for_context(&policy, ContextTier::Private, Path::new("/home/zarclaw/mindlock"));
        let private_names: Vec<&str> = private_tools
            .iter()
            .map(|t| t["function"]["name"].as_str().unwrap())
            .collect();

        assert!(private_names.contains(&"request_review"));
        assert!(private_names.contains(&"self_escalate"));
        assert!(private_names.contains(&"promote_from_mindlock"));
    }

    #[test]
    fn tool_identity_gating_filters_tool_offering() {
        use crate::policy::ContextRoots;
        use std::fs;

        let id = format!("vericore-tools-id-test-{}", std::process::id());
        let base = std::env::temp_dir().join(id);
        fs::create_dir_all(base.join("private")).unwrap();
        fs::create_dir_all(base.join("family")).unwrap();

        let roots = ContextRoots::new(&base, base.join("private"), base.join("family")).unwrap();
        let policy = GatePolicy::new(roots, vec!["openrouter.ai".into()])
            .with_tool_identity_gating(
                true,
                vec!["read_file".into(), "web_fetch".into()], // public
                vec!["*".into()],                             // family
                vec!["*".into()],                             // private
            );

        let public_tools = tool_definitions_for_context(&policy, ContextTier::Public, Path::new("/home/zarclaw/mindlock"));
        let public_names: Vec<&str> = public_tools
            .iter()
            .map(|t| t["function"]["name"].as_str().unwrap())
            .collect();
        assert!(public_names.contains(&"read_file"));
        assert!(public_names.contains(&"web_fetch"));
        assert!(!public_names.contains(&"list_dir")); // not in tool identity allowlist
        assert!(!public_names.contains(&"write_file"));
        assert!(!public_names.contains(&"exec"));
    }
}
