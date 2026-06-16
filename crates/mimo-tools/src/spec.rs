use std::{
    path::{Component, Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{
    history::{
        FileSnapshot, SharedWorkspaceHistory, WorkspaceSnapshot, new_shared_workspace_history,
    },
    shell::{SharedShellManager, new_shared_shell_manager},
};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum ApprovalRequirement {
    Auto,
    #[default]
    Prompt,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ToolKind {
    FileRead,
    FileWrite,
    Search,
    Git,
    Network,
    Project,
    Shell,
}

#[derive(Debug, Clone)]
pub struct ToolResult {
    pub content: String,
    pub summary: String,
}

impl ToolResult {
    pub fn new(content: impl Into<String>, summary: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            summary: summary.into(),
        }
    }
}

pub trait ToolSpec: Send + Sync {
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    fn input_schema(&self) -> Value;
    fn kind(&self) -> ToolKind;
    fn summarize(&self, input: &Value) -> String;

    fn approval_requirement(&self) -> ApprovalRequirement {
        match self.kind() {
            ToolKind::FileRead | ToolKind::Search | ToolKind::Git | ToolKind::Project => {
                ApprovalRequirement::Auto
            }
            ToolKind::FileWrite | ToolKind::Network | ToolKind::Shell => {
                ApprovalRequirement::Prompt
            }
        }
    }

    fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult>;
}

#[derive(Clone)]
pub struct ToolContext {
    pub workspace_root: PathBuf,
    pub shell_manager: SharedShellManager,
    pub workspace_history: SharedWorkspaceHistory,
    pub(crate) cancellation: CancellationFlag,
}

impl ToolContext {
    pub fn new(workspace_root: impl Into<PathBuf>) -> Self {
        let workspace_root = workspace_root.into();
        Self {
            shell_manager: new_shared_shell_manager(workspace_root.clone()),
            workspace_history: new_shared_workspace_history(),
            cancellation: CancellationFlag::default(),
            workspace_root,
        }
    }

    pub fn child_operation(&self) -> Self {
        Self {
            workspace_root: self.workspace_root.clone(),
            shell_manager: Arc::clone(&self.shell_manager),
            workspace_history: Arc::clone(&self.workspace_history),
            cancellation: CancellationFlag::default(),
        }
    }

    pub fn cancel(&self) {
        self.cancellation.cancel();
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancellation.is_cancelled()
    }

    pub fn resolve_path(&self, raw: &str) -> Result<PathBuf> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            bail!("path cannot be empty");
        }

        let workspace = self
            .workspace_root
            .canonicalize()
            .with_context(|| format!("failed to resolve {}", self.workspace_root.display()))?;

        let candidate = if Path::new(trimmed).is_absolute() {
            PathBuf::from(trimmed)
        } else {
            workspace.join(trimmed)
        };

        ensure_no_symlink_component(&candidate)
            .with_context(|| format!("path contains a symlink: {}", candidate.display()))?;

        let normalized = if candidate.exists() {
            candidate
                .canonicalize()
                .with_context(|| format!("failed to resolve {}", candidate.display()))?
        } else {
            let mut ancestor = candidate.clone();
            let mut suffix = Vec::new();

            while !ancestor.exists() {
                let Some(name) = ancestor.file_name() else {
                    bail!("path escapes workspace: {}", candidate.display());
                };
                suffix.push(name.to_owned());
                let Some(parent) = ancestor.parent() else {
                    bail!("path escapes workspace: {}", candidate.display());
                };
                ancestor = parent.to_path_buf();
            }

            let mut normalized = ancestor
                .canonicalize()
                .with_context(|| format!("failed to resolve {}", ancestor.display()))?;
            for part in suffix.into_iter().rev() {
                normalized.push(part);
            }
            normalize_path(&normalized)
        };

        if !normalized.starts_with(&workspace) {
            bail!("path escapes workspace: {}", normalized.display());
        }

        Ok(normalized)
    }

    pub fn record_workspace_snapshot(
        &self,
        summary: impl Into<String>,
        files: Vec<FileSnapshot>,
    ) -> Result<Option<WorkspaceSnapshot>> {
        let mut history = self
            .workspace_history
            .lock()
            .map_err(|_| anyhow::anyhow!("workspace history is unavailable"))?;
        Ok(history.record(summary, unix_timestamp(), files))
    }

    pub fn list_workspace_snapshots(&self) -> Result<Vec<WorkspaceSnapshot>> {
        let history = self
            .workspace_history
            .lock()
            .map_err(|_| anyhow::anyhow!("workspace history is unavailable"))?;
        Ok(history.list())
    }

    pub fn restore_workspace_snapshot(&self, id: Option<&str>) -> Result<WorkspaceSnapshot> {
        let snapshot = {
            let history = self
                .workspace_history
                .lock()
                .map_err(|_| anyhow::anyhow!("workspace history is unavailable"))?;
            match id {
                Some(id) => history
                    .find(id)
                    .with_context(|| format!("workspace snapshot {id} not found"))?,
                None => history
                    .latest()
                    .context("no workspace snapshots available")?,
            }
        };

        let restore_targets = snapshot
            .files
            .iter()
            .map(|file| {
                let resolved = self.resolve_path(&file.path)?;
                let current_content = if resolved.exists() {
                    Some(std::fs::read_to_string(&resolved).with_context(|| {
                        format!("failed to read current {}", resolved.display())
                    })?)
                } else {
                    None
                };
                Ok(RestoreTarget {
                    path: file.path.clone(),
                    resolved,
                    target_content: file.previous_content.clone(),
                    current_content,
                })
            })
            .collect::<Result<Vec<_>>>()?;

        let mut applied = Vec::with_capacity(restore_targets.len());
        for target in &restore_targets {
            if let Err(error) = apply_restore_target(target) {
                rollback_restore_targets(&applied)
                    .with_context(|| format!("failed to rollback restore after: {error}"))?;
                return Err(error);
            }
            applied.push(target.clone());
        }

        Ok(snapshot)
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct CancellationFlag {
    cancelled: Arc<AtomicBool>,
}

impl CancellationFlag {
    pub(crate) fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }

    pub(crate) fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }
}

