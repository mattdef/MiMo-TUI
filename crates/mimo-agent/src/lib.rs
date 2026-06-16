use std::{future::Future, pin::Pin};

use anyhow::{Context, Result, anyhow, bail};
use mimo_client::{AssistantResponse, MimoClient};
use mimo_protocol::{ApiTool, ChatMessage};
use mimo_tools::{ApprovalRequirement, ToolContext, ToolInvocation, ToolRegistry};
use tokio::task;

const MAX_TOOL_ROUNDS: usize = 25;

/// Abstraction pour un client de chat completion.
///
/// # Pourquoi cette complexité ?
///
/// Ce trait utilise des lifetimes explicites et `Pin<Box<dyn Future>>` au lieu de `async fn`
/// pour plusieurs raisons :
///
/// 1. **Compatibilité avec `tokio::spawn`** : L'agent loop utilise `spawn_blocking` pour
///    exécuter les tools, ce qui requiert que les futures soient `Send + 'static`. Les
///    lifetimes explicites permettent de garantir cette propriété.
///
/// 2. **Pas de dépendance sur `async_trait`** : Bien que `async_trait` simplifierait la
///    syntaxe, cela ajouterait une dépendance externe et générerait du code boxing automatique.
///    L'approche manuelle donne un contrôle total sur le boxing et les lifetimes.
///
/// 3. **Flexibilité pour les mocks** : Les lifetimes liées à `&'a self` permettent aux
///    implémentations de test (comme `TestClient`) de capturer des références sans cloning.
///
/// # Implémentation
///
/// Pour implémenter ce trait, wrappez votre `async fn` dans `Box::pin(async move { ... })`.
/// Voir l'implémentation pour `MimoClient` comme exemple.
pub trait ChatCompletionClient {
    fn stream_chat_completion<'a>(
        &'a self,
        messages: &'a [ChatMessage],
        tools: &'a [ApiTool],
        on_delta: &'a mut (dyn FnMut(&str) -> Result<()> + Send + 'a),
    ) -> Pin<Box<dyn Future<Output = Result<AssistantResponse>> + Send + 'a>>;
}

impl ChatCompletionClient for MimoClient {
    fn stream_chat_completion<'a>(
        &'a self,
        messages: &'a [ChatMessage],
        tools: &'a [ApiTool],
        on_delta: &'a mut (dyn FnMut(&str) -> Result<()> + Send + 'a),
    ) -> Pin<Box<dyn Future<Output = Result<AssistantResponse>> + Send + 'a>> {
        Box::pin(async move {
            MimoClient::stream_chat_completion(self, messages, tools, |delta| on_delta(delta)).await
        })
    }
}

