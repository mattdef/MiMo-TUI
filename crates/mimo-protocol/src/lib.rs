use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChatMessage {
    pub role: Role,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
}

impl ChatMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: Role::System,
            content: content.into(),
            tool_call_id: None,
            tool_calls: None,
        }
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: content.into(),
            tool_call_id: None,
            tool_calls: None,
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: content.into(),
            tool_call_id: None,
            tool_calls: None,
        }
    }

    pub fn assistant_with_tool_calls(
        content: impl Into<String>,
        tool_calls: Vec<ToolCall>,
    ) -> Self {
        Self {
            role: Role::Assistant,
            content: content.into(),
            tool_call_id: None,
            tool_calls: Some(tool_calls),
        }
    }

    pub fn tool(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: Role::Tool,
            content: content.into(),
            tool_call_id: Some(tool_call_id.into()),
            tool_calls: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub function: ToolFunction,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolFunction {
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiTool {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub function: ApiToolFunction,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiToolFunction {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

/// Returns `false` if `host` is `localhost` or resolves to a loopback,
/// private, link-local, unique-local, multicast, or unspecified IP address.
/// This is intentionally conservative: plain hostnames are allowed because
/// DNS resolution is not performed here.
pub fn is_allowed_remote_host(host: &str) -> bool {
    if host.eq_ignore_ascii_case("localhost") {
        return false;
    }
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        return !ip.is_loopback()
            && !ip.is_unspecified()
            && !ip.is_multicast()
            && match ip {
                std::net::IpAddr::V4(v4) => !v4.is_private() && !v4.is_link_local(),
                std::net::IpAddr::V6(v6) => {
                    let octets = v6.octets();
                    let is_unique_local = matches!(octets[0], 0xfc | 0xfd);
                    let is_link_or_site_local = octets[0] == 0xfe;
                    !is_unique_local && !is_link_or_site_local
                }
            };
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_message_serializes_correctly() {
        let msg = ChatMessage::system("You are helpful.");
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"role\":\"system\""));
        assert!(json.contains("\"content\":\"You are helpful.\""));
        assert!(!json.contains("tool_call_id"));
        assert!(!json.contains("tool_calls"));
    }

    #[test]
    fn user_message_serializes_correctly() {
        let msg = ChatMessage::user("Hello");
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"role\":\"user\""));
        assert!(json.contains("\"content\":\"Hello\""));
    }

    #[test]
    fn assistant_message_deserializes_correctly() {
        let json = r#"{"role":"assistant","content":"Hi there"}"#;
        let msg: ChatMessage = serde_json::from_str(json).unwrap();
        assert_eq!(msg.role, Role::Assistant);
        assert_eq!(msg.content, "Hi there");
        assert!(msg.tool_call_id.is_none());
        assert!(msg.tool_calls.is_none());
    }

    #[test]
    fn tool_message_includes_call_id() {
        let msg = ChatMessage::tool("call_abc", "result data");
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"role\":\"tool\""));
        assert!(json.contains("\"tool_call_id\":\"call_abc\""));
        assert!(json.contains("\"content\":\"result data\""));
        assert!(!json.contains("tool_calls"));
    }

    #[test]
    fn assistant_with_tool_calls_serializes() {
        let tc = ToolCall {
            id: "call_1".to_string(),
            kind: "function".to_string(),
            function: ToolFunction {
                name: "read_file".to_string(),
                arguments: "{\"path\":\"src/main.rs\"}".to_string(),
            },
        };
        let msg = ChatMessage::assistant_with_tool_calls("Let me read that file", vec![tc]);
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"role\":\"assistant\""));
        assert!(json.contains("tool_calls"));
        assert!(json.contains("\"id\":\"call_1\""));
        assert!(json.contains("\"name\":\"read_file\""));
    }

    #[test]
    fn empty_content_skipped_in_serialization() {
        let msg = ChatMessage {
            role: Role::Assistant,
            content: String::new(),
            tool_call_id: None,
            tool_calls: Some(vec![ToolCall {
                id: "call_1".to_string(),
                kind: "function".to_string(),
                function: ToolFunction {
                    name: "read".to_string(),
                    arguments: "{}".to_string(),
                },
            }]),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(!json.contains("\"content\":\"\""));
        assert!(json.contains("tool_calls"));
    }

    #[test]
    fn role_serialization_is_lowercase() {
        assert_eq!(serde_json::to_string(&Role::System).unwrap(), "\"system\"");
        assert_eq!(serde_json::to_string(&Role::User).unwrap(), "\"user\"");
        assert_eq!(
            serde_json::to_string(&Role::Assistant).unwrap(),
            "\"assistant\""
        );
        assert_eq!(serde_json::to_string(&Role::Tool).unwrap(), "\"tool\"");
    }

    #[test]
    fn rejects_local_and_private_hosts() {
        assert!(!is_allowed_remote_host("localhost"));
        assert!(!is_allowed_remote_host("127.0.0.1"));
        assert!(!is_allowed_remote_host("10.0.0.5"));
        assert!(!is_allowed_remote_host("169.254.1.10"));
        assert!(!is_allowed_remote_host("::1"));
        assert!(!is_allowed_remote_host("fe80::1"));
        assert!(!is_allowed_remote_host("fd00::1"));
    }

    #[test]
    fn allows_public_hosts() {
        assert!(is_allowed_remote_host("example.com"));
        assert!(is_allowed_remote_host("2606:2800:220:1:248:1893:25c8:1946"));
    }

    #[test]
    fn api_tool_serializes_with_type_field() {
        let tool = ApiTool {
            kind: "function",
            function: ApiToolFunction {
                name: "read_file".to_string(),
                description: "Read a file".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": {"type": "string"}
                    }
                }),
            },
        };
        let json = serde_json::to_string(&tool).unwrap();
        assert!(json.contains("\"type\":\"function\""));
        assert!(json.contains("\"name\":\"read_file\""));
        assert!(json.contains("\"description\":\"Read a file\""));
    }
}
