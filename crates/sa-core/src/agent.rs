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

use rig_core::completion::prompt::{Section, SystemPrompt};
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

    /// the next message the operator sent while this turn was running, if any. the loop
    /// folds one in between model round-trips, so the agent can be redirected without
    /// losing its work.
    fn take_steering(&self) -> Option<String> {
        None
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

    /// the same call, reporting output as it arrives.
    ///
    /// a tool that says nothing until it is finished is the default: only a tool that runs
    /// a process has anything to report before the end.
    fn execute_streaming<'a>(
        &'a self,
        call: ToolCall,
        _sink: &'a dyn OutputSink,
    ) -> BoxFuture<'a, Result<ToolOutcome, String>> {
        self.execute(call)
    }
}

/// where a tool's output goes while the tool is still running, so the operator sees a long
/// command working rather than a blank frame until it ends.
pub trait OutputSink: Send + Sync {
    fn output(&self, call_id: &str, chunk: &str);
}

/// a sink that keeps nothing, for a call nobody is watching live.
pub struct NoOutput;

impl OutputSink for NoOutput {
    fn output(&self, _call_id: &str, _chunk: &str) {}
}

/// the sink a turn writes a tool's live output to: the session's event log, so a client
/// that attaches mid-command sees it too.
struct ToolStream<'a> {
    sink: &'a dyn EventSink,
    turn_id: &'a str,
}

impl OutputSink for ToolStream<'_> {
    fn output(&self, call_id: &str, chunk: &str) {
        // a dropped chunk costs a line of the live view, never the recorded output: the
        // tool's result is appended whole when it finishes.
        let _ = self.sink.append(
            kinds::TOOL_DELTA,
            &json!({"turn_id": self.turn_id, "call_id": call_id, "text": chunk}),
        );
    }
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
    /// continue the final assistant message instead of answering a new prompt.
    pub continue_final: bool,
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
        let (system, mut messages) = self.build_transcript(&request)?;
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
            // instruction, rather than waiting for this turn to finish. one message per
            // round-trip: a queue of redirects is not a single confused prompt.
            if let Some(message) = self.sink.take_steering() {
                messages.push(message::user(message));
            }

            let Some(streamed) = self
                .until_cancelled(self.provider.stream(ChatRequest {
                    model: request.model.clone(),
                    system: Some(system.clone()),
                    messages: messages.clone(),
                    tools: self.tools.specs(),
                    max_tokens: request.max_tokens,
                    reasoning_effort: request.reasoning_effort.clone(),
                    continue_final: request.continue_final,
                }))
                .await
            else {
                return Err(AgentError::Cancelled);
            };
            let mut rx = streamed?;

            let mut text = String::new();
            let mut tool_calls = Vec::new();
            let mut done = false;

            loop {
                let delta = match self.next_delta(&mut rx).await {
                    Ok(Some(delta)) => delta,
                    Ok(None) => break,
                    Err(err) => {
                        // a cancelled stream produced what it produced: record it, or the
                        // turn that carries on has nothing to carry on from.
                        if !text.is_empty() {
                            self.sink.append(
                                kinds::LLM_COMPLETED,
                                &json!({
                                    "turn_id": request.turn_id,
                                    "stop_reason": "cancelled",
                                    "text": text,
                                    "purpose": "turn",
                                    "usage": {
                                        "prompt_tokens": usage.prompt_tokens,
                                        "completion_tokens": usage.completion_tokens,
                                    },
                                }),
                            )?;
                        }
                        return Err(err);
                    }
                };
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
                        // a round that asked for calls stopped for them whatever the
                        // endpoint called it, and the transcript rebuild reads that back.
                        let stop_reason = if tool_calls.is_empty() {
                            stop_reason
                        } else {
                            "tool_calls".to_owned()
                        };
                        // no calls means nothing left to answer: the turn is over. a
                        // round that asked for calls but produced none would otherwise
                        // repeat forever.
                        if tool_calls.is_empty() {
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
                let stream = ToolStream {
                    sink: self.sink.as_ref(),
                    turn_id: &request.turn_id,
                };
                let Some(executed) = self
                    .until_cancelled(self.tools.execute_streaming(call.clone(), &stream))
                    .await
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
    ) -> Result<(SystemPrompt, Vec<rig_core::completion::Message>), AgentError> {
        let mut system = system_prompt(&self.tools.specs(), &request.workspace);
        if let Some(context) = &request.context {
            system
                .sections
                .push(Section::new("project_context", context.clone()));
        }
        if let Some(summary) = &request.compaction_summary {
            system
                .sections
                .push(Section::new("earlier_work", summary.clone()));
        }
        let mut messages = Vec::new();

        // the whole conversation, tool calls and results included. dropping them made every
        // turn start from prose alone, so the model re-read the workspace and drifted from
        // what it had already done.
        //
        // one round of calls at a time: each round announces its own calls before their
        // results, because a result whose call was never announced is an orphan, and rig
        // drops it on the way to the wire.
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
                        // a new round: the calls that follow are its own, and its turn is
                        // emitted with them once the first result arrives.
                        asked.clear();
                        asked_text = text.unwrap_or_default().to_owned();
                        announced = false;
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

        // a continued answer ends the conversation: a prompt would start a new one instead
        // of carrying on the message that was interrupted.
        if !request.continue_final {
            messages.push(if request.images.is_empty() {
                message::user(request.prompt.clone())
            } else {
                message::user_with_images(request.prompt.clone(), &request.images)
            });
        }

        // rig owns the pairing rules: an interrupted turn leaves a call nothing answered,
        // and repairing that here is what keeps the endpoint from rejecting the history.
        // rig shapes the history for the model that will read it: failed turns are
        // skipped, and every tool call is answered before the wire sees it.
        let adapted = rig_core::completion::history::adapt(
            &messages,
            &ChatTarget {
                model: request.model.clone(),
            },
        );
        Ok((system, adapted))
    }
}

