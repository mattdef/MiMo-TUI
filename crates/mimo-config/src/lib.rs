use std::{
    env, fmt, fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use url::Url;

const DEFAULT_BASE_URL: &str = "https://api.xiaomimimo.com/v1";
const DEFAULT_MODEL: &str = "mimo-v2-flash";
const DEFAULT_TEMPERATURE: f32 = 0.2;
pub const AUTO_MODEL: &str = "auto";
const KNOWN_MIMO_MODELS: &[&str] = &["mimo-v2-flash", "mimo-v2.5", "mimo-v2.5-pro"];
const DEFAULT_PERMISSION_POLICY: PermissionPolicy = PermissionPolicy::Prompt;
const DEFAULT_SYSTEM_PROMPT: &str = r#"You are MiMo TUI, a terminal assistant specialised for Xiaomi MiMo models.
Answer concisely, preserve technical accuracy, and adapt to developer workflows.
When the user asks for code, prefer small, practical changes and explain tradeoffs."#;
const WORKSPACE_INSTRUCTIONS_MAX_BYTES: usize = 32 * 1024;
const WORKSPACE_INSTRUCTION_FILENAMES: [&str; 3] = ["AGENTS.md", "CLAUDE.md", "README.md"];

#[derive(Debug, Clone, Default)]
pub struct ConfigOverrides {
    pub api_key: Option<String>,
    pub base_url: Option<String>,
    pub model: Option<String>,
    pub temperature: Option<f32>,
    pub permissions: Option<PermissionPolicy>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigValueSource {
    Cli,
    Env,
    File,
    WorkspaceInstructions,
    Default,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PermissionPolicy {
    #[serde(alias = "read-only")]
    ReadOnly,
    #[default]
    Prompt,
    Auto,
}

impl fmt::Display for PermissionPolicy {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReadOnly => formatter.write_str("read_only"),
            Self::Prompt => formatter.write_str("prompt"),
            Self::Auto => formatter.write_str("auto"),
        }
    }
}

impl fmt::Display for ConfigValueSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cli => formatter.write_str("cli"),
            Self::Env => formatter.write_str("env"),
            Self::File => formatter.write_str("config file"),
            Self::WorkspaceInstructions => formatter.write_str("workspace instructions"),
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
    pub permissions: PermissionPolicy,
    pub system_prompt: String,
    pub config_path: PathBuf,
    pub api_key_source: ConfigValueSource,
    pub base_url_source: ConfigValueSource,
    pub model_source: ConfigValueSource,
    pub temperature_source: ConfigValueSource,
    pub permissions_source: ConfigValueSource,
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
    permissions: Option<PermissionPolicy>,
    #[serde(skip_serializing_if = "Option::is_none")]
    system_prompt: Option<String>,
}

#[derive(Debug, Clone)]
struct DiscoveredInstructions {
    path: PathBuf,
    content: String,
    truncated: bool,
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

        let (permissions, permissions_source) = {
            let (permissions, permissions_source) = pick_permission([
                (ConfigValueSource::Cli, overrides.permissions),
                (ConfigValueSource::File, file_config.permissions),
            ]);
            match permissions {
                Some(permissions) => (permissions, permissions_source),
                None => resolve_permissions(None),
            }
        };

        let (base_system_prompt, base_system_prompt_source) =
            pick_string([(ConfigValueSource::File, file_config.system_prompt)]);
        let workspace_root = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let (system_prompt, system_prompt_source) = resolve_system_prompt(
            base_system_prompt,
            base_system_prompt_source,
            &workspace_root,
        )?;

