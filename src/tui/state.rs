use std::fmt;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum AppMode {
    Plan,
    #[default]
    #[serde(alias = "chat")]
    Agent,
    Yolo,
}

impl fmt::Display for AppMode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Plan => formatter.write_str("plan"),
            Self::Agent => formatter.write_str("agent"),
            Self::Yolo => formatter.write_str("yolo"),
        }
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
