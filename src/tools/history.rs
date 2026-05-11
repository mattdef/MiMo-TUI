use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileSnapshot {
    pub path: String,
    #[serde(default)]
    pub previous_content: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceSnapshot {
    pub id: String,
    pub summary: String,
    pub created_at_epoch: u64,
    pub files: Vec<FileSnapshot>,
}

#[derive(Debug, Default)]
pub struct WorkspaceHistory {
    snapshots: Vec<WorkspaceSnapshot>,
    next_id: u64,
}

pub type SharedWorkspaceHistory = Arc<Mutex<WorkspaceHistory>>;

pub fn new_shared_workspace_history() -> SharedWorkspaceHistory {
    Arc::new(Mutex::new(WorkspaceHistory::default()))
}

impl WorkspaceHistory {
    pub fn record(
        &mut self,
        summary: impl Into<String>,
        created_at_epoch: u64,
        files: Vec<FileSnapshot>,
    ) -> Option<WorkspaceSnapshot> {
        if files.is_empty() {
            return None;
        }
        self.next_id = self.next_id.saturating_add(1);
        let snapshot = WorkspaceSnapshot {
            id: format!("snapshot-{}", self.next_id),
            summary: summary.into(),
            created_at_epoch,
            files,
        };
        self.snapshots.insert(0, snapshot.clone());
        self.snapshots.truncate(50);
        Some(snapshot)
    }

    pub fn list(&self) -> Vec<WorkspaceSnapshot> {
        self.snapshots.clone()
    }

    pub fn latest(&self) -> Option<WorkspaceSnapshot> {
        self.snapshots.first().cloned()
    }

    pub fn find(&self, id: &str) -> Option<WorkspaceSnapshot> {
        self.snapshots
            .iter()
            .find(|snapshot| snapshot.id == id)
            .cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::{FileSnapshot, WorkspaceHistory};

    #[test]
    fn records_snapshots_in_reverse_chronological_order() {
        let mut history = WorkspaceHistory::default();
        history.record(
            "First change",
            1,
            vec![FileSnapshot {
                path: "src/main.rs".to_string(),
                previous_content: Some("fn main() {}".to_string()),
            }],
        );
        history.record(
            "Second change",
            2,
            vec![FileSnapshot {
                path: "README.md".to_string(),
                previous_content: None,
            }],
        );

        let snapshots = history.list();
        assert_eq!(snapshots.len(), 2);
        assert_eq!(snapshots[0].summary, "Second change");
        assert_eq!(snapshots[1].summary, "First change");
    }
}