        Ok(Self {
            api_key,
            base_url,
            model,
            temperature,
            permissions,
            system_prompt,
            config_path,
            api_key_source,
            base_url_source,
            model_source,
            temperature_source,
            permissions_source,
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

fn resolve_system_prompt(
    base_system_prompt: Option<String>,
    base_system_prompt_source: ConfigValueSource,
    workspace_root: &Path,
) -> Result<(String, ConfigValueSource)> {
    let base_system_prompt =
        base_system_prompt.unwrap_or_else(|| DEFAULT_SYSTEM_PROMPT.to_string());
    match discover_workspace_instructions(workspace_root)? {
        Some(instructions) => Ok((
            append_workspace_instructions(base_system_prompt, &instructions),
            ConfigValueSource::WorkspaceInstructions,
        )),
        None => Ok((base_system_prompt, base_system_prompt_source)),
    }
}

fn resolve_permissions(
    file_permissions: Option<PermissionPolicy>,
) -> (PermissionPolicy, ConfigValueSource) {
    match file_permissions {
        Some(permissions) => (permissions, ConfigValueSource::File),
        None => (DEFAULT_PERMISSION_POLICY, ConfigValueSource::Default),
    }
}

fn pick_permission<const N: usize>(
    values: [(ConfigValueSource, Option<PermissionPolicy>); N],
) -> (Option<PermissionPolicy>, ConfigValueSource) {
    for (source, value) in values {
        if let Some(value) = value {
            return (Some(value), source);
        }
    }
    (None, ConfigValueSource::Default)
}

fn append_workspace_instructions(
    base_system_prompt: String,
    instructions: &DiscoveredInstructions,
) -> String {
    let filename = instructions
        .path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("workspace instructions");

    let mut system_prompt = base_system_prompt;
    system_prompt.push_str("\n\n---\n\n# Project instructions from ");
    system_prompt.push_str(filename);
    system_prompt.push_str("\n\n");
    system_prompt.push_str(&instructions.content);
    if instructions.truncated {
        system_prompt.push_str("\n\n(Note: workspace instructions truncated to 32 KiB.)");
    }
    system_prompt
}

fn discover_workspace_instructions(root: &Path) -> Result<Option<DiscoveredInstructions>> {
    for filename in WORKSPACE_INSTRUCTION_FILENAMES {
        let path = root.join(filename);
        if let Some(instructions) = discover_workspace_instruction(&path)? {
            return Ok(Some(instructions));
        }
    }
    Ok(None)
}

fn discover_workspace_instruction(path: &Path) -> Result<Option<DiscoveredInstructions>> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| format!("failed to inspect {}", path.display()));
        }
    };

    let file_type = metadata.file_type();
    if file_type.is_symlink() || !file_type.is_file() {
        return Ok(None);
    }

    let file = match open_workspace_instruction_file(path) {
        Ok(file) => file,
        Err(error) if is_symlink_open_error(&error) => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| format!("failed to open {}", path.display()));
        }
    };

    let (bytes, truncated) = read_workspace_instruction_bytes(file, path)?;
    let content = decode_workspace_instruction_bytes(&bytes, truncated);
    if content.trim().is_empty() {
        return Ok(None);
    }

    Ok(Some(DiscoveredInstructions {
        path: path.to_path_buf(),
        content,
        truncated,
    }))
}

fn open_workspace_instruction_file(path: &Path) -> io::Result<fs::File> {
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    options.open(path)
}

fn is_symlink_open_error(error: &io::Error) -> bool {
    #[cfg(unix)]
    {
        error.raw_os_error() == Some(libc::ELOOP)
    }

    #[cfg(not(unix))]
    {
        let _ = error;
        false
    }
}

fn read_workspace_instruction_bytes(file: fs::File, path: &Path) -> Result<(Vec<u8>, bool)> {
    let mut bytes = Vec::new();
    let mut reader = file.take((WORKSPACE_INSTRUCTIONS_MAX_BYTES + 1) as u64);
    reader
        .read_to_end(&mut bytes)
        .with_context(|| format!("failed to read {}", path.display()))?;
    let truncated = bytes.len() > WORKSPACE_INSTRUCTIONS_MAX_BYTES;
    Ok((bytes, truncated))
}

