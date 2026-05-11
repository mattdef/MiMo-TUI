use std::{env, fmt, fs, io::Write, path::PathBuf};

use anyhow::{Context, Result};
use reqwest::Url;
use serde::{Deserialize, Serialize};

const DEFAULT_BASE_URL: &str = "https://api.xiaomimimo.com/v1";
const DEFAULT_MODEL: &str = "mimo-v2-flash";
const DEFAULT_TEMPERATURE: f32 = 0.2;
pub const AUTO_MODEL: &str = "auto";
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigValueSource {
    Cli,
    Env,
    File,
    Default,
}

impl fmt::Display for ConfigValueSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cli => formatter.write_str("cli"),
            Self::Env => formatter.write_str("env"),
            Self::File => formatter.write_str("config file"),
            Self::Default => formatter.write_str("default"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub api_key: Option<String>,
    pub base_url: String,
    pub model: String,
    pub temperature: f32,
    pub system_prompt: String,
    pub config_path: PathBuf,
    pub api_key_source: ConfigValueSource,
    pub base_url_source: ConfigValueSource,
    pub model_source: ConfigValueSource,
    pub temperature_source: ConfigValueSource,
    pub system_prompt_source: ConfigValueSource,
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

        let (api_key, api_key_source) = pick_string([
            (ConfigValueSource::Cli, overrides.api_key),
            (ConfigValueSource::Env, env::var("MIMO_API_KEY").ok()),
            (ConfigValueSource::File, file_config.api_key),
        ]);

        let (base_url, base_url_source) = pick_string([
            (ConfigValueSource::Cli, overrides.base_url),
            (ConfigValueSource::Env, env::var("MIMO_BASE_URL").ok()),
            (ConfigValueSource::File, file_config.base_url),
        ]);
        let base_url = base_url
            .unwrap_or_else(|| DEFAULT_BASE_URL.to_string())
            .trim_end_matches('/')
            .to_string();

        let (model, model_source) = pick_string([
            (ConfigValueSource::Cli, overrides.model),
            (ConfigValueSource::Env, env::var("MIMO_MODEL").ok()),
            (ConfigValueSource::File, file_config.model),
        ]);
        let model = model.unwrap_or_else(|| DEFAULT_MODEL.to_string());

        let (temperature, temperature_source) = pick_temperature([
            (ConfigValueSource::Cli, overrides.temperature),
            (ConfigValueSource::Env, env_f32("MIMO_TEMPERATURE")?),
            (ConfigValueSource::File, file_config.temperature),
        ]);
        let temperature = temperature.unwrap_or(DEFAULT_TEMPERATURE);

        let (system_prompt, system_prompt_source) =
            pick_string([(ConfigValueSource::File, file_config.system_prompt)]);
        let system_prompt = system_prompt.unwrap_or_else(|| DEFAULT_SYSTEM_PROMPT.to_string());

        Ok(Self {
            api_key,
            base_url,
            model,
            temperature,
            system_prompt,
            config_path,
            api_key_source,
            base_url_source,
            model_source,
            temperature_source,
            system_prompt_source,
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
        let base_url = base_url.trim_end_matches('/').to_string();
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
    let mut models = vec![AUTO_MODEL.to_string()];
    models.extend(KNOWN_MIMO_MODELS.iter().map(|model| (*model).to_string()));
    let normalized_current = current_model.trim().to_ascii_lowercase();
    if !normalized_current.is_empty() && !models.iter().any(|model| model == &normalized_current) {
        models.push(normalized_current);
    }
    models
}

pub fn normalize_model_name(model: &str) -> Option<String> {
    let model = model.trim().to_ascii_lowercase();
    if model == AUTO_MODEL {
        return Some(model);
    }
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
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(meta) = fs::metadata(parent) {
                let mut perms = meta.permissions();
                if perms.mode() & 0o077 != 0 {
                    perms.set_mode(0o700);
                    let _ = fs::set_permissions(parent, perms);
                }
            }
        }
    }
    let contents = toml::to_string_pretty(&file_config).context("failed to encode config file")?;
    write_config_file_atomic(path, &contents)
}

fn write_config_file_atomic(path: &PathBuf, contents: &str) -> Result<()> {
    let temp = path.with_extension("tmp");
    {
        let mut opts = fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut file = opts
            .open(&temp)
            .with_context(|| format!("failed to open {}", temp.display()))?;
        file.write_all(contents.as_bytes())
            .with_context(|| format!("failed to write {}", temp.display()))?;
        file.flush().ok();
    }
    fs::rename(&temp, path)
        .with_context(|| format!("failed to rename {} to {}", temp.display(), path.display()))
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

fn pick_string<const N: usize>(
    values: [(ConfigValueSource, Option<String>); N],
) -> (Option<String>, ConfigValueSource) {
    for (source, value) in values {
        if let Some(value) = value {
            let trimmed = value.trim().to_string();
            if !trimmed.is_empty() {
                return (Some(trimmed), source);
            }
        }
    }
    (None, ConfigValueSource::Default)
}

fn pick_temperature<const N: usize>(
    values: [(ConfigValueSource, Option<f32>); N],
) -> (Option<f32>, ConfigValueSource) {
    for (source, value) in values {
        if let Some(value) = value
            && value.is_finite()
            && (0.0..=2.0).contains(&value)
        {
            return (Some(value), source);
        }
    }
    (None, ConfigValueSource::Default)
}

pub fn auto_route_model(messages: &[mimo_protocol::ChatMessage]) -> &'static str {
    let latest_user = messages
        .iter()
        .rev()
        .find(|message| message.role == mimo_protocol::Role::User)
        .map(|message| message.content.as_str())
        .unwrap_or_default();
    let transcript_chars = messages
        .iter()
        .map(|message| message.content.len())
        .sum::<usize>();
    let latest_lower = latest_user.to_ascii_lowercase();
    let complex_keywords = [
        "architecture",
        "debug",
        "error",
        "failure",
        "fix",
        "implement",
        "investigate",
        "migrate",
        "plan",
        "refactor",
        "review",
        "security",
        "test",
        "trace",
    ];
    let is_complex = latest_user.len() > 600
        || transcript_chars > 4_000
        || latest_user.matches('\n').count() > 8
        || latest_user.contains("```")
        || complex_keywords
            .iter()
            .any(|keyword| latest_lower.contains(keyword));
    if is_complex {
        "mimo-v2.5-pro"
    } else if latest_user.len() > 200 || transcript_chars > 1_500 {
        "mimo-v2.5"
    } else {
        "mimo-v2-flash"
    }
}

#[cfg(test)]
mod tests {
    use super::{known_mimo_models, normalize_model_name};

    #[test]
    fn known_model_catalog_includes_mimo_25_models() {
        let models = known_mimo_models("mimo-v2-flash");
        assert!(models.iter().any(|model| model == "auto"));
        assert!(models.iter().any(|model| model == "mimo-v2.5"));
        assert!(models.iter().any(|model| model == "mimo-v2.5-pro"));
    }

    #[test]
    fn normalize_model_name_accepts_auto() {
        assert_eq!(normalize_model_name("AUTO"), Some("auto".to_string()));
    }
}