#[derive(Debug, Clone)]
struct RestoreTarget {
    path: String,
    resolved: PathBuf,
    target_content: Option<String>,
    current_content: Option<String>,
}

fn apply_restore_target(target: &RestoreTarget) -> Result<()> {
    match &target.target_content {
        Some(content) => {
            if let Some(parent) = target.resolved.parent() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("failed to create {}", parent.display()))?;
            }
            std::fs::write(&target.resolved, content)
                .with_context(|| format!("failed to restore {}", target.resolved.display()))?;
        }
        None => {
            if target.resolved.exists() {
                std::fs::remove_file(&target.resolved)
                    .with_context(|| format!("failed to remove {}", target.resolved.display()))?;
            }
        }
    }
    Ok(())
}

fn rollback_restore_targets(applied: &[RestoreTarget]) -> Result<()> {
    for target in applied.iter().rev() {
        let rollback_target = RestoreTarget {
            path: target.path.clone(),
            resolved: target.resolved.clone(),
            target_content: target.current_content.clone(),
            current_content: None,
        };
        apply_restore_target(&rollback_target)
            .with_context(|| format!("failed to rollback {}", target.resolved.display()))?;
    }
    Ok(())
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();

    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }

    normalized
}

/// Vérifie qu'aucun composant du chemin n'est un symlink.
///
/// # Sécurité
///
/// Cette vérification est **best-effort** et non une garantie de sécurité complète.
/// Elle protège contre les symlinks accidentels ou malveillants présents au moment
/// de la vérification, mais ne protège pas contre les attaques TOCTOU (Time-Of-Check
/// Time-Of-Use) où un attaquant modifierait le filesystem entre cette vérification
/// et l'ouverture du fichier.
///
/// Une protection TOCTOU-safe complète nécessiterait `openat2()` avec `RESOLVE_NO_SYMLINKS`
/// (Linux 5.6+), qui n'est pas disponible partout. Cette approche est un compromis
/// pragmatique qui couvre la majorité des cas d'usage.
///
/// La protection finale repose sur `O_NOFOLLOW` dans `read_file_safe` et `write_file_safe`
/// qui refusent d'ouvrir un symlink comme fichier cible.
fn ensure_no_symlink_component(path: &Path) -> Result<()> {
    let mut current = Some(path);
    while let Some(p) = current {
        match p.symlink_metadata() {
            Ok(meta) if meta.file_type().is_symlink() => {
                bail!("path contains a symlink: {}", p.display());
            }
            Ok(_) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => {
                return Err(err).with_context(|| format!("failed to inspect {}", p.display()));
            }
        }
        current = p.parent();
    }
    Ok(())
}

fn unix_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::ToolContext;

    #[test]
    fn resolves_relative_paths_inside_workspace() {
        let workspace = tempfile::tempdir().expect("tempdir");
        fs::create_dir_all(workspace.path().join("src")).expect("mkdir");
        fs::write(workspace.path().join("src/main.rs"), "fn main() {}").expect("write");
        let context = ToolContext::new(workspace.path());

        let path = context.resolve_path("src/main.rs").expect("resolved path");
        assert!(path.ends_with("src/main.rs"));
    }

    #[test]
    fn rejects_paths_that_escape_workspace() {
        let workspace = tempfile::tempdir().expect("tempdir");
        let context = ToolContext::new(workspace.path());

        let error = context
            .resolve_path("../outside.txt")
            .expect_err("should reject");
        assert!(error.to_string().contains("path escapes workspace"));
    }

    #[test]
    fn resolves_nonexistent_children_under_workspace() {
        let workspace = tempfile::tempdir().expect("tempdir");
        fs::create_dir_all(workspace.path().join("nested")).expect("mkdir");
        let context = ToolContext::new(workspace.path());

        let path = context
            .resolve_path("nested/new-file.txt")
            .expect("resolved path");
        assert!(path.ends_with("nested/new-file.txt"));
    }
}
