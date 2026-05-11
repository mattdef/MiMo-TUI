use anyhow::{Context, Result, anyhow, bail};
use futures_util::StreamExt;
use mimo_config::{AUTO_MODEL, AppConfig, auto_route_model};
use mimo_protocol::{ApiTool, ChatMessage, ToolCall, ToolFunction};
use reqwest::Client;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone)]
pub struct AssistantResponse {
    pub content: String,
    pub tool_calls: Vec<ToolCall>,
}

#[derive(Debug, Clone)]
pub struct MimoClient {
    http: Client,
    api_key: String,
    base_url: String,
    model: String,
    temperature: f32,
}

#[derive(Debug, Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: &'a [ChatMessage],
    stream: bool,
    temperature: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<&'a [ApiTool]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<&'a str>,
}

#[derive(Debug, Deserialize)]
struct ChatChunk {
    choices: Vec<Choice>,
}

#[derive(Debug, Deserialize)]
struct Choice {
    #[serde(default)]
    delta: Delta,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct Delta {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<ToolCallDelta>>,
}

#[derive(Debug, Default, Deserialize)]
struct ToolCallDelta {
    index: usize,
    #[serde(default)]
    id: Option<String>,
    #[serde(rename = "type", default)]
    kind: Option<String>,
    #[serde(default)]
    function: Option<ToolFunctionDelta>,
}

#[derive(Debug, Default, Deserialize)]
struct ToolFunctionDelta {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arguments: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ModelListResponse {
    #[serde(default)]
    data: Vec<ModelInfo>,
}

#[derive(Debug, Deserialize)]
struct ModelInfo {
    id: String,
}

impl MimoClient {
    pub fn new(config: &AppConfig) -> Result<Self> {
        let api_key = config
            .api_key
            .clone()
            .context("MiMo API key is missing; set MIMO_API_KEY or add api_key to config.toml")?;

        Ok(Self {
            http: Client::new(),
            api_key,
            base_url: config.base_url.clone(),
            model: config.model.clone(),
            temperature: config.temperature,
        })
    }

    pub async fn stream_chat_completion<F>(
        &self,
        messages: &[ChatMessage],
        tools: &[ApiTool],
        mut on_delta: F,
    ) -> Result<AssistantResponse>
    where
        F: FnMut(&str) -> Result<()>,
    {
        let url = format!("{}/chat/completions", self.base_url);
        let resolved_model = self.resolve_model(messages);
        let request = ChatRequest {
            model: &resolved_model,
            messages,
            stream: true,
            temperature: self.temperature,
            tools: (!tools.is_empty()).then_some(tools),
            tool_choice: (!tools.is_empty()).then_some("auto"),
        };

        let response = self
            .http
            .post(url)
            .bearer_auth(&self.api_key)
            .json(&request)
            .send()
            .await
            .context("failed to send request to MiMo API")?;

        let status = response.status();
        if !status.is_success() {
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "<failed to read response body>".to_string());
            bail!("MiMo API returned {status}: {body}");
        }

        let mut content = String::new();
        let mut tool_calls = Vec::new();
        let mut finish_reason = None;
        let mut buffer = String::new();
        let mut stream = response.bytes_stream();

        while let Some(chunk) = stream.next().await {
            let chunk = chunk.context("failed to read MiMo stream chunk")?;
            buffer.push_str(&String::from_utf8_lossy(&chunk));

            while let Some(newline) = buffer.find('\n') {
                let line = buffer.drain(..=newline).collect::<String>();
                if handle_sse_line(
                    line.trim(),
                    &mut content,
                    &mut tool_calls,
                    &mut finish_reason,
                    &mut on_delta,
                )? {
                    return Ok(AssistantResponse {
                        content,
                        tool_calls,
                    });
                }
            }
        }

        if !buffer.trim().is_empty() {
            handle_sse_line(
                buffer.trim(),
                &mut content,
                &mut tool_calls,
                &mut finish_reason,
                &mut on_delta,
            )?;
        }

        Ok(AssistantResponse {
            content,
            tool_calls,
        })
    }

    pub async fn list_models(&self) -> Result<Vec<String>> {
        let url = format!("{}/models", self.base_url);
        let response = self
            .http
            .get(url)
            .bearer_auth(&self.api_key)
            .send()
            .await
            .context("failed to request MiMo model catalog")?;

        let status = response.status();
        if !status.is_success() {
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "<failed to read response body>".to_string());
            bail!("MiMo API returned {status}: {body}");
        }

        let response = response
            .json::<ModelListResponse>()
            .await
            .context("failed to parse MiMo model catalog response")?;

        Ok(model_ids_from_response(response))
    }

    pub fn resolve_model(&self, messages: &[ChatMessage]) -> String {
        if self.model != AUTO_MODEL {
            return self.model.clone();
        }
        auto_route_model(messages).to_string()
    }
}

fn model_ids_from_response(response: ModelListResponse) -> Vec<String> {
    response
        .data
        .into_iter()
        .map(|model| model.id.trim().to_string())
        .filter(|model| !model.is_empty())
        .collect()
}

