use anyhow::{Context, Result, anyhow, bail};
use futures_util::StreamExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};

use crate::config::AppConfig;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: Role,
    pub content: String,
}

impl ChatMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: Role::System,
            content: content.into(),
        }
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: content.into(),
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: content.into(),
        }
    }
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
}

#[derive(Debug, Deserialize)]
struct ChatChunk {
    choices: Vec<Choice>,
}

#[derive(Debug, Deserialize)]
struct Choice {
    delta: Delta,
}

#[derive(Debug, Deserialize)]
struct Delta {
    content: Option<String>,
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

    pub async fn stream_chat<F>(&self, messages: &[ChatMessage], mut on_delta: F) -> Result<()>
    where
        F: FnMut(&str) -> Result<()>,
    {
        let url = format!("{}/chat/completions", self.base_url);
        let request = ChatRequest {
            model: &self.model,
            messages,
            stream: true,
            temperature: self.temperature,
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

        let mut buffer = String::new();
        let mut stream = response.bytes_stream();

        while let Some(chunk) = stream.next().await {
            let chunk = chunk.context("failed to read MiMo stream chunk")?;
            buffer.push_str(&String::from_utf8_lossy(&chunk));

            while let Some(newline) = buffer.find('\n') {
                let line = buffer.drain(..=newline).collect::<String>();
                if handle_sse_line(line.trim(), &mut on_delta)? {
                    return Ok(());
                }
            }
        }

        if !buffer.trim().is_empty() {
            handle_sse_line(buffer.trim(), &mut on_delta)?;
        }

        Ok(())
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
}

fn model_ids_from_response(response: ModelListResponse) -> Vec<String> {
    response
        .data
        .into_iter()
        .map(|model| model.id.trim().to_string())
        .filter(|model| !model.is_empty())
        .collect()
}

fn handle_sse_line<F>(line: &str, on_delta: &mut F) -> Result<bool>
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
        if let Some(content) = choice.delta.content {
            on_delta(&content)?;
        }
    }

    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::{ModelListResponse, model_ids_from_response};

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
}