/// who the agent is, what it can do, and how to behave. the tool list is generated from
/// the registry so the prompt cannot drift from the tools that are actually callable.
fn system_prompt(tools: &[ToolSpec], workspace: &str) -> SystemPrompt {
    let mut tools_text = String::new();
    for tool in tools {
        let _ = writeln!(tools_text, "- {}: {}", tool.name, tool.description);
    }

    SystemPrompt {
        role: format!(
            "You are stuffies-agent, a coding agent working in the git workspace at {workspace}. \
             Tool paths resolve inside it; a path outside it is an error."
        ),
        sections: vec![
            Section::new("tools", tools_text),
            Section::new(
                "rules",
                "- read a file before editing it\n\
                 - search with grep and find rather than the shell\n\
                 - every edit and write reports what the language server says about the \
                 file it changed: fix an error before moving on\n\
                 - use lsp references and lsp rename rather than a text search when a \
                 symbol is renamed, so every use changes and nothing else does\n\
                 - run the project's own checks after changing code\n\
                 - keep replies short",
            ),
        ],
    }
}

/// the wire the replayed history is shaped for: an openai-compatible chat completions
/// endpoint. rig asks it what it accepts, so the history fits the model reading it.
#[derive(Debug)]
struct ChatTarget {
    model: String,
}

impl rig_core::completion::ReplayTarget for ChatTarget {
    fn api(&self) -> rig_core::completion::message::Api {
        rig_core::completion::message::Api::from_static("openai.chat")
    }

    fn provider(&self) -> &'static str {
        "openai"
    }

    fn model(&self) -> &str {
        &self.model
    }

    fn accepts(&self, _model: &str) -> rig_core::completion::history::Accepts {
        rig_core::completion::history::Accepts::ALL
    }

    fn map_options(
        &self,
        _request: &rig_core::completion::CompletionRequest,
        fields: rig_core::completion::options::OptionFields<'_>,
    ) -> rig_core::completion::options::OptionMap {
        use rig_core::completion::options::{Mapping, OptionMap};
        // the agent sets none of these itself; anything a caller sets is sent as it is.
        const UNSET: &str = "the agent does not set this option";
        OptionMap {
            reasoning: Mapping::of(fields.reasoning, |_| Mapping::Send(serde_json::json!({}))),
            cache: Mapping::of(fields.cache, |_| Mapping::Omit(UNSET)),
            service_tier: Mapping::of(fields.service_tier, |_| Mapping::Omit(UNSET)),
            verbosity: Mapping::of(fields.verbosity, |_| Mapping::Omit(UNSET)),
            parallel_tool_calls: Mapping::of(fields.parallel_tool_calls, |_| Mapping::Omit(UNSET)),
            top_p: Mapping::of(fields.top_p, |_| Mapping::Omit(UNSET)),
            seed: Mapping::of(fields.seed, |_| Mapping::Omit(UNSET)),
            stop: Mapping::of_stop(fields.stop, |_| Mapping::Omit(UNSET)),
            cache_key: Mapping::of(fields.cache_key, |_| Mapping::Omit(UNSET)),
        }
    }
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
