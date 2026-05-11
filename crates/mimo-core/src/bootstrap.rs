use std::{env, path::PathBuf};

use mimo_tools::{ToolContext, ToolRegistry, ToolRegistryBuilder};

pub fn current_workspace_root() -> PathBuf {
    env::current_dir().unwrap_or_else(|_| ".".into())
}

pub fn create_tool_context(workspace_root: PathBuf) -> ToolContext {
    ToolContext::new(workspace_root)
}

pub fn create_default_tool_context() -> ToolContext {
    create_tool_context(current_workspace_root())
}

pub fn create_default_tool_registry() -> ToolRegistry {
    ToolRegistryBuilder::new()
        .with_file_tools()
        .with_search_tools()
        .with_git_tools()
        .with_web_tools()
        .with_project_tools()
        .with_patch_tools()
        .with_diagnostics_tool()
        .with_test_runner_tool()
        .with_shell_tools()
        .build()
}
