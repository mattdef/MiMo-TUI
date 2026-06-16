use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use mimo_config::AppConfig;
use serde::{Deserialize, Serialize};

use crate::AppMode;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Queued,
    Running,
    Completed,
    Failed,
    Cancelled,
    Interrupted,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedTask {
    pub id: String,
    pub prompt: String,
    pub model: String,
    #[serde(default)]
    pub routed_model: Option<String>,
    pub mode: AppMode,
    pub status: TaskStatus,
    pub created_at_epoch: u64,
    pub updated_at_epoch: u64,
    #[serde(default)]
    pub started_at_epoch: Option<u64>,
    #[serde(default)]
    pub finished_at_epoch: Option<u64>,
    #[serde(default)]
    pub assistant_output: String,
    #[serde(default)]
    pub activity_log: Vec<String>,
    #[serde(default)]
    pub error: Option<String>,
}

pub fn tasks_path(config: &AppConfig) -> PathBuf {
    config
        .config_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("tasks.json")
}

pub fn load_tasks(config: &AppConfig) -> Result<Vec<SavedTask>> {
    let path = tasks_path(config);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let content =
        fs::read_to_string(&path).with_context(|| format!("failed to read {}", path.display()))?;
    let mut tasks = serde_json::from_str::<Vec<SavedTask>>(&content)
        .with_context(|| format!("failed to parse {}", path.display()))?;
    normalize_loaded_tasks(&mut tasks);
    sort_tasks(&mut tasks);
    Ok(tasks)
}

pub fn save_tasks(config: &AppConfig, tasks: &[SavedTask]) -> Result<PathBuf> {
    let path = tasks_path(config);
    let content = serde_json::to_string_pretty(tasks).context("failed to encode task store")?;
    crate::write_string_atomic(&path, &content)?;
    Ok(path)
}

pub fn next_task_id(tasks: &[SavedTask]) -> String {
    let timestamp = now_epoch();
    let mut index = 1usize;
    loop {
        let candidate = format!("task-{timestamp}-{index}");
        if tasks.iter().all(|task| task.id != candidate) {
            return candidate;
        }
        index += 1;
    }
}

pub fn now_epoch() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn normalize_loaded_tasks(tasks: &mut [SavedTask]) {
    for task in tasks {
        if matches!(task.status, TaskStatus::Queued | TaskStatus::Running) {
            task.status = TaskStatus::Interrupted;
            task.finished_at_epoch = Some(now_epoch());
            task.updated_at_epoch = now_epoch();
            task.activity_log
                .push("Task interrupted because MiMo-TUI was restarted.".to_string());
            if task.error.is_none() {
                task.error = Some("Interrupted by application restart".to_string());
            }
        }
    }
}

pub fn sort_tasks(tasks: &mut [SavedTask]) {
    tasks.sort_by(|left, right| {
        right
            .updated_at_epoch
            .cmp(&left.updated_at_epoch)
            .then_with(|| right.created_at_epoch.cmp(&left.created_at_epoch))
            .then_with(|| left.id.cmp(&right.id))
    });
}

#[cfg(test)]
mod tests {
    use mimo_config::{AppConfig, ConfigValueSource};

    use super::{SavedTask, TaskStatus, load_tasks, next_task_id, now_epoch, save_tasks};

    fn test_config(dir: &tempfile::TempDir) -> AppConfig {
        AppConfig {
            api_key: None,
            base_url: "https://example.test/v1".to_string(),
            model: "mimo-v2-flash".to_string(),
            temperature: 0.2,
            system_prompt: "test".to_string(),
            config_path: dir.path().join("config.toml"),
            api_key_source: ConfigValueSource::Default,
            base_url_source: ConfigValueSource::Default,
            model_source: ConfigValueSource::Default,
            temperature_source: ConfigValueSource::Default,
            system_prompt_source: ConfigValueSource::Default,
        }
    }

    #[test]
    fn persists_and_reloads_tasks() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = test_config(&dir);
        let tasks = vec![SavedTask {
            id: "task-1".to_string(),
            prompt: "Summarize the repo".to_string(),
            model: "mimo-v2-flash".to_string(),
            routed_model: None,
            mode: crate::AppMode::Agent,
            status: TaskStatus::Completed,
            created_at_epoch: now_epoch(),
            updated_at_epoch: now_epoch(),
            started_at_epoch: Some(now_epoch()),
            finished_at_epoch: Some(now_epoch()),
            assistant_output: "Done".to_string(),
            activity_log: vec!["Started".to_string()],
            error: None,
        }];

        save_tasks(&config, &tasks).expect("save");
        let loaded = load_tasks(&config).expect("load");
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].assistant_output, "Done");
    }

    #[test]
    fn reload_marks_running_tasks_interrupted() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = test_config(&dir);
        let tasks = vec![SavedTask {
            id: "task-1".to_string(),
            prompt: "Background run".to_string(),
            model: "mimo-v2-flash".to_string(),
            routed_model: None,
            mode: crate::AppMode::Yolo,
            status: TaskStatus::Running,
            created_at_epoch: now_epoch(),
            updated_at_epoch: now_epoch(),
            started_at_epoch: Some(now_epoch()),
            finished_at_epoch: None,
            assistant_output: String::new(),
            activity_log: Vec::new(),
            error: None,
        }];

        save_tasks(&config, &tasks).expect("save");
        let loaded = load_tasks(&config).expect("load");
        assert_eq!(loaded[0].status, TaskStatus::Interrupted);
    }

    #[test]
    fn generates_unique_task_ids() {
        let tasks = vec![SavedTask {
            id: "task-1-1".to_string(),
            prompt: String::new(),
            model: String::new(),
            routed_model: None,
            mode: crate::AppMode::Agent,
            status: TaskStatus::Queued,
            created_at_epoch: 0,
            updated_at_epoch: 0,
            started_at_epoch: None,
            finished_at_epoch: None,
            assistant_output: String::new(),
            activity_log: Vec::new(),
            error: None,
        }];
        assert!(next_task_id(&tasks).starts_with("task-"));
    }
}
