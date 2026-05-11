use anyhow::{Context, Result};

use crate::{
    client::{ChatMessage, MimoClient},
    tools::{ApprovalRequirement, ToolContext, ToolInvocation, ToolRegistry},
};

pub async fn run_agent_turn<FDelta, FStatus, FApprove, Fut>(
    client: &MimoClient,
    mut messages: Vec<ChatMessage>,
    registry: &ToolRegistry,
    context: &ToolContext,
    mut on_delta: FDelta,
    mut on_status: FStatus,
    mut approve_tool: FApprove,
) -> Result<()>
where
    FDelta: FnMut(&str) -> Result<()>,
    FStatus: FnMut(AgentStatus) -> Result<()>,
    FApprove: FnMut(ToolInvocation) -> Fut,
    Fut: std::future::Future<Output = Result<bool>>,
{
    loop {
        let assistant = client
            .stream_chat_completion(&messages, registry.api_tools(), |delta| on_delta(delta))
            .await?;

        if assistant.tool_calls.is_empty() {
            if !assistant.content.is_empty() {
                messages.push(ChatMessage::assistant(assistant.content));
            }
            return Ok(());
        }

        let tool_calls = assistant.tool_calls.clone();
        messages.push(ChatMessage::assistant_with_tool_calls(
            assistant.content,
            assistant.tool_calls,
        ));

        for call in tool_calls {
            let invocation = registry.invocation_for(&call)?;
            on_status(AgentStatus::ToolRequested(invocation.clone()))?;

            let approved = match invocation.approval_requirement {
                ApprovalRequirement::Auto => true,
                ApprovalRequirement::Prompt => approve_tool(invocation.clone()).await?,
            };

            if !approved {
                on_status(AgentStatus::ToolFinished(invocation.clone(), false))?;
                messages.push(ChatMessage::tool(
                    call.id,
                    format!("Tool request denied by the user: {}", invocation.summary),
                ));
                continue;
            }

            on_status(AgentStatus::ToolStarted(invocation.clone()))?;
            let output = match registry
                .execute(&invocation, context)
                .with_context(|| format!("tool '{}' failed", invocation.name))
            {
                Ok(output) => output,
                Err(error) => {
                    on_status(AgentStatus::ToolFailed(
                        invocation.clone(),
                        error.to_string(),
                    ))?;
                    return Err(error);
                }
            };
            let summary = output.summary.clone();
            messages.push(ChatMessage::tool(call.id, output.content));
            on_status(AgentStatus::ToolSucceeded(invocation, summary))?;
        }
    }
}

#[derive(Debug, Clone)]
pub enum AgentStatus {
    ToolRequested(ToolInvocation),
    ToolStarted(ToolInvocation),
    ToolSucceeded(ToolInvocation, String),
    ToolFailed(ToolInvocation, String),
    ToolFinished(ToolInvocation, bool),
}
