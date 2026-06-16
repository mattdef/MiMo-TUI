use std::{fs, io::Write, path::Path};

use anyhow::{Context, Result};

pub mod diagnostics_store;
pub mod mcp_store;
pub mod memory_store;
pub mod models;
pub mod session_store;
pub mod skill_store;
pub mod task_store;

pub use models::{AppMode, FileAttachment, PlanItem};

pub(crate) fn ensure_parent_dir(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    Ok(())
}

pub(crate) fn write_string_atomic(path: &Path, contents: &str) -> Result<()> {
    ensure_parent_dir(path)?;

    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("state");
    let temp = path.with_file_name(format!(".{file_name}.tmp"));

    {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&temp)
            .with_context(|| format!("failed to open {}", temp.display()))?;
        file.write_all(contents.as_bytes())
            .with_context(|| format!("failed to write {}", temp.display()))?;
        file.flush()
            .with_context(|| format!("failed to flush {}", temp.display()))?;
    }

    #[cfg(windows)]
    if path.exists() {
        fs::remove_file(path).with_context(|| format!("failed to replace {}", path.display()))?;
    }

    fs::rename(&temp, path)
        .with_context(|| format!("failed to rename {} to {}", temp.display(), path.display()))
}
