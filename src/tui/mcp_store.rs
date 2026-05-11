use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::config::AppConfig;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum McpTransport {
    Stdio,
    Http,
}

impl std::fmt::Display for McpTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Stdio => write!(f, "stdio"),
            Self::Http => write!(f, "http"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    pub name: String,
    pub enabled: bool,
    pub transport: McpTransport,
    #[serde(default)]
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub url: String,
}

impl McpServerConfig {
    pub fn summary(&self) -> String {
        match self.transport {
            McpTransport::Stdio => {
                let args = if self.args.is_empty() {
                    String::new()
                } else {
                    format!(" {}", self.args.join(" "))
                };
                format!("{}{}", self.command, args)
            }
            McpTransport::Http => self.url.clone(),
        }
    }
}

pub fn load_servers(config: &AppConfig) -> Result<Vec<McpServerConfig>> {
    let path = mcp_servers_path(config);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let contents =
        fs::read_to_string(&path).with_context(|| format!("failed to read {}", path.display()))?;
    let mut servers = serde_json::from_str::<Vec<McpServerConfig>>(&contents)
        .with_context(|| format!("failed to parse {}", path.display()))?;
    servers.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(servers)
}

pub fn find_server(config: &AppConfig, name: &str) -> Result<McpServerConfig> {
    let normalized = normalize_server_name(name);
    load_servers(config)?
        .into_iter()
        .find(|server| server.name == normalized)
        .with_context(|| format!("unknown MCP server '{normalized}'"))
}

pub fn add_stdio_server(
    config: &AppConfig,
    name: &str,
    command: &str,
    args: Vec<String>,
) -> Result<McpServerConfig> {
    let normalized = normalize_server_name(name);
    if normalized.is_empty() {
        bail!("server name cannot be empty");
    }
    if command.trim().is_empty() {
        bail!("stdio command cannot be empty");
    }
    mutate_servers(config, |servers| {
        upsert_server(
            servers,
            McpServerConfig {
                name: normalized.clone(),
                enabled: true,
                transport: McpTransport::Stdio,
                command: command.trim().to_string(),
                args: args.clone(),
                url: String::new(),
            },
        )
    })
}

pub fn add_http_server(config: &AppConfig, name: &str, url: &str) -> Result<McpServerConfig> {
    let normalized = normalize_server_name(name);
    if normalized.is_empty() {
        bail!("server name cannot be empty");
    }
    if url.trim().is_empty() {
        bail!("server URL cannot be empty");
    }
    mutate_servers(config, |servers| {
        upsert_server(
            servers,
            McpServerConfig {
                name: normalized.clone(),
                enabled: true,
                transport: McpTransport::Http,
                command: String::new(),
                args: Vec::new(),
                url: url.trim().to_string(),
            },
        )
    })
}

pub fn set_enabled(config: &AppConfig, name: &str, enabled: bool) -> Result<McpServerConfig> {
    let normalized = normalize_server_name(name);
    mutate_servers(config, |servers| {
        let server = servers
            .iter_mut()
            .find(|server| server.name == normalized)
            .with_context(|| format!("unknown MCP server '{normalized}'"))?;
        server.enabled = enabled;
        Ok(server.clone())
    })
}

pub fn remove_server(config: &AppConfig, name: &str) -> Result<McpServerConfig> {
    let normalized = normalize_server_name(name);
    mutate_servers(config, |servers| {
        let index = servers
            .iter()
            .position(|server| server.name == normalized)
            .with_context(|| format!("unknown MCP server '{normalized}'"))?;
        Ok(servers.remove(index))
    })
}

pub fn mcp_servers_path(config: &AppConfig) -> PathBuf {
    config
        .config_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("mcp-servers.json")
}

fn mutate_servers<T>(
    config: &AppConfig,
    mut mutate: impl FnMut(&mut Vec<McpServerConfig>) -> Result<T>,
) -> Result<T> {
    let path = mcp_servers_path(config);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let mut servers = load_servers(config)?;
    let value = mutate(&mut servers)?;
    servers.sort_by(|left, right| left.name.cmp(&right.name));
    fs::write(&path, serde_json::to_string_pretty(&servers)?)
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(value)
}

fn upsert_server(
    servers: &mut Vec<McpServerConfig>,
    next: McpServerConfig,
) -> Result<McpServerConfig> {
    if let Some(existing) = servers.iter_mut().find(|server| server.name == next.name) {
        *existing = next.clone();
        Ok(existing.clone())
    } else {
        servers.push(next.clone());
        Ok(next)
    }
}

fn normalize_server_name(value: &str) -> String {
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
