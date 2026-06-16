use anyhow::{Context, Result, anyhow, bail};
use mimo_client::MimoClient;
use mimo_protocol::ChatMessage;
use mimo_tools::{ApprovalRequirement, ToolContext, ToolInvocation, ToolRegistry};
use tokio::task;

const MAX_TOOL_ROUNDS: usize = 25;

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
    for _round in 0..MAX_TOOL_ROUNDS {
        if context.is_cancelled() {
            bail!("agent turn cancelled");
        }

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
            let exec_invocation = invocation.clone();
            let exec_registry = registry.clone();
            let exec_context = context.clone();
            let result = task::spawn_blocking(move || {
                exec_registry
                    .execute(&exec_invocation, &exec_context)
                    .with_context(|| format!("tool '{}' failed", exec_invocation.name))
            })
            .await
            .context("tool execution panicked")?;
            match result {
                Ok(output) => {
                    let summary = output.summary.clone();
                    messages.push(ChatMessage::tool(call.id, output.content));
                    on_status(AgentStatus::ToolSucceeded(invocation, summary))?;
                }
                Err(error) => {
                    let error_message = error.to_string();
                    on_status(AgentStatus::ToolFailed(
                        invocation.clone(),
                        error_message.clone(),
                    ))?;
                    messages.push(ChatMessage::tool(
                        call.id,
                        format!("Tool error: {error_message}"),
                    ));
                }
            }
        }
    }

    Err(anyhow!(
        "agent exceeded {MAX_TOOL_ROUNDS} tool-call rounds; possible infinite loop"
    ))
}

#[derive(Debug, Clone)]
pub enum AgentStatus {
    ToolRequested(ToolInvocation),
    ToolStarted(ToolInvocation),
    ToolSucceeded(ToolInvocation, String),
    ToolFailed(ToolInvocation, String),
    ToolFinished(ToolInvocation, bool),
}

#[cfg(test)]
mod tests {
    use super::AgentStatus;
    use mimo_tools::{ApprovalRequirement, ToolInvocation, ToolKind};

    fn make_invocation(name: &str) -> ToolInvocation {
        ToolInvocation {
            call_id: String::new(),
            name: name.to_string(),
            kind: ToolKind::Shell,
            summary: name.to_string(),
            approval_requirement: ApprovalRequirement::Auto,
            input: serde_json::Value::Null,
        }
    }

    #[test]
    fn agent_status_variants_are_clonable() {
        let inv = make_invocation("test");
        let requested = AgentStatus::ToolRequested(inv);
        let clone = requested.clone();
        assert!(matches!(clone, AgentStatus::ToolRequested(_)));
    }

    #[test]
    fn agent_status_variants_cover_lifecycle() {
        let inv = make_invocation("build");

        let started = AgentStatus::ToolStarted(inv.clone());
        assert!(matches!(started, AgentStatus::ToolStarted(_)));

        let succeeded = AgentStatus::ToolSucceeded(inv.clone(), "done".to_string());
        assert!(matches!(succeeded, AgentStatus::ToolSucceeded(_, _)));

        let failed = AgentStatus::ToolFailed(inv.clone(), "error".to_string());
        assert!(matches!(failed, AgentStatus::ToolFailed(_, _)));

        let finished = AgentStatus::ToolFinished(inv, true);
        assert!(matches!(finished, AgentStatus::ToolFinished(_, _)));
    }
}
