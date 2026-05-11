use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::shell::{SharedShellManager, new_shared_shell_manager};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum ApprovalRequirement {
    #[default]
    Auto,
    Prompt,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ToolKind {
    FileRead,
    FileWrite,
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
            ToolKind::FileRead => ApprovalRequirement::Auto,
            ToolKind::FileWrite | ToolKind::Shell => ApprovalRequirement::Prompt,
        }
    }

    fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult>;
}

#[derive(Clone)]
pub struct ToolContext {
    pub workspace_root: PathBuf,
    pub shell_manager: SharedShellManager,
}

impl ToolContext {
    pub fn new(workspace_root: impl Into<PathBuf>) -> Self {
        let workspace_root = workspace_root.into();
        Self {
            shell_manager: new_shared_shell_manager(workspace_root.clone()),
            workspace_root,
        }
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
