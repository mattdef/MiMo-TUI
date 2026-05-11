use std::{collections::BTreeMap, sync::Arc};

use anyhow::{Context, Result};
use serde::Serialize;
use serde_json::Value;

use crate::client::ToolCall;

use super::{
    EditFileTool, ExecShellTool, ListDirTool, ReadFileTool, ShellCancelTool, ShellInteractTool,
    ShellWaitTool, ToolContext, ToolKind, ToolResult, ToolSpec, WriteFileTool,
};

#[derive(Debug, Clone, Serialize)]
pub struct ApiTool {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub function: ApiToolFunction,
}

#[derive(Debug, Clone, Serialize)]
pub struct ApiToolFunction {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

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
