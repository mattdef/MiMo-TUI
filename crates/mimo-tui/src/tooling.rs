use mimo_config::{ConfigValueSource, PermissionPolicy};
use tokio::sync::oneshot;

use mimo_tools::ToolKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolStatus {
    PendingApproval,
    Running,
    Completed,
    Failed,
    Denied,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolRequest {
    pub id: String,
    pub name: String,
    pub kind: ToolKind,
    pub summary: String,
    pub status: ToolStatus,
}

pub struct PendingApproval {
    pub request: ToolRequest,
    responder: Option<oneshot::Sender<bool>>,
}

impl std::fmt::Debug for PendingApproval {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PendingApproval")
            .field("request", &self.request)
            .finish()
    }
}

impl PendingApproval {
    pub fn new(request: ToolRequest, responder: oneshot::Sender<bool>) -> Self {
        Self {
            request,
            responder: Some(responder),
        }
    }

    pub fn respond(mut self, approve: bool) {
        if let Some(responder) = self.responder.take() {
            let _ = responder.send(approve);
        }
    }
}

#[derive(Debug)]
pub struct ToolRuntime {
    pub permissions: PermissionPolicy,
    pub permissions_source: ConfigValueSource,
    pub active: Option<ToolRequest>,
    pub pending_approval: Option<PendingApproval>,
    pub history: Vec<ToolRequest>,
}

impl Default for ToolRuntime {
    fn default() -> Self {
        Self {
            permissions: PermissionPolicy::default(),
            permissions_source: ConfigValueSource::Default,
            active: None,
            pending_approval: None,
            history: Vec::new(),
        }
    }
}

impl ToolRuntime {
    pub fn summary(&self) -> String {
        let active = usize::from(self.active.is_some());
        let pending = usize::from(self.pending_approval.is_some());
        format!(
            "{active} running, {pending} pending, permissions {} ({})",
            self.permissions, self.permissions_source
        )
    }

    pub fn begin_approval(&mut self, request: ToolRequest, responder: oneshot::Sender<bool>) {
        self.pending_approval = Some(PendingApproval::new(request, responder));
    }

    pub fn approve_pending(&mut self, approve: bool) -> Option<ToolRequest> {
        let pending = self.pending_approval.take()?;
        let mut request = pending.request.clone();
        request.status = if approve {
            ToolStatus::Running
        } else {
            ToolStatus::Denied
        };
        pending.respond(approve);
        if approve {
            self.active = Some(request.clone());
        }
        Some(request)
    }

    pub fn start(&mut self, request: ToolRequest) {
        self.active = Some(request);
    }

    pub fn finish(&mut self, request: ToolRequest) {
        self.active = None;
        self.push_history(request);
    }

    pub fn clear_transient(&mut self) {
        self.active = None;
        self.pending_approval = None;
    }

    fn push_history(&mut self, request: ToolRequest) {
        self.history.insert(0, request);
        self.history.truncate(20);
    }
}
