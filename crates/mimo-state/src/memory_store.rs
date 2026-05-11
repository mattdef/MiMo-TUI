use std::{fs, path::PathBuf};

use anyhow::{Context, Result};

use mimo_config::AppConfig;

const MAX_MEMORY_NOTES: usize = 200;

pub fn memory_path(config: &AppConfig) -> PathBuf {
    config
        .config_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .join("memory.txt")
}

pub fn load_notes(config: &AppConfig) -> Result<Vec<String>> {
    let path = memory_path(config);
    if !path.exists() {
        return Ok(Vec::new());
    }

    let content =
        fs::read_to_string(&path).with_context(|| format!("failed to read {}", path.display()))?;
    Ok(parse_notes(&content))
}

pub fn show_memory(config: &AppConfig) -> Result<String> {
    let path = memory_path(config);
    let notes = load_notes(config)?;
    let mut output = format!("Memory file\n\nPath: {}", path.display());
    if notes.is_empty() {
        output.push_str("\n\nNo memory notes saved yet.");
        return Ok(output);
    }

    output.push_str("\n\n");
    for note in notes {
        output.push_str("- ");
        output.push_str(&note);
        output.push('\n');
    }
    Ok(output.trim_end().to_string())
}

pub fn append_note(config: &AppConfig, note: &str) -> Result<PathBuf> {
    let path = memory_path(config);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }

    let mut notes = load_notes(config)?;
    let note = note.trim();
    if note.is_empty() {
        return Ok(path);
    }
    if let Some(index) = notes.iter().position(|existing| existing == note) {
        notes.remove(index);
    }
    notes.push(note.to_string());
    if notes.len() > MAX_MEMORY_NOTES {
        let excess = notes.len() - MAX_MEMORY_NOTES;
        notes.drain(0..excess);
    }
    write_notes(&path, &notes)?;
    Ok(path)
}

pub fn clear_memory(config: &AppConfig) -> Result<PathBuf> {
    let path = memory_path(config);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    write_notes(&path, &[])?;
    Ok(path)
}

pub fn search_memory(config: &AppConfig, query: &str, max_results: usize) -> Result<Vec<String>> {
    let query = query.trim().to_ascii_lowercase();
    if query.is_empty() {
        return Ok(Vec::new());
    }

    Ok(load_notes(config)?
        .into_iter()
        .filter(|note| note.to_ascii_lowercase().contains(&query))
        .take(max_results)
        .collect())
}

fn write_notes(path: &PathBuf, notes: &[String]) -> Result<()> {
    let mut output = String::new();
    for note in notes {
        output.push_str("- ");
        output.push_str(note.trim());
        output.push('\n');
    }
    fs::write(path, output).with_context(|| format!("failed to write {}", path.display()))
}

fn parse_notes(content: &str) -> Vec<String> {
    content
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| line.strip_prefix("- ").unwrap_or(line).trim().to_string())
        .filter(|line| !line.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use mimo_config::{AppConfig, ConfigValueSource};

    use super::{append_note, clear_memory, load_notes, search_memory, show_memory};

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
    fn appends_and_searches_memory_notes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = test_config(&dir);

        append_note(&config, "Prefer concise Rust diffs").expect("append");
        append_note(&config, "Use cargo check before concluding").expect("append");

        let notes = load_notes(&config).expect("load");
        assert_eq!(notes.len(), 2);
        assert_eq!(
            search_memory(&config, "cargo", 10).expect("search"),
            vec!["Use cargo check before concluding".to_string()]
        );
    }

    #[test]
    fn clears_memory_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = test_config(&dir);

        append_note(&config, "Keep track of API defaults").expect("append");
        clear_memory(&config).expect("clear");

        assert!(load_notes(&config).expect("load").is_empty());
        let shown = show_memory(&config).expect("show");
        assert!(shown.contains("No memory notes saved yet"));
    }
}
