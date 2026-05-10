use std::{env, fs, path::PathBuf};

use anyhow::{Context, Result};
use reqwest::Url;
use serde::{Deserialize, Serialize};

const DEFAULT_BASE_URL: &str = "https://api.xiaomimimo.com/v1";
const DEFAULT_MODEL: &str = "mimo-v2-flash";
const DEFAULT_TEMPERATURE: f32 = 0.2;
const KNOWN_MIMO_MODELS: &[&str] = &["mimo-v2-flash", "mimo-v2.5", "mimo-v2.5-pro"];
const DEFAULT_SYSTEM_PROMPT: &str = r#"You are MiMo TUI, a terminal assistant specialised for Xiaomi MiMo models.
Answer concisely, preserve technical accuracy, and adapt to developer workflows.
When the user asks for code, prefer small, practical changes and explain tradeoffs."#;

#[derive(Debug, Clone, Default)]
pub struct ConfigOverrides {
    pub api_key: Option<String>,
    pub base_url: Option<String>,
    pub model: Option<String>,
    pub temperature: Option<f32>,
}

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub api_key: Option<String>,
    pub base_url: String,
    pub model: String,
    pub temperature: f32,
    pub system_prompt: String,
    pub config_path: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct FileConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    api_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    base_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    system_prompt: Option<String>,
}

impl AppConfig {
    pub fn load(overrides: ConfigOverrides) -> Result<Self> {
        let config_path = config_path();
        let file_config = read_file_config(&config_path)?;

        let api_key = first_non_empty([
            overrides.api_key,
            env::var("MIMO_API_KEY").ok(),
            file_config.api_key,
        ]);

        let base_url = first_non_empty([
            overrides.base_url,
            env::var("MIMO_BASE_URL").ok(),
            file_config.base_url,
        ])
        .unwrap_or_else(|| DEFAULT_BASE_URL.to_string())
        .trim_end_matches('/')
        .to_string();

        let model = first_non_empty([
            overrides.model,
            env::var("MIMO_MODEL").ok(),
            file_config.model,
        ])
        .unwrap_or_else(|| DEFAULT_MODEL.to_string());

        let temperature = overrides
            .temperature
            .or(env_f32("MIMO_TEMPERATURE")?)
            .or(file_config.temperature)
            .unwrap_or(DEFAULT_TEMPERATURE);

        let system_prompt = file_config
            .system_prompt
            .unwrap_or_else(|| DEFAULT_SYSTEM_PROMPT.to_string());

        Ok(Self {
            api_key,
            base_url,
            model,
            temperature,
            system_prompt,
            config_path,
        })
    }

    pub fn masked_api_key(&self) -> &'static str {
        if self.api_key.is_some() {
            "configured"
        } else {
            "missing"
        }
    }

    pub fn set_api_key(&mut self, api_key: Option<String>) -> Result<()> {
        update_file_config(&self.config_path, |file_config| {
            file_config.api_key = api_key.clone();
        })?;
        self.api_key = api_key;
        Ok(())
    }

    pub fn set_base_url(&mut self, base_url: String) -> Result<()> {
        update_file_config(&self.config_path, |file_config| {
            file_config.base_url = Some(base_url.clone());
        })?;
        self.base_url = base_url;
        Ok(())
    }

    pub fn set_model(&mut self, model: String) -> Result<()> {
        update_file_config(&self.config_path, |file_config| {
            file_config.model = Some(model.clone());
        })?;
        self.model = model;
        Ok(())
    }

    pub fn set_temperature(&mut self, temperature: f32) -> Result<()> {
        update_file_config(&self.config_path, |file_config| {
            file_config.temperature = Some(temperature);
        })?;
        self.temperature = temperature;
        Ok(())
    }
}

pub fn known_mimo_models(current_model: &str) -> Vec<String> {
    let mut models = KNOWN_MIMO_MODELS
        .iter()
        .map(|model| (*model).to_string())
        .collect::<Vec<_>>();
    let normalized_current = current_model.trim().to_ascii_lowercase();
    if !normalized_current.is_empty() && !models.iter().any(|model| model == &normalized_current) {
        models.push(normalized_current);
    }
    models
}

pub fn normalize_model_name(model: &str) -> Option<String> {
    let model = model.trim().to_ascii_lowercase();
    is_valid_model_name(&model).then_some(model)
}

pub fn is_valid_model_name(model: &str) -> bool {
    !model.is_empty()
        && model.starts_with("mimo-")
        && model
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | ':'))
}

pub fn normalize_base_url(base_url: &str) -> Option<String> {
    let base_url = base_url.trim();
    let parsed = Url::parse(base_url).ok()?;
    matches!(parsed.scheme(), "http" | "https").then(|| base_url.trim_end_matches('/').to_string())
}

fn config_path() -> PathBuf {
    if let Ok(path) = env::var("MIMO_TUI_CONFIG") {
        return PathBuf::from(path);
    }

    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("mimo-tui")
        .join("config.toml")
}

fn read_file_config(path: &PathBuf) -> Result<FileConfig> {
    if !path.exists() {
        return Ok(FileConfig::default());
    }

    let contents = fs::read_to_string(path)
        .with_context(|| format!("failed to read config file {}", path.display()))?;
    toml::from_str(&contents)
        .with_context(|| format!("failed to parse config file {}", path.display()))
}

fn update_file_config(path: &PathBuf, mutate: impl FnOnce(&mut FileConfig)) -> Result<()> {
    let mut file_config = read_file_config(path)?;
    mutate(&mut file_config);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create config directory {}", parent.display()))?;
    }
    let contents = toml::to_string_pretty(&file_config).context("failed to encode config file")?;
    fs::write(path, contents).with_context(|| format!("failed to write {}", path.display()))
}

fn env_f32(name: &str) -> Result<Option<f32>> {
    let Ok(value) = env::var(name) else {
        return Ok(None);
    };
    if value.trim().is_empty() {
        return Ok(None);
    }
    value
        .parse::<f32>()
        .map(Some)
        .with_context(|| format!("{name} must be a floating point number"))
}

fn first_non_empty(values: impl IntoIterator<Item = Option<String>>) -> Option<String> {
    values
        .into_iter()
        .flatten()
        .map(|value| value.trim().to_string())
        .find(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::known_mimo_models;

    #[test]
    fn known_model_catalog_includes_mimo_25_models() {
        let models = known_mimo_models("mimo-v2-flash");
        assert!(models.iter().any(|model| model == "mimo-v2.5"));
        assert!(models.iter().any(|model| model == "mimo-v2.5-pro"));
    }
}