fn handle_sse_line<F>(
    line: &str,
    content: &mut String,
    tool_calls: &mut Vec<ToolCall>,
    finish_reason: &mut Option<String>,
    on_delta: &mut F,
) -> Result<bool>
where
    F: FnMut(&str) -> Result<()>,
{
    if line.is_empty() || line.starts_with(':') {
        return Ok(false);
    }

    let Some(data) = line.strip_prefix("data:") else {
        return Ok(false);
    };

    let data = data.trim();
    if data == "[DONE]" {
        return Ok(true);
    }

    let chunk = serde_json::from_str::<ChatChunk>(data)
        .map_err(|source| anyhow!("failed to parse MiMo stream event: {source}; data={data}"))?;

    for choice in chunk.choices {
        if choice.finish_reason.is_some() {
            *finish_reason = choice.finish_reason;
        }
        if let Some(delta_content) = choice.delta.content {
            on_delta(&delta_content)?;
            content.push_str(&delta_content);
        }
        merge_tool_call_deltas(tool_calls, choice.delta.tool_calls.unwrap_or_default());
        if finish_reason.as_deref() == Some("tool_calls") {
            return Ok(true);
        }
    }

    Ok(false)
}

fn merge_tool_call_deltas(tool_calls: &mut Vec<ToolCall>, deltas: Vec<ToolCallDelta>) {
    for delta in deltas {
        while tool_calls.len() <= delta.index {
            tool_calls.push(ToolCall {
                id: String::new(),
                kind: "function".to_string(),
                function: ToolFunction {
                    name: String::new(),
                    arguments: String::new(),
                },
            });
        }

        let call = &mut tool_calls[delta.index];
        if let Some(id) = delta.id
            && call.id.is_empty()
        {
            call.id = id;
        }
        if let Some(kind) = delta.kind {
            call.kind = kind;
        }
        if let Some(function) = delta.function {
            if let Some(name) = function.name {
                call.function.name.push_str(&name);
            }
            if let Some(arguments) = function.arguments {
                call.function.arguments.push_str(&arguments);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ModelListResponse, ToolCall, ToolCallDelta, ToolFunction, ToolFunctionDelta,
        handle_sse_line, merge_tool_call_deltas, model_ids_from_response,
    };
    use mimo_config::auto_route_model;
    use mimo_protocol::ChatMessage;

    #[test]
    fn extracts_model_ids_from_openai_style_catalog() {
        let payload = r#"{
            "object": "list",
            "data": [
                {"id": "mimo-v2-flash", "object": "model"},
                {"id": " mimo-v2.5-pro "},
                {"id": ""}
            ]
        }"#;

        let response: ModelListResponse = serde_json::from_str(payload).expect("valid catalog");
        let models = model_ids_from_response(response);

        assert_eq!(models, vec!["mimo-v2-flash", "mimo-v2.5-pro"]);
    }

    #[test]
    fn merges_streamed_tool_call_deltas() {
        let mut tool_calls = Vec::new();
        merge_tool_call_deltas(
            &mut tool_calls,
            vec![
                ToolCallDelta {
                    index: 0,
                    id: Some("call_1".to_string()),
                    kind: Some("function".to_string()),
                    function: Some(ToolFunctionDelta {
                        name: Some("read_file".to_string()),
                        arguments: Some("{\"path\":\"src/".to_string()),
                    }),
                },
                ToolCallDelta {
                    index: 0,
                    id: None,
                    kind: None,
                    function: Some(ToolFunctionDelta {
                        name: None,
                        arguments: Some("main.rs\"}".to_string()),
                    }),
                },
            ],
        );

        assert_eq!(
            tool_calls,
            vec![ToolCall {
                id: "call_1".to_string(),
                kind: "function".to_string(),
                function: ToolFunction {
                    name: "read_file".to_string(),
                    arguments: "{\"path\":\"src/main.rs\"}".to_string(),
                },
            }]
        );
    }

    #[test]
    fn detects_tool_call_finish_reason() {
        let mut content = String::new();
        let mut tool_calls = Vec::new();
        let mut finish_reason = None;
        let done = handle_sse_line(
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"read_file","arguments":"{\"path\":\"src/main.rs\"}"}}]},"finish_reason":"tool_calls"}]}"#,
            &mut content,
            &mut tool_calls,
            &mut finish_reason,
            &mut |_| Ok(()),
        )
        .expect("line should parse");

        assert!(done);
        assert_eq!(finish_reason.as_deref(), Some("tool_calls"));
        assert_eq!(tool_calls.len(), 1);
    }

    #[test]
    fn accepts_null_tool_calls_in_stream_chunks() {
        let mut content = String::new();
        let mut tool_calls = Vec::new();
        let mut finish_reason = None;

        let done = handle_sse_line(
            r#"data: {"choices":[{"delta":{"role":"assistant","content":"","reasoning_content":null,"tool_calls":null},"finish_reason":null}]}"#,
            &mut content,
            &mut tool_calls,
            &mut finish_reason,
            &mut |_| Ok(()),
        )
        .expect("line should parse");

        assert!(!done);
        assert!(content.is_empty());
        assert!(tool_calls.is_empty());
        assert!(finish_reason.is_none());
    }

    #[test]
    fn auto_router_uses_pro_for_complex_turns() {
        let messages = vec![ChatMessage::user(
            "Please investigate this failing build, review the architecture, and propose a refactor plan.\n```\nerror[E0308]\n```",
        )];
        assert_eq!(auto_route_model(&messages), "mimo-v2.5-pro");
    }

    #[test]
    fn auto_router_uses_flash_for_short_turns() {
        let messages = vec![ChatMessage::user("Summarize this repo.")];
        assert_eq!(auto_route_model(&messages), "mimo-v2-flash");
    }
}
