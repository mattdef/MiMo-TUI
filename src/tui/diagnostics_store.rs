use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::config::AppConfig;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticsStatus {
    Passed,
    Failed,
}

impl std::fmt::Display for DiagnosticsStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Passed => write!(f, "passed"),
            Self::Failed => write!(f, "failed"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DiagnosticsSnapshot {
    pub updated_at_epoch: u64,
    pub command: String,
    pub status: DiagnosticsStatus,
    pub summary: String,
    pub output: String,
    pub error_count: usize,
    pub warning_count: usize,
}

pub fn load_snapshot(config: &AppConfig) -> Result<Option<DiagnosticsSnapshot>> {
    let path = diagnostics_path(config);
    if !path.exists() {
        return Ok(None);
    }
    let contents =
        fs::read_to_string(&path).with_context(|| format!("failed to read {}", path.display()))?;
    let snapshot = serde_json::from_str::<DiagnosticsSnapshot>(&contents)
        .with_context(|| format!("failed to parse {}", path.display()))?;
    Ok(Some(snapshot))
}

pub fn save_snapshot(config: &AppConfig, snapshot: &DiagnosticsSnapshot) -> Result<PathBuf> {
    let path = diagnostics_path(config);
    ensure_parent_dir(&path)?;
    let contents = serde_json::to_string_pretty(snapshot)?;
    fs::write(&path, contents).with_context(|| format!("failed to write {}", path.display()))?;
    Ok(path)
}

pub fn clear_snapshot(config: &AppConfig) -> Result<PathBuf> {
    let path = diagnostics_path(config);
    if path.exists() {
        fs::remove_file(&path).with_context(|| format!("failed to remove {}", path.display()))?;
    }
    Ok(path)
}

pub fn diagnostics_path(config: &AppConfig) -> PathBuf {
    config
        .config_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("diagnostics.json")
}

pub fn now_epoch() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn ensure_parent_dir(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf};

    use crate::config::{AppConfig, ConfigValueSource};

    use super::{
        DiagnosticsSnapshot, DiagnosticsStatus, clear_snapshot, load_snapshot, now_epoch,
        save_snapshot,
    };

    fn test_config() -> AppConfig {
        let base = std::env::temp_dir().join(format!("mimo-tui-diagnostics-test-{}", now_epoch()));
        let _ = fs::remove_dir_all(&base);
        AppConfig {
            api_key: None,
            base_url: "https://example.test/v1".to_string(),
            model: "mimo-v2-flash".to_string(),
            temperature: 0.2,
            system_prompt: "test".to_string(),
            config_path: PathBuf::from(&base).join("config.toml"),
            api_key_source: ConfigValueSource::Default,
            base_url_source: ConfigValueSource::Default,
            model_source: ConfigValueSource::Default,
            temperature_source: ConfigValueSource::Default,
            system_prompt_source: ConfigValueSource::Default,
        }
    }

    #[test]
    fn persists_and_clears_snapshot() {
        let config = test_config();
        let snapshot = DiagnosticsSnapshot {
            updated_at_epoch: 1,
            command: "cargo check".to_string(),
            status: DiagnosticsStatus::Passed,
            summary: "Diagnostics passed".to_string(),
            output: "Finished".to_string(),
            error_count: 0,
            warning_count: 0,
        };
        save_snapshot(&config, &snapshot).expect("save snapshot");
        let loaded = load_snapshot(&config)
            .expect("load snapshot")
            .expect("snapshot should exist");
        assert_eq!(loaded, snapshot);

        clear_snapshot(&config).expect("clear snapshot");
        assert!(
            load_snapshot(&config)
                .expect("reload after clear")
                .is_none()
        );
        let _ = fs::remove_dir_all(config.config_path.parent().expect("config parent"));
    }
}