pub async fn run_agent_turn<C, FDelta, FStatus, FApprove, Fut>(
    client: &C,
    mut messages: Vec<ChatMessage>,
    registry: &ToolRegistry,
    context: &ToolContext,
    mut on_delta: FDelta,
    mut on_status: FStatus,
    mut approve_tool: FApprove,
) -> Result<()>
where
    C: ChatCompletionClient + Sync + ?Sized,
    FDelta: FnMut(&str) -> Result<()> + Send,
    FStatus: FnMut(AgentStatus) -> Result<()> + Send,
    FApprove: FnMut(ToolInvocation) -> Fut + Send,
    Fut: std::future::Future<Output = Result<bool>> + Send,
{
    for _round in 0..MAX_TOOL_ROUNDS {
        if context.is_cancelled() {
            bail!("agent turn cancelled");
        }

        let assistant = {
            let mut delta_handler = |delta: &str| on_delta(delta);
            client
                .stream_chat_completion(&messages, registry.api_tools(), &mut delta_handler)
                .await?
        };

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
    use std::{
        collections::VecDeque,
        future::Future,
        pin::Pin,
        sync::{Arc, Mutex},
    };

    use anyhow::{Context, Result, anyhow, bail};
    use mimo_client::AssistantResponse;
    use mimo_protocol::{ApiTool, ChatMessage, Role, ToolCall, ToolFunction};
    use mimo_tools::{
        ApprovalRequirement, ToolContext, ToolInvocation, ToolKind, ToolRegistry,
        ToolRegistryBuilder, ToolResult, ToolSpec,
    };
    use serde_json::{Value, json};

    use super::{AgentStatus, ChatCompletionClient, MAX_TOOL_ROUNDS, run_agent_turn};

    #[derive(Debug, Clone)]
    struct TestRound {
        response: AssistantResponse,
        deltas: Vec<String>,
    }

    #[derive(Clone)]
    struct TestClient {
        rounds: Arc<Mutex<VecDeque<TestRound>>>,
        seen_messages: Arc<Mutex<Vec<Vec<ChatMessage>>>>,
    }

    impl TestClient {
        fn new(rounds: Vec<TestRound>) -> Self {
            Self {
                rounds: Arc::new(Mutex::new(VecDeque::from(rounds))),
                seen_messages: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn seen_messages(&self) -> Vec<Vec<ChatMessage>> {
            self.seen_messages
                .lock()
                .map(|messages| messages.clone())
                .unwrap_or_default()
        }
    }

    impl ChatCompletionClient for TestClient {
        fn stream_chat_completion<'a>(
            &'a self,
            messages: &'a [ChatMessage],
            _tools: &'a [ApiTool],
            on_delta: &'a mut (dyn FnMut(&str) -> Result<()> + Send + 'a),
        ) -> Pin<Box<dyn Future<Output = Result<AssistantResponse>> + Send + 'a>> {
            let rounds = Arc::clone(&self.rounds);
            let seen_messages = Arc::clone(&self.seen_messages);
            let snapshot = messages.to_vec();

            Box::pin(async move {
                seen_messages
                    .lock()
                    .map_err(|poison| anyhow!("test mutex poisoned: {poison}"))?
                    .push(snapshot);
                let round = rounds
                    .lock()
                    .map_err(|poison| anyhow!("test mutex poisoned: {poison}"))?
                    .pop_front()
                    .context("expected queued round")?;
                for delta in &round.deltas {
                    on_delta(delta)?;
                }
                Ok(round.response)
            })
        }
    }

    struct SuccessfulTool;

    impl ToolSpec for SuccessfulTool {
        fn name(&self) -> &'static str {
            "successful_tool"
        }

        fn description(&self) -> &'static str {
            "Returns a canned success response"
        }

        fn input_schema(&self) -> Value {
            json!({ "type": "object" })
        }

        fn kind(&self) -> ToolKind {
            ToolKind::Shell
        }

        fn summarize(&self, _input: &Value) -> String {
            "successful_tool".to_string()
        }

        fn execute(&self, _input: Value, _context: &ToolContext) -> Result<ToolResult> {
            Ok(ToolResult::new("tool output", "tool success"))
        }
    }

    struct FailingTool;

    impl ToolSpec for FailingTool {
        fn name(&self) -> &'static str {
            "failing_tool"
        }

        fn description(&self) -> &'static str {
            "Always fails"
        }

        fn input_schema(&self) -> Value {
            json!({ "type": "object" })
        }

        fn kind(&self) -> ToolKind {
            ToolKind::Shell
        }

        fn summarize(&self, _input: &Value) -> String {
            "failing_tool".to_string()
        }

        fn execute(&self, _input: Value, _context: &ToolContext) -> Result<ToolResult> {
            bail!("boom")
        }
    }

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

    fn make_tool_call(name: &str) -> ToolCall {
        ToolCall {
            id: "call_1".to_string(),
            kind: "function".to_string(),
            function: ToolFunction {
                name: name.to_string(),
                arguments: "{}".to_string(),
            },
        }
    }

    fn registry_with(tool: Arc<dyn ToolSpec>) -> ToolRegistry {
        ToolRegistryBuilder::new().with_tool(tool).build()
    }

    fn status_name(status: &AgentStatus) -> String {
        match status {
            AgentStatus::ToolRequested(invocation) => format!("requested:{}", invocation.name),
            AgentStatus::ToolStarted(invocation) => format!("started:{}", invocation.name),
            AgentStatus::ToolSucceeded(invocation, _) => format!("succeeded:{}", invocation.name),
            AgentStatus::ToolFailed(invocation, _) => format!("failed:{}", invocation.name),
            AgentStatus::ToolFinished(invocation, approved) => {
                format!("finished:{approved}:{}", invocation.name)
            }
        }
    }

    fn test_context() -> ToolContext {
        ToolContext::new(std::env::current_dir().expect("current dir"))
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

    #[tokio::test]
    async fn completes_without_tool_calls() {
        let client = TestClient::new(vec![TestRound {
            response: AssistantResponse {
                content: "done".to_string(),
                tool_calls: Vec::new(),
            },
            deltas: vec!["done".to_string()],
        }]);
        let registry = ToolRegistryBuilder::new().build();
        let context = test_context();
        let mut deltas = String::new();
        let mut statuses = Vec::new();

        run_agent_turn(
            &client,
            vec![ChatMessage::user("hello")],
            &registry,
            &context,
            |delta| {
                deltas.push_str(delta);
                Ok(())
            },
            |status| {
                statuses.push(status_name(&status));
                Ok(())
            },
            |_| std::future::ready(Ok(true)),
        )
        .await
        .expect("agent should complete without tools");

        assert_eq!(deltas, "done");
        assert!(statuses.is_empty());
        assert_eq!(client.seen_messages().len(), 1);
    }

    #[tokio::test]
    async fn runs_approved_tool_and_sends_tool_output_to_next_round() {
        let client = TestClient::new(vec![
            TestRound {
                response: AssistantResponse {
                    content: String::new(),
                    tool_calls: vec![make_tool_call("successful_tool")],
                },
                deltas: Vec::new(),
            },
            TestRound {
                response: AssistantResponse {
                    content: "done".to_string(),
                    tool_calls: Vec::new(),
                },
                deltas: vec!["done".to_string()],
            },
        ]);
        let registry = registry_with(Arc::new(SuccessfulTool));
        let context = test_context();
        let mut statuses = Vec::new();

        run_agent_turn(
            &client,
            vec![ChatMessage::user("run tool")],
            &registry,
            &context,
            |_| Ok(()),
            |status| {
                statuses.push(status_name(&status));
                Ok(())
            },
            |_| std::future::ready(Ok(true)),
        )
        .await
        .expect("approved tool flow should succeed");

        assert_eq!(
            statuses,
            vec![
                "requested:successful_tool".to_string(),
                "started:successful_tool".to_string(),
                "succeeded:successful_tool".to_string(),
            ]
        );

        let seen_messages = client.seen_messages();
        assert_eq!(seen_messages.len(), 2);
        assert!(
            seen_messages[1]
                .iter()
                .any(|message| message.role == Role::Tool && message.content == "tool output")
        );
    }

    #[tokio::test]
    async fn records_denied_tool_request_and_skips_execution() {
        let client = TestClient::new(vec![
            TestRound {
                response: AssistantResponse {
                    content: String::new(),
                    tool_calls: vec![make_tool_call("successful_tool")],
                },
                deltas: Vec::new(),
            },
            TestRound {
                response: AssistantResponse {
                    content: "done".to_string(),
                    tool_calls: Vec::new(),
                },
                deltas: vec!["done".to_string()],
            },
        ]);
        let registry = registry_with(Arc::new(SuccessfulTool));
        let context = test_context();
        let mut statuses = Vec::new();

        run_agent_turn(
            &client,
            vec![ChatMessage::user("deny tool")],
            &registry,
            &context,
            |_| Ok(()),
            |status| {
                statuses.push(status_name(&status));
                Ok(())
            },
            |_| std::future::ready(Ok(false)),
        )
        .await
        .expect("denied tool flow should still complete");

        assert_eq!(
            statuses,
            vec![
                "requested:successful_tool".to_string(),
                "finished:false:successful_tool".to_string(),
            ]
        );

        let seen_messages = client.seen_messages();
        assert_eq!(seen_messages.len(), 2);
        assert!(seen_messages[1].iter().any(|message| {
            message.role == Role::Tool
                && message
                    .content
                    .contains("Tool request denied by the user: successful_tool")
        }));
    }

    #[tokio::test]
    async fn records_tool_errors_and_continues() {
        let client = TestClient::new(vec![
            TestRound {
                response: AssistantResponse {
                    content: String::new(),
                    tool_calls: vec![make_tool_call("failing_tool")],
                },
                deltas: Vec::new(),
            },
            TestRound {
                response: AssistantResponse {
                    content: "done".to_string(),
                    tool_calls: Vec::new(),
                },
                deltas: vec!["done".to_string()],
            },
        ]);
        let registry = registry_with(Arc::new(FailingTool));
        let context = test_context();
        let mut statuses = Vec::new();

        run_agent_turn(
            &client,
            vec![ChatMessage::user("fail tool")],
            &registry,
            &context,
            |_| Ok(()),
            |status| {
                statuses.push(status_name(&status));
                Ok(())
            },
            |_| std::future::ready(Ok(true)),
        )
        .await
        .expect("tool error flow should still complete");

        assert_eq!(
            statuses,
            vec![
                "requested:failing_tool".to_string(),
                "started:failing_tool".to_string(),
                "failed:failing_tool".to_string(),
            ]
        );

        let seen_messages = client.seen_messages();
        assert_eq!(seen_messages.len(), 2);
        assert!(seen_messages[1].iter().any(|message| {
            message.role == Role::Tool
                && message
                    .content
                    .contains("Tool error: tool 'failing_tool' failed")
        }));
    }

    #[tokio::test]
    async fn errors_after_maximum_tool_rounds() {
        let rounds = (0..MAX_TOOL_ROUNDS)
            .map(|_| TestRound {
                response: AssistantResponse {
                    content: String::new(),
                    tool_calls: vec![make_tool_call("successful_tool")],
                },
                deltas: Vec::new(),
            })
            .collect::<Vec<_>>();
        let client = TestClient::new(rounds);
        let registry = registry_with(Arc::new(SuccessfulTool));
        let context = test_context();

        let error = run_agent_turn(
            &client,
            vec![ChatMessage::user("loop")],
            &registry,
            &context,
            |_| Ok(()),
            |_| Ok(()),
            |_| std::future::ready(Ok(true)),
        )
        .await
        .expect_err("agent should stop after maximum tool rounds");

        assert!(error.to_string().contains("agent exceeded"));
    }
}
