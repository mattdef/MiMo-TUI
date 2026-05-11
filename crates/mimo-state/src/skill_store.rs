use std::{
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, Result, bail};
use reqwest::blocking::Client;

use mimo_config::AppConfig;

#[derive(Debug, Clone)]
pub struct InstalledSkill {
    pub name: String,
    pub description: String,
    pub path: PathBuf,
    pub content: String,
}

pub fn list_installed_skills(config: &AppConfig) -> Result<Vec<InstalledSkill>> {
    let dir = skills_dir(config);
    if !dir.exists() {
        return Ok(Vec::new());
    }

    let mut skills = fs::read_dir(&dir)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(OsStr::to_str) == Some("md"))
        .filter_map(|path| load_skill_path(&path).ok())
        .collect::<Vec<_>>();
    skills.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(skills)
}

pub fn load_skill(config: &AppConfig, name: &str) -> Result<InstalledSkill> {
    let normalized = normalize_skill_name(name);
    if normalized.is_empty() {
        bail!("skill name cannot be empty");
    }
    load_skill_path(&skill_path(config, &normalized))
}

pub fn install_skill(config: &AppConfig, spec: &str) -> Result<InstalledSkill> {
    let spec = spec.trim();
    if spec.is_empty() {
        bail!("skill source cannot be empty");
    }

    let (content, fallback_name) = if spec.starts_with("http://") || spec.starts_with("https://") {
        let response = Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(20))
            .build()
            .context("failed to create HTTP client")?
            .get(spec)
            .send()
            .with_context(|| format!("failed to fetch {spec}"))?
            .error_for_status()
            .with_context(|| format!("failed to fetch {spec}"))?;
        let content = response
            .text()
            .with_context(|| format!("failed to read {spec}"))?;
        let fallback = Path::new(spec)
            .file_stem()
            .and_then(OsStr::to_str)
            .unwrap_or("skill")
            .to_string();
        (content, fallback)
    } else {
        let path = PathBuf::from(spec);
        let content = fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        let fallback = path
            .file_stem()
            .and_then(OsStr::to_str)
            .unwrap_or("skill")
            .to_string();
        (content, fallback)
    };

    let name = infer_skill_name(&content, &fallback_name);
    let path = skill_path(config, &name);
    ensure_skills_dir(config)?;
    fs::write(&path, content).with_context(|| format!("failed to write {}", path.display()))?;
    load_skill_path(&path)
}

pub fn uninstall_skill(config: &AppConfig, name: &str) -> Result<PathBuf> {
    let normalized = normalize_skill_name(name);
    if normalized.is_empty() {
        bail!("skill name cannot be empty");
    }
    let path = skill_path(config, &normalized);
    if !path.exists() {
        bail!("skill '{normalized}' is not installed");
    }
    fs::remove_file(&path).with_context(|| format!("failed to remove {}", path.display()))?;
    Ok(path)
}

pub fn skills_path(config: &AppConfig) -> PathBuf {
    skills_dir(config)
}

fn load_skill_path(path: &Path) -> Result<InstalledSkill> {
    let content =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    let fallback_name = path
        .file_stem()
        .and_then(OsStr::to_str)
        .unwrap_or("skill")
        .to_string();
    let name = infer_skill_name(&content, &fallback_name);
    Ok(InstalledSkill {
        description: infer_description(&content),
        name,
        path: path.to_path_buf(),
        content,
    })
}

fn ensure_skills_dir(config: &AppConfig) -> Result<()> {
    fs::create_dir_all(skills_dir(config))
        .with_context(|| format!("failed to create {}", skills_dir(config).display()))
}

fn skills_dir(config: &AppConfig) -> PathBuf {
    config
        .config_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("skills")
}

fn skill_path(config: &AppConfig, name: &str) -> PathBuf {
    skills_dir(config).join(format!("{name}.md"))
}

fn infer_skill_name(content: &str, fallback: &str) -> String {
    let heading = content
        .lines()
        .map(str::trim)
        .find_map(|line| line.strip_prefix("# "))
        .unwrap_or(fallback);
    let normalized = normalize_skill_name(heading);
    if normalized.is_empty() {
        normalize_skill_name(fallback)
    } else {
        normalized
    }
}

fn infer_description(content: &str) -> String {
    content
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .find(|line| !line.starts_with("# "))
        .unwrap_or("No description")
        .chars()
        .take(120)
        .collect()
}

pub fn normalize_skill_name(value: &str) -> String {
    let mut normalized = String::new();
    let mut prev_dash = false;
    for ch in value.trim().chars() {
        if ch.is_ascii_alphanumeric() {
            normalized.push(ch.to_ascii_lowercase());
            prev_dash = false;
        } else if (ch == '-' || ch == '_' || ch.is_whitespace()) && !prev_dash {
            normalized.push('-');
            prev_dash = true;
        }
    }
    normalized.trim_matches('-').to_string()
}
