use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum ApprovalMode {
    #[default]
    Prompt,
    ReadOnly,
    Auto,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ToolKind {
    Shell,
    FileRead,
    FileWrite,
    Git,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ToolStatus {
    Planned,
    PendingApproval,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolRequest {
    pub kind: ToolKind,
    pub summary: String,
    pub status: ToolStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ToolRuntime {
    pub approval_mode: ApprovalMode,
    pub pending: Vec<ToolRequest>,
}

impl ToolRuntime {
    pub fn summary(&self) -> String {
        format!(
            "{} pending, approvals {:?}",
            self.pending.len(),
            self.approval_mode
        )
    }
}
