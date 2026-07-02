use serde::{Deserialize, Serialize};

pub use mimo_config::AppMode;

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
