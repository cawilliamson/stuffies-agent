//! the turn loop (spec 9, 10).
//!
//! ponytail: cross-turn history is rebuilt from `turn.started` prompts and `llm.completed`
//! final text only. tool calls are live inside a turn but not replayed across turn
//! boundaries. upgrade to a full message log if multi-step tool reasoning must survive a
//! turn boundary.

use std::fmt::Write as _;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use sa_proto::session::kinds;
use sa_proto::session::{Event, Usage};
use serde_json::{json, Value};

use crate::llm::{message, ChatRequest, Delta, Provider, ToolCall, ToolSpec};
use crate::store::StoreError;

/// how often an in-flight turn checks for cancellation. short enough that escape feels
/// instant, long enough not to spin.
const CANCEL_POLL: std::time::Duration = std::time::Duration::from_millis(25);

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    #[error("llm: {0}")]
    Llm(#[from] crate::llm::LlmError),
    #[error("store: {0}")]
    Store(#[from] StoreError),
    #[error("cancelled")]
    Cancelled,
}

/// where the agent writes its events.
pub trait EventSink: Send + Sync {
    fn append(&self, kind: &str, payload: &Value) -> Result<u64, StoreError>;
    fn is_cancelled(&self) -> bool;

    /// messages the operator sent while this turn was running. the loop folds them in
    /// between model round-trips, so the agent can be redirected without losing its work.
    fn drain_steering(&self) -> Vec<String> {
        Vec::new()
    }
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
    /// the session's workspace. the daemon serves many workspaces, so its own cwd is
    /// meaningless to a turn.
    pub workspace: String,
    pub history: Vec<Event>,
    pub compaction_summary: Option<String>,
    pub context: Option<String>,
    pub max_tokens: Option<u64>,
    pub reasoning_effort: Option<String>,
    /// images attached to this prompt.
    pub images: Vec<sa_proto::session::ImageAttachment>,
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
    /// runs `work`, giving up the moment the operator cancels.
    ///
    /// cancellation is a flag, so a plain await on a stalled provider would not observe it
    /// until the endpoint produced something. polling turns escape into a 25ms guarantee.
    async fn until_cancelled<F>(&self, work: F) -> Option<F::Output>
    where
        F: Future,
    {
        let mut work = std::pin::pin!(work);
        loop {
            tokio::select! {
                output = &mut work => return Some(output),
                () = tokio::time::sleep(CANCEL_POLL) => {
                    if self.sink.is_cancelled() {
                        return None;
                    }
                }
            }
        }
    }

    /// the next delta, or `Cancelled` if the operator interrupts while waiting.
    async fn next_delta(
        &self,
        rx: &mut tokio::sync::mpsc::Receiver<Delta>,
    ) -> Result<Option<Delta>, AgentError> {
        self.until_cancelled(rx.recv())
            .await
            .map_or_else(|| Err(AgentError::Cancelled), Ok)
    }

    pub async fn run_turn(&self, request: TurnRequest) -> Result<TurnOutcome, AgentError> {
        let mut messages = self.build_transcript(&request)?;
        let mut usage = Usage {
            prompt_tokens: 0,
            completion_tokens: 0,
            cost_micros: None,
        };

        // a turn runs until the model answers or the operator interrupts it: no iteration
        // cap, so a long job is never cut short.
        loop {
            if self.sink.is_cancelled() {
                return Err(AgentError::Cancelled);
            }

            // anything the operator typed while the last round-trip ran becomes the next
            // instruction, rather than waiting for this turn to finish.
            for message in self.sink.drain_steering() {
                messages.push(message::user(message));
            }

            let Some(streamed) = self
                .until_cancelled(self.provider.stream(ChatRequest {
                    model: request.model.clone(),
                    messages: messages.clone(),
                    tools: self.tools.specs(),
                    max_tokens: request.max_tokens,
                    reasoning_effort: request.reasoning_effort.clone(),
                }))
                .await
            else {
                return Err(AgentError::Cancelled);
            };
            let mut rx = streamed?;

            let mut text = String::new();
            let mut tool_calls = Vec::new();
            let mut done = false;

            while let Some(delta) = self.next_delta(&mut rx).await? {
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
                    Delta::Failed(reason) => {
                        return Err(AgentError::Llm(crate::llm::LlmError::Stream(reason)));
                    }
                    Delta::Done {
                        stop_reason,
                        prompt_tokens,
                        completion_tokens,
                    } => {
                        usage.prompt_tokens += prompt_tokens;
                        usage.completion_tokens += completion_tokens;
                        // the meter follows the conversation as it grows, rather than
                        // sitting at its last figure until the whole turn ends.
                        if prompt_tokens > 0 {
                            self.sink.append(
                                kinds::TURN_CONTEXT,
                                &json!({
                                    "turn_id": request.turn_id,
                                    "tokens": prompt_tokens,
                                }),
                            )?;
                        }
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
            messages.push(message::assistant_with_calls(&text, &tool_calls)?);

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
                // a cancelled turn drops the tool future, and a process tool is killed on
                // drop, so escape stops a running command rather than waiting it out.
                let Some(executed) = self.until_cancelled(self.tools.execute(call.clone())).await
                else {
                    return Err(AgentError::Cancelled);
                };
                match executed {
                    Ok(outcome) => {
                        let (output, truncated) = cap_tool_output(outcome.output);
                        self.sink.append(
                            kinds::TOOL_COMPLETED,
                            &json!({
                                "turn_id": request.turn_id,
                                "call_id": call.id,
                                "output": output,
                                "truncated": truncated,
                                "exit_code": outcome.exit_code,
                                "duration_ms": started.elapsed().as_millis() as u64,
                            }),
                        )?;
                        messages.push(message::tool_result(call, output)?);
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
                        messages.push(message::tool_result(call, format!("error: {error}"))?);
                    }
                }
            }
        }
    }

    fn build_transcript(
        &self,
        request: &TurnRequest,
    ) -> Result<Vec<rig_core::completion::Message>, AgentError> {
        let mut system = system_prompt(&self.tools.specs(), &request.workspace);
        if let Some(context) = &request.context {
            system.push_str("\n\nProject context (read-only, deployed outside the workspace):\n");
            system.push_str(context);
        }
        let mut messages = vec![message::system(system)];

        if let Some(summary) = &request.compaction_summary {
            messages.push(message::system(format!(
                "Summary of earlier work:\n{summary}"
            )));
        }

        // the whole conversation, tool calls and results included. dropping them made every
        // turn start from prose alone, so the model re-read the workspace and drifted from
        // what it had already done.
        let mut asked: Vec<ToolCall> = Vec::new();
        let mut asked_text = String::new();
        let mut announced = false;
        for event in &request.history {
            match event.kind.as_str() {
                kinds::TURN_STARTED => {
                    if let Some(prompt) = event.payload.get("prompt").and_then(Value::as_str) {
                        if prompt != request.prompt {
                            messages.push(message::user(prompt));
                        }
                    }
                }
                kinds::TURN_STEERED => {
                    if let Some(prompt) = event.payload.get("prompt").and_then(Value::as_str) {
                        messages.push(message::user(prompt));
                    }
                }
                kinds::LLM_COMPLETED => {
                    let text = event.payload.get("text").and_then(Value::as_str);
                    let calls_tools = event.payload.get("stop_reason").and_then(Value::as_str)
                        == Some("tool_calls");
                    if calls_tools {
                        // the turn that asked for the calls is emitted with them, once the
                        // first result arrives, so a call is never left unpaired.
                        asked_text = text.unwrap_or_default().to_owned();
                    } else if let Some(text) = text {
                        if !text.is_empty() {
                            messages.push(message::assistant(text));
                        }
                    }
                }
                kinds::TOOL_REQUESTED => {
                    let id = event.payload["call_id"].as_str().unwrap_or_default();
                    let name = event.payload["tool"].as_str().unwrap_or_default();
                    if id.is_empty() || name.is_empty() {
                        continue;
                    }
                    asked.push(ToolCall {
                        id: id.to_owned(),
                        name: name.to_owned(),
                        arguments: event.payload["args"].to_string(),
                    });
                }
                kinds::TOOL_COMPLETED | kinds::TOOL_FAILED => {
                    if !announced && !asked.is_empty() {
                        messages.push(message::assistant_with_calls(&asked_text, &asked)?);
                        announced = true;
                    }
                    let id = event.payload["call_id"].as_str().unwrap_or_default();
                    let Some(call) = asked.iter().find(|call| call.id == id) else {
                        continue;
                    };
                    let content = event.payload["output"].as_str().map_or_else(
                        || {
                            format!(
                                "error: {}",
                                event.payload["error"].as_str().unwrap_or("failed")
                            )
                        },
                        str::to_owned,
                    );
                    messages.push(message::tool_result(call, content)?);
                }
                _ => {}
            }
        }

        messages.push(if request.images.is_empty() {
            message::user(request.prompt.clone())
        } else {
            message::user_with_images(request.prompt.clone(), &request.images)
        });

        // rig owns the pairing rules: an interrupted turn leaves a call nothing answered,
        // and repairing that here is what keeps the endpoint from rejecting the history.
        let paired = rig_core::conversation::pairing::repair(messages);
        if !paired.repairs.is_empty() {
            tracing::debug!(repairs = ?paired.repairs, "repaired the replayed history");
        }
        Ok(paired.messages)
    }
}

/// who the agent is, what it can do, and how to behave. the tool list is generated from
/// the registry so the prompt cannot drift from the tools that are actually callable.
fn system_prompt(tools: &[ToolSpec], workspace: &str) -> String {
    let mut prompt = format!(
        "You are sa, a coding agent working in the git workspace at {workspace}. \
         Tool paths resolve inside it; a path outside it is an error.\nTools:\n"
    );
    for tool in tools {
        let _ = writeln!(prompt, "- {}: {}", tool.name, tool.description);
    }

    prompt.push_str(
        "Rules:\n\
         - read a file before editing it\n\
         - search with grep and find rather than the shell\n\
         - run the project's own checks after changing code\n\
         - keep replies short\n",
    );
    prompt
}

fn parse_args(raw: &str) -> Value {
    serde_json::from_str(raw).unwrap_or_else(|_| Value::String(raw.to_owned()))
}

/// bounds a tool result so one flooding command cannot eat the context (spec 9). the cut
/// lands on a char boundary and is marked, and the event records it.
fn cap_tool_output(output: String) -> (String, bool) {
    if output.len() <= crate::tools::MAX_TOOL_OUTPUT {
        return (output, false);
    }
    let mut end = crate::tools::MAX_TOOL_OUTPUT;
    while !output.is_char_boundary(end) {
        end -= 1;
    }
    let mut capped = output[..end].to_owned();
    capped.push_str("\n[truncated]");
    (capped, true)
}
