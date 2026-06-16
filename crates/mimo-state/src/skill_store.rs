use std::{
    ffi::OsStr,
    fs,
    io::Read,
    path::{Path, PathBuf},
    sync::LazyLock,
    time::Duration,
};

use anyhow::{Context, Result, bail};
use reqwest::{Url, blocking::Client};

use mimo_config::AppConfig;

const MAX_SKILL_SIZE_BYTES: u64 = 256 * 1024;

static SKILL_HTTP_CLIENT: LazyLock<Client> = LazyLock::new(|| {
    Client::builder()
        .timeout(Duration::from_secs(20))
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() >= 3 {
                return attempt.error(std::io::Error::other("too many redirects"));
            }
            if let Some(host) = attempt.url().host_str()
                && !is_allowed_skill_host(host)
            {
                return attempt.error(std::io::Error::other("redirected to a disallowed host"));
            }
            attempt.follow()
        }))
        .build()
        .expect("failed to build shared HTTP client")
});

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
        let parsed = Url::parse(spec).with_context(|| format!("invalid URL: {spec}"))?;
        if !matches!(parsed.scheme(), "http" | "https") {
            bail!("only http(s) URLs are supported for skill installation");
        }
        if let Some(host) = parsed.host_str()
            && !is_allowed_skill_host(host)
        {
            bail!("internal/private hosts are not allowed for skill installation: {host}");
        }
        let response = SKILL_HTTP_CLIENT
            .get(spec)
            .send()
            .with_context(|| format!("failed to fetch {spec}"))?
            .error_for_status()
            .with_context(|| format!("failed to fetch {spec}"))?;
        let mut body = Vec::new();
        response
            .take(MAX_SKILL_SIZE_BYTES + 1)
            .read_to_end(&mut body)
            .with_context(|| format!("failed to read {spec}"))?;
        if body.len() > MAX_SKILL_SIZE_BYTES as usize {
            bail!(
                "skill content exceeds maximum size of {} bytes",
                MAX_SKILL_SIZE_BYTES
            );
        }
        let content = String::from_utf8(body)
            .with_context(|| format!("skill at {spec} is not valid UTF-8"))?;
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
    crate::write_string_atomic(&path, &content)?;
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

fn is_allowed_skill_host(host: &str) -> bool {
    mimo_protocol::is_allowed_remote_host(host)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use mimo_config::{AppConfig, ConfigValueSource};

    use super::{install_skill, is_allowed_skill_host, load_skill, uninstall_skill};

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
    fn rejects_private_and_local_skill_hosts() {
        assert!(!is_allowed_skill_host("localhost"));
        assert!(!is_allowed_skill_host("127.0.0.1"));
        assert!(!is_allowed_skill_host("10.1.2.3"));
        assert!(!is_allowed_skill_host("::1"));
        assert!(!is_allowed_skill_host("fe80::1"));
        assert!(!is_allowed_skill_host("fd12::1"));
        assert!(is_allowed_skill_host("example.com"));
    }

    #[test]
    fn installs_loads_and_uninstalls_local_skill() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = test_config(&dir);
        let source = dir.path().join("skill.md");
        fs::write(
            &source,
            "# Rust Helper\n\nPrefer concise Rust diffs and safe file edits.\n",
        )
        .expect("write source skill");

        let installed =
            install_skill(&config, source.to_str().expect("utf8 path")).expect("install skill");
        assert_eq!(installed.name, "rust-helper");

        let loaded = load_skill(&config, "rust-helper").expect("load skill");
        assert!(loaded.content.contains("Prefer concise Rust diffs"));

        let removed = uninstall_skill(&config, "rust-helper").expect("uninstall skill");
        assert!(!removed.exists());
    }
}
