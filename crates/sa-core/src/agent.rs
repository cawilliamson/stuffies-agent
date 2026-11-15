//! the turn loop (spec 9, 10).
//!
//! ponytail: cross-turn history is rebuilt from `turn.started` prompts and `llm.completed`
//! final text only. tool calls are live inside a turn but not replayed across turn
//! boundaries. upgrade to a full message log if multi-step tool reasoning must survive a
//! turn boundary.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use sa_proto::session::kinds;
use sa_proto::session::{Event, Usage};
use serde_json::{json, Value};

use crate::llm::{ChatMessage, ChatRequest, Delta, Provider, ToolCall, ToolSpec};
use crate::store::StoreError;

/// the maximum number of model round-trips in one turn. bounds a runaway loop.
pub const MAX_ITERATIONS: usize = 45;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    #[error("llm: {0}")]
    Llm(#[from] crate::llm::LlmError),
    #[error("store: {0}")]
    Store(#[from] StoreError),
    #[error("cancelled")]
    Cancelled,
    #[error("iteration limit {MAX_ITERATIONS} reached without a final answer")]
    IterationLimit,
}

/// where the agent writes its events.
pub trait EventSink: Send + Sync {
    fn append(&self, kind: &str, payload: &Value) -> Result<u64, StoreError>;
    fn is_cancelled(&self) -> bool;
}

/// executes tool calls. the registry arrives at v1.0 task 4; tests use an empty one.
///
/// what a tool produced. `exit_code` is set by tools that run a process, so the client can
/// show it rather than burying it in the output text.
#[derive(Debug, Clone)]
pub struct ToolOutcome {
    pub output: String,
    pub exit_code: Option<i32>,
}

pub trait ToolExecutor: Send + Sync {
    fn specs(&self) -> Vec<ToolSpec>;
    fn execute(&self, call: ToolCall) -> BoxFuture<'_, Result<ToolOutcome, String>>;
}

pub struct EmptyTools;

impl ToolExecutor for EmptyTools {
    fn specs(&self) -> Vec<ToolSpec> {
        Vec::new()
    }

    fn execute(&self, call: ToolCall) -> BoxFuture<'_, Result<ToolOutcome, String>> {
        Box::pin(async move { Err(format!("unknown tool: {}", call.name)) })
    }
}

#[derive(Debug, Clone)]
pub struct TurnRequest {
    pub turn_id: String,
    pub prompt: String,
    pub model: String,
    pub history: Vec<Event>,
    pub compaction_summary: Option<String>,
    pub context: Option<String>,
    pub max_tokens: Option<u64>,
    pub reasoning_effort: Option<String>,
}

#[derive(Debug, Clone)]
pub struct TurnOutcome {
    pub finish_reason: String,
    pub usage: Usage,
    pub text: String,
}

pub struct Agent {
    provider: Arc<dyn Provider>,
    tools: Arc<dyn ToolExecutor>,
    sink: Arc<dyn EventSink>,
}

impl Agent {
    #[must_use]
    pub fn new(
        provider: Arc<dyn Provider>,
        tools: Arc<dyn ToolExecutor>,
        sink: Arc<dyn EventSink>,
    ) -> Self {
        Self {
            provider,
            tools,
            sink,
        }
    }

