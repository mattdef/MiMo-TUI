use std::{collections::BTreeMap, sync::Arc};

use anyhow::{Context, Result};
use mimo_protocol::{ApiTool, ApiToolFunction, ToolCall};
use serde_json::Value;

use super::{
    ApplyPatchTool, EditFileTool, ExecShellTool, FindPathsTool, GitDiffTool, GitLogTool,
    GitStatusTool, ListDirTool, ProjectSummaryTool, ReadFileTool, RunDiagnosticsTool, RunTestsTool,
    SearchTextTool, ShellCancelTool, ShellInteractTool, ShellWaitTool, ToolContext, ToolKind,
    ToolResult, ToolSpec, WebFetchTool, WriteFileTool,
};

#[derive(Debug, Clone)]
pub struct ToolInvocation {
    pub call_id: String,
    pub name: String,
    pub kind: ToolKind,
    pub summary: String,
    pub approval_requirement: super::ApprovalRequirement,
    pub input: Value,
}

#[derive(Clone)]
pub struct ToolRegistry {
    tools: Arc<BTreeMap<String, Arc<dyn ToolSpec>>>,
    api_tools: Arc<Vec<ApiTool>>,
}

impl ToolRegistry {
    pub fn api_tools(&self) -> &[ApiTool] {
        self.api_tools.as_slice()
    }

    pub fn invocation_for(&self, call: &ToolCall) -> Result<ToolInvocation> {
        let tool = self
            .tools
            .get(&call.function.name)
            .with_context(|| format!("tool '{}' is not registered", call.function.name))?;
        let input = serde_json::from_str::<Value>(&call.function.arguments)
            .with_context(|| format!("invalid tool arguments for {}", call.function.name))?;

        Ok(ToolInvocation {
            call_id: call.id.clone(),
            name: call.function.name.clone(),
            kind: tool.kind(),
            summary: tool.summarize(&input),
            approval_requirement: tool.approval_requirement(),
            input,
        })
    }

    pub fn execute(
        &self,
        invocation: &ToolInvocation,
        context: &ToolContext,
    ) -> Result<ToolResult> {
        let tool = self
            .tools
            .get(&invocation.name)
            .with_context(|| format!("tool '{}' is not registered", invocation.name))?;
        tool.execute(invocation.input.clone(), context)
    }
}

pub struct ToolRegistryBuilder {
    tools: BTreeMap<String, Arc<dyn ToolSpec>>,
}

impl ToolRegistryBuilder {
    pub fn new() -> Self {
        Self {
            tools: BTreeMap::new(),
        }
    }

    pub fn with_tool(mut self, tool: Arc<dyn ToolSpec>) -> Self {
        self.tools.insert(tool.name().to_string(), tool);
        self
    }

    pub fn with_file_tools(self) -> Self {
        self.with_tool(Arc::new(ReadFileTool))
            .with_tool(Arc::new(ListDirTool))
            .with_tool(Arc::new(WriteFileTool))
            .with_tool(Arc::new(EditFileTool))
    }

    pub fn with_shell_tools(self) -> Self {
        self.with_tool(Arc::new(ExecShellTool))
            .with_tool(Arc::new(ShellWaitTool))
            .with_tool(Arc::new(ShellInteractTool))
            .with_tool(Arc::new(ShellCancelTool))
    }

    pub fn with_search_tools(self) -> Self {
        self.with_tool(Arc::new(FindPathsTool))
            .with_tool(Arc::new(SearchTextTool))
    }

    pub fn with_git_tools(self) -> Self {
        self.with_tool(Arc::new(GitStatusTool))
            .with_tool(Arc::new(GitDiffTool))
            .with_tool(Arc::new(GitLogTool))
    }

    pub fn with_web_tools(self) -> Self {
        self.with_tool(Arc::new(WebFetchTool))
    }

    pub fn with_project_tools(self) -> Self {
        self.with_tool(Arc::new(ProjectSummaryTool))
    }

    pub fn with_patch_tools(self) -> Self {
        self.with_tool(Arc::new(ApplyPatchTool))
    }

    pub fn with_diagnostics_tool(self) -> Self {
        self.with_tool(Arc::new(RunDiagnosticsTool))
    }

    pub fn with_test_runner_tool(self) -> Self {
        self.with_tool(Arc::new(RunTestsTool))
    }

    pub fn build_all(self) -> ToolRegistry {
        self.with_file_tools()
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

    pub fn build(self) -> ToolRegistry {
        let api_tools = self
            .tools
            .values()
            .map(|tool| ApiTool {
                kind: "function",
                function: ApiToolFunction {
                    name: tool.name().to_string(),
                    description: tool.description().to_string(),
                    parameters: tool.input_schema(),
                },
            })
            .collect::<Vec<_>>();

        ToolRegistry {
            tools: Arc::new(self.tools),
            api_tools: Arc::new(api_tools),
        }
    }
}

impl Default for ToolRegistryBuilder {
    fn default() -> Self {
        Self::new()
    }
}