fn decode_workspace_instruction_bytes(bytes: &[u8], truncated: bool) -> String {
    let slice = if truncated {
        &bytes[..WORKSPACE_INSTRUCTIONS_MAX_BYTES]
    } else {
        bytes
    };

    match std::str::from_utf8(slice) {
        Ok(text) => text.to_owned(),
        Err(error) => match std::str::from_utf8(&slice[..error.valid_up_to()]) {
            Ok(text) => text.to_owned(),
            Err(_) => String::new(),
        },
    }
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
    use std::fs;

    use super::{
        ConfigValueSource, DEFAULT_SYSTEM_PROMPT, PermissionPolicy,
        WORKSPACE_INSTRUCTIONS_MAX_BYTES, discover_workspace_instructions, known_mimo_models,
        normalize_base_url, normalize_model_name, resolve_permissions, resolve_system_prompt,
    };

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

    #[test]
    fn normalize_base_url_accepts_http_and_trims_trailing_slash() {
        assert_eq!(
            normalize_base_url("https://example.test/v1/"),
            Some("https://example.test/v1".to_string())
        );
        assert_eq!(
            normalize_base_url("http://example.test/"),
            Some("http://example.test".to_string())
        );
    }

    #[test]
    fn normalize_base_url_rejects_non_http_schemes() {
        assert_eq!(normalize_base_url("ftp://example.test"), None);
        assert_eq!(normalize_base_url("not-a-url"), None);
    }

    #[test]
    fn workspace_instructions_display_label_is_human_readable() {
        assert_eq!(
            ConfigValueSource::WorkspaceInstructions.to_string(),
            "workspace instructions"
        );
    }

    #[test]
    fn permission_policy_display_is_human_readable() {
        assert_eq!(PermissionPolicy::ReadOnly.to_string(), "read_only");
        assert_eq!(PermissionPolicy::Prompt.to_string(), "prompt");
        assert_eq!(PermissionPolicy::Auto.to_string(), "auto");
    }

    #[test]
    fn parses_permission_policy_from_toml() {
        let file_config: super::FileConfig =
            toml::from_str(r#"permissions = "read_only""#).expect("parse config");
        assert_eq!(file_config.permissions, Some(PermissionPolicy::ReadOnly));
    }

    #[test]
    fn defaults_permission_policy_to_prompt_when_missing() {
        let (permissions, source) = resolve_permissions(None);

        assert_eq!(permissions, PermissionPolicy::Prompt);
        assert_eq!(source, ConfigValueSource::Default);
    }

    #[test]
    fn uses_file_permission_policy_when_configured() {
        let (permissions, source) = resolve_permissions(Some(PermissionPolicy::Auto));

        assert_eq!(permissions, PermissionPolicy::Auto);
        assert_eq!(source, ConfigValueSource::File);
    }

    #[test]
    fn discovers_workspace_instructions_in_priority_order() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(dir.path().join("AGENTS.md"), "AGENTS").expect("write AGENTS");
        fs::write(dir.path().join("CLAUDE.md"), "CLAUDE").expect("write CLAUDE");
        fs::write(dir.path().join("README.md"), "README").expect("write README");

        let discovered = discover_workspace_instructions(dir.path())
            .expect("discover instructions")
            .expect("instructions found");

        assert_eq!(
            discovered.path.file_name().and_then(|name| name.to_str()),
            Some("AGENTS.md")
        );
        assert_eq!(discovered.content, "AGENTS");
        assert!(!discovered.truncated);
    }

    #[test]
    fn falls_back_to_claude_when_agents_missing() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(dir.path().join("CLAUDE.md"), "CLAUDE").expect("write CLAUDE");

        let discovered = discover_workspace_instructions(dir.path())
            .expect("discover instructions")
            .expect("instructions found");

        assert_eq!(
            discovered.path.file_name().and_then(|name| name.to_str()),
            Some("CLAUDE.md")
        );
        assert_eq!(discovered.content, "CLAUDE");
    }

    #[test]
    fn falls_back_to_readme_when_only_readme_exists() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(dir.path().join("README.md"), "README").expect("write README");

        let discovered = discover_workspace_instructions(dir.path())
            .expect("discover instructions")
            .expect("instructions found");

        assert_eq!(
            discovered.path.file_name().and_then(|name| name.to_str()),
            Some("README.md")
        );
        assert_eq!(discovered.content, "README");
    }

    #[test]
    fn ignores_whitespace_only_workspace_instructions() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(dir.path().join("AGENTS.md"), "   \n\n").expect("write AGENTS");

        assert!(
            discover_workspace_instructions(dir.path())
                .expect("discover instructions")
                .is_none()
        );
    }

    #[test]
    fn returns_none_when_no_workspace_instruction_files_exist() {
        let dir = tempfile::tempdir().expect("tempdir");

        assert!(
            discover_workspace_instructions(dir.path())
                .expect("discover instructions")
                .is_none()
        );
    }

    #[test]
    fn appends_workspace_instructions_after_default_prompt() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(dir.path().join("AGENTS.md"), "Keep it short.").expect("write AGENTS");

        let (system_prompt, source) =
            resolve_system_prompt(None, ConfigValueSource::Default, dir.path())
                .expect("resolve prompt");

        assert!(system_prompt.starts_with(DEFAULT_SYSTEM_PROMPT));
        assert!(
            system_prompt
                .contains("\n\n---\n\n# Project instructions from AGENTS.md\n\nKeep it short.")
        );
        assert_eq!(source, ConfigValueSource::WorkspaceInstructions);
    }

    #[test]
    fn appends_workspace_instructions_after_config_prompt() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(dir.path().join("CLAUDE.md"), "Use the config prompt first.")
            .expect("write CLAUDE");

        let (system_prompt, source) = resolve_system_prompt(
            Some("Configured prompt".to_string()),
            ConfigValueSource::File,
            dir.path(),
        )
        .expect("resolve prompt");

        assert!(system_prompt.starts_with("Configured prompt"));
        assert!(system_prompt.contains(
            "\n\n---\n\n# Project instructions from CLAUDE.md\n\nUse the config prompt first."
        ));
        assert_eq!(source, ConfigValueSource::WorkspaceInstructions);
    }

    #[test]
    fn keeps_existing_source_when_no_workspace_instructions_exist() {
        let dir = tempfile::tempdir().expect("tempdir");

        let (system_prompt, source) = resolve_system_prompt(
            Some("Configured prompt".to_string()),
            ConfigValueSource::File,
            dir.path(),
        )
        .expect("resolve prompt");

        assert_eq!(system_prompt, "Configured prompt");
        assert_eq!(source, ConfigValueSource::File);
    }

    #[test]
    fn uses_default_prompt_when_no_workspace_instructions_exist() {
        let dir = tempfile::tempdir().expect("tempdir");

        let (system_prompt, source) =
            resolve_system_prompt(None, ConfigValueSource::Default, dir.path())
                .expect("resolve prompt");

        assert_eq!(system_prompt, DEFAULT_SYSTEM_PROMPT);
        assert_eq!(source, ConfigValueSource::Default);
    }

    #[test]
    fn truncates_workspace_instructions_to_32_kib() {
        let dir = tempfile::tempdir().expect("tempdir");
        let content = "a".repeat(WORKSPACE_INSTRUCTIONS_MAX_BYTES + 512);
        fs::write(dir.path().join("AGENTS.md"), content.as_bytes()).expect("write AGENTS");

        let (system_prompt, source) =
            resolve_system_prompt(None, ConfigValueSource::Default, dir.path())
                .expect("resolve prompt");

        let truncated_prefix = "a".repeat(WORKSPACE_INSTRUCTIONS_MAX_BYTES);
        let overflow_prefix = "a".repeat(WORKSPACE_INSTRUCTIONS_MAX_BYTES + 1);
        assert_eq!(source, ConfigValueSource::WorkspaceInstructions);
        assert!(system_prompt.contains(&truncated_prefix));
        assert!(system_prompt.contains("(Note: workspace instructions truncated to 32 KiB.)"));
        assert!(!system_prompt.contains(&overflow_prefix));
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinked_agents_file_and_falls_back() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().expect("tempdir");
        let target = dir.path().join("target.md");
        fs::write(&target, "SYMLINK TARGET").expect("write target");
        symlink(&target, dir.path().join("AGENTS.md")).expect("create symlink");
        fs::write(dir.path().join("CLAUDE.md"), "CLAUDE").expect("write CLAUDE");

        let discovered = discover_workspace_instructions(dir.path())
            .expect("discover instructions")
            .expect("instructions found");

        assert_eq!(
            discovered.path.file_name().and_then(|name| name.to_str()),
            Some("CLAUDE.md")
        );
        assert_eq!(discovered.content, "CLAUDE");
    }
}
