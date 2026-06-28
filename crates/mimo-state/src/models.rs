use std::fmt;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum AppMode {
    Plan,
    #[default]
    Agent,
}

impl fmt::Display for AppMode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Plan => formatter.write_str("plan"),
            Self::Agent => formatter.write_str("agent"),
        }
    }
}

impl<'de> Deserialize<'de> for AppMode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Ok(match value.to_ascii_lowercase().as_str() {
            "plan" => Self::Plan,
            "agent" | "chat" => Self::Agent,
            _ => Self::Agent,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlanItem {
    pub text: String,
    pub done: bool,
}

impl PlanItem {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            done: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileAttachment {
    pub path: String,
}

impl FileAttachment {
    pub fn new(path: impl Into<String>) -> Self {
        Self { path: path.into() }
    }
}

#[cfg(test)]
mod tests {
    use super::AppMode;

    #[test]
    fn unknown_mode_strings_fall_back_to_agent() {
        let mode = serde_json::from_str::<AppMode>(r#""legacy""#).expect("parse mode");
        assert_eq!(mode, AppMode::Agent);
    }
}
