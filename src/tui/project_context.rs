use std::{env, fs, path::Path};

use anyhow::Result;

pub fn summarize_current_directory(max_depth: usize, max_entries: usize) -> Result<String> {
    let cwd = env::current_dir()?;
    let mut lines = vec![format!("Workspace: {}", cwd.display())];
    let mut entries_left = max_entries;
    collect_entries(&cwd, 0, max_depth, &mut entries_left, &mut lines)?;
    if entries_left == 0 {
        lines.push("... output truncated ...".to_string());
    }
    Ok(lines.join("\n"))
}

fn collect_entries(
    path: &Path,
    depth: usize,
    max_depth: usize,
    entries_left: &mut usize,
    lines: &mut Vec<String>,
) -> Result<()> {
    if depth >= max_depth || *entries_left == 0 {
        return Ok(());
    }

    let mut entries = fs::read_dir(path)?
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_str()
                .map(|name| !name.starts_with('.'))
                .unwrap_or(false)
        })
        .collect::<Vec<_>>();
    entries.sort_by_key(|entry| entry.file_name());

    for entry in entries {
        if *entries_left == 0 {
            break;
        }
        let file_type = entry.file_type()?;
        let indent = "  ".repeat(depth);
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let suffix = if file_type.is_dir() { "/" } else { "" };
        lines.push(format!("{indent}- {name}{suffix}"));
        *entries_left = entries_left.saturating_sub(1);
        if file_type.is_dir() {
            collect_entries(&entry.path(), depth + 1, max_depth, entries_left, lines)?;
        }
    }

    Ok(())
}