    /// runs one turn to completion, appending every step to the log.
    pub async fn run_turn(&self, request: TurnRequest) -> Result<TurnOutcome, AgentError> {
        let mut messages = Self::build_transcript(&request);
        let mut usage = Usage {
            prompt_tokens: 0,
            completion_tokens: 0,
            cost_micros: None,
        };

        for _ in 0..MAX_ITERATIONS {
            if self.sink.is_cancelled() {
                return Err(AgentError::Cancelled);
            }

            let mut rx = self
                .provider
                .stream(ChatRequest {
                    model: request.model.clone(),
                    messages: messages.clone(),
                    tools: self.tools.specs(),
                    max_tokens: request.max_tokens,
                    reasoning_effort: request.reasoning_effort.clone(),
                })
                .await?;

            let mut text = String::new();
            let mut tool_calls = Vec::new();
            let mut done = false;

            while let Some(delta) = rx.recv().await {
                if self.sink.is_cancelled() {
                    return Err(AgentError::Cancelled);
                }
                match delta {
                    Delta::Text(chunk) => {
                        text.push_str(&chunk);
                        self.sink.append(
                            kinds::LLM_DELTA,
                            &json!({"turn_id": request.turn_id, "text": chunk}),
                        )?;
                    }
                    Delta::Reasoning(chunk) => {
                        self.sink.append(
                            kinds::LLM_REASONING,
                            &json!({"turn_id": request.turn_id, "text": chunk}),
                        )?;
                    }
                    Delta::ToolCall(call) => tool_calls.push(call),
                    Delta::Done {
                        stop_reason,
                        prompt_tokens,
                        completion_tokens,
                    } => {
                        usage.prompt_tokens += prompt_tokens;
                        usage.completion_tokens += completion_tokens;
                        done = true;
                        if stop_reason != "tool_calls" && tool_calls.is_empty() {
                            self.sink.append(
                                kinds::LLM_COMPLETED,
                                &json!({
                                    "turn_id": request.turn_id,
                                    "stop_reason": stop_reason,
                                    "text": text,
                                    "purpose": "turn",
                                    "usage": {
                                        "prompt_tokens": prompt_tokens,
                                        "completion_tokens": completion_tokens,
                                    },
                                }),
                            )?;
                            return Ok(TurnOutcome {
                                finish_reason: stop_reason,
                                usage,
                                text,
                            });
                        }
                    }
                }
            }

            if !done {
                return Err(AgentError::Llm(crate::llm::LlmError::Truncated));
            }

            self.sink.append(
                kinds::LLM_COMPLETED,
                &json!({
                    "turn_id": request.turn_id,
                    "stop_reason": "tool_calls",
                    "text": text,
                    "purpose": "turn",
                    "usage": {
                        "prompt_tokens": usage.prompt_tokens,
                        "completion_tokens": usage.completion_tokens,
                    },
                }),
            )?;

            // the assistant turn must carry the calls verbatim, before any result, so the
            // model can associate each result with the call that requested it.
            messages.push(ChatMessage::assistant_with_calls(&text, &tool_calls));

            for call in &tool_calls {
                self.sink.append(
                    kinds::TOOL_REQUESTED,
                    &json!({
                        "turn_id": request.turn_id,
                        "call_id": call.id,
                        "tool": call.name,
                        "args": parse_args(&call.arguments),
                    }),
                )?;

                let started = std::time::Instant::now();
                match self.tools.execute(call.clone()).await {
                    Ok(outcome) => {
                        // no truncation: the operator asked for full command output.
                        let stored = outcome.output.clone();
                        self.sink.append(
                            kinds::TOOL_COMPLETED,
                            &json!({
                                "turn_id": request.turn_id,
                                "call_id": call.id,
                                "output": stored,
                                "truncated": false,
                                "exit_code": outcome.exit_code,
                                "duration_ms": started.elapsed().as_millis() as u64,
                            }),
                        )?;
                        messages.push(ChatMessage::tool(&call.id, outcome.output));
                    }
                    Err(error) => {
                        self.sink.append(
                            kinds::TOOL_FAILED,
                            &json!({
                                "turn_id": request.turn_id,
                                "call_id": call.id,
                                "error": error,
                                "class": "tool_error",
                            }),
                        )?;
                        messages.push(ChatMessage::tool(&call.id, format!("error: {error}")));
                    }
                }
            }
        }

        Err(AgentError::IterationLimit)
    }

    fn build_transcript(request: &TurnRequest) -> Vec<ChatMessage> {
        let mut system = system_prompt();
        if let Some(context) = &request.context {
            system.push_str("\n\nProject context (read-only, deployed outside the workspace):\n");
            system.push_str(context);
        }
        let mut messages = vec![ChatMessage::system(system)];

        if let Some(summary) = &request.compaction_summary {
            messages.push(ChatMessage::system(format!(
                "Summary of earlier work:\n{summary}"
            )));
        }

        for event in &request.history {
            match event.kind.as_str() {
                kinds::TURN_STARTED => {
                    if let Some(prompt) = event.payload.get("prompt").and_then(Value::as_str) {
                        if prompt != request.prompt {
                            messages.push(ChatMessage::user(prompt));
                        }
                    }
                }
                kinds::LLM_COMPLETED => {
                    if let Some(text) = event.payload.get("text").and_then(Value::as_str) {
                        if !text.is_empty() {
                            messages.push(ChatMessage::assistant(text));
                        }
                    }
                }
                _ => {}
            }
        }

        messages.push(ChatMessage::user(request.prompt.clone()));
        messages
    }
}

fn system_prompt() -> String {
    "You are sa, a coding agent running in a daemon. You work inside a single git \
     workspace. Use the provided tools to read and modify files and to run commands. \
     Be concise. Prefer small, verifiable changes."
        .to_owned()
}

fn parse_args(raw: &str) -> Value {
    serde_json::from_str(raw).unwrap_or_else(|_| Value::String(raw.to_owned()))
}
