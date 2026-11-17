//! openai-compatible model client (spec 10.2).
//!
//! one `Provider` implementation on the wire, behind a trait so the agent loop can be driven
//! by a scripted provider in tests without touching the network.

use std::future::Future;
use std::pin::Pin;

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::mpsc;

#[derive(Debug, thiserror::Error)]
pub enum LlmError {
    #[error("http: {0}")]
    Http(String),
    #[error("model returned {status}: {body}")]
    Status { status: u16, body: String },
    #[error("malformed stream: {0}")]
    Stream(String),
    #[error("no credentials configured")]
    NoCredentials,
    #[error("provider closed the stream without a completion")]
    Truncated,
}

/// one part of a multimodal message.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentPart {
    Text { text: String },
    ImageUrl { image_url: ImageUrl },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageUrl {
    pub url: String,
}

/// message content: text, or text with images. serialised as a bare string when it is
/// only text, which is what every endpoint expects for the ordinary case.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MessageContent {
    Text(String),
    Parts(Vec<ContentPart>),
}

impl MessageContent {
    /// the text, when the content is text only.
    #[must_use]
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text(text) => Some(text),
            Self::Parts(_) => None,
        }
    }
}

/// one message in the openai chat-completions shape.
///
/// tool results must carry the `tool_call_id` of the call they answer, and the assistant
/// message that requested them must carry the calls verbatim, or the model cannot associate
/// a result with a call and will re-issue it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub content: Option<MessageContent>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub tool_calls: Option<Vec<WireToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub tool_call_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WireToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub function: WireFunction,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WireFunction {
    pub name: String,
    pub arguments: String,
}

impl From<&ToolCall> for WireToolCall {
    fn from(call: &ToolCall) -> Self {
        Self {
            id: call.id.clone(),
            kind: "function".to_owned(),
            function: WireFunction {
                name: call.name.clone(),
                arguments: call.arguments.clone(),
            },
        }
    }
}

impl ChatMessage {
    #[must_use]
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: "system".to_owned(),
            content: Some(MessageContent::Text(content.into())),
            tool_calls: None,
            tool_call_id: None,
        }
    }

    #[must_use]
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: "user".to_owned(),
            content: Some(MessageContent::Text(content.into())),
            tool_calls: None,
            tool_call_id: None,
        }
    }

    /// a user turn carrying images alongside its text.
    #[must_use]
    pub fn user_with_images(
        content: impl Into<String>,
        images: &[sa_proto::session::ImageAttachment],
    ) -> Self {
        let mut parts = vec![ContentPart::Text {
            text: content.into(),
        }];
        for image in images {
            parts.push(ContentPart::ImageUrl {
                image_url: ImageUrl {
                    url: image.data_url(),
                },
            });
        }
        Self {
            role: "user".to_owned(),
            content: Some(MessageContent::Parts(parts)),
            tool_calls: None,
            tool_call_id: None,
        }
    }

    #[must_use]
    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: "assistant".to_owned(),
            content: Some(MessageContent::Text(content.into())),
            tool_calls: None,
            tool_call_id: None,
        }
    }

    /// the assistant turn that requested tool calls. `content` may be empty.
    #[must_use]
    pub fn assistant_with_calls(content: &str, calls: &[ToolCall]) -> Self {
        Self {
            role: "assistant".to_owned(),
            content: if content.is_empty() {
                None
            } else {
                Some(MessageContent::Text(content.to_owned()))
            },
            tool_calls: Some(calls.iter().map(WireToolCall::from).collect()),
            tool_call_id: None,
        }
    }

    /// the result of one tool call, addressed to the call that requested it.
    #[must_use]
    pub fn tool(call_id: &str, content: impl Into<String>) -> Self {
        Self {
            role: "tool".to_owned(),
            content: Some(MessageContent::Text(content.into())),
            tool_calls: None,
            tool_call_id: Some(call_id.to_owned()),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

#[derive(Debug, Clone)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    pub tools: Vec<ToolSpec>,
    pub max_tokens: Option<u64>,
    pub reasoning_effort: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Delta {
    Text(String),
    Reasoning(String),
    ToolCall(ToolCall),
    Done {
        stop_reason: String,
        prompt_tokens: u64,
        completion_tokens: u64,
    },
}

/// boxed stream future, so the trait stays object-safe without an async-trait dependency.
pub type StreamFuture<'a> =
    Pin<Box<dyn Future<Output = Result<mpsc::Receiver<Delta>, LlmError>> + Send + 'a>>;

/// boxed future for model discovery.
pub type ModelsFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Vec<ModelInfo>, LlmError>> + Send + 'a>>;

/// what an endpoint says about a model it serves.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ModelInfo {
    pub id: String,
    /// the model's context window, when the endpoint reports one. absent means unknown,
    /// which is not the same as a guessed default.
    pub context_limit: Option<u64>,
    /// the reasoning effort values the endpoint accepts for this model, in its own order.
    /// empty means it said nothing, and nothing is sent.
    pub effort_levels: Vec<String>,
}

/// the context window an endpoint advertises for a model, under the shapes seen in the
/// wild: openrouter nests it, others put it at the top level.
#[must_use]
pub fn advertised_context(model: &Value) -> Option<u64> {
    const KEYS: [&str; 3] = ["context_length", "max_context_length", "max_input_tokens"];
    for key in KEYS {
        if let Some(limit) = model.get(key).and_then(Value::as_u64) {
            return Some(limit);
        }
    }
    for nested in ["top_provider", "meta"] {
        for key in KEYS {
            if let Some(limit) = model
                .get(nested)
                .and_then(|value| value.get(key))
                .and_then(Value::as_u64)
            {
                return Some(limit);
            }
        }
    }
    None
}

/// the reasoning effort values an endpoint advertises for a model, under the shapes seen in
/// the wild. empty means the endpoint said nothing about effort at all.
#[must_use]
pub fn advertised_efforts(model: &Value) -> Vec<String> {
    const PATHS: [&str; 4] = [
        "/reasoning_effort_levels",
        "/effort_levels",
        "/reasoning/effort_levels",
        "/reasoning_effort/values",
    ];
    for path in PATHS {
        let Some(items) = model.pointer(path).and_then(Value::as_array) else {
            continue;
        };
        let levels: Vec<String> = items
            .iter()
            .filter_map(|item| {
                item.as_str().map(str::to_owned).or_else(|| {
                    item.get("value")
                        .or_else(|| item.get("name"))
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                })
            })
            .collect();
        if !levels.is_empty() {
            return levels;
        }
    }
    Vec::new()
}

pub trait Provider: Send + Sync {
    fn stream(&self, request: ChatRequest) -> StreamFuture<'_>;

    /// the models the endpoint actually serves. empty when unknown.
    fn list_models(&self) -> ModelsFuture<'_> {
        Box::pin(async { Ok(Vec::new()) })
    }
}

/// talks to any openai-compatible chat completions endpoint.
pub struct OpenAiProvider {
    endpoint: String,
    api_key: String,
    client: reqwest::Client,
}

impl OpenAiProvider {
    pub fn new(endpoint: impl Into<String>, api_key: impl Into<String>) -> Result<Self, LlmError> {
        let client = reqwest::Client::builder()
            .build()
            .map_err(|e| LlmError::Http(e.to_string()))?;
        Ok(Self {
            endpoint: endpoint.into().trim_end_matches('/').to_owned(),
            api_key: api_key.into(),
            client,
        })
    }

    /// reads `OPENAI_API_ENDPOINT` and `OPENAI_API_TOKEN` from the environment.
    pub fn from_env() -> Result<Self, LlmError> {
        let endpoint = std::env::var("OPENAI_API_ENDPOINT")
            .unwrap_or_else(|_| "https://api.openai.com/v1".to_owned());
        let api_key = std::env::var("OPENAI_API_TOKEN").map_err(|_| LlmError::NoCredentials)?;
        Self::new(endpoint, api_key)
    }
}

impl OpenAiProvider {
    /// asks the endpoint what it is serving, so the daemon never has to guess.
    pub async fn list_models(&self) -> Result<Vec<ModelInfo>, LlmError> {
        let url = format!("{}/models", self.endpoint);
        let response = self
            .client
            .get(&url)
            .bearer_auth(&self.api_key)
            .send()
            .await
            .map_err(|e| LlmError::Http(e.to_string()))?;
        let status = response.status();
        if !status.is_success() {
            return Err(LlmError::Status {
                status: status.as_u16(),
                body: response.text().await.unwrap_or_default(),
            });
        }
        let body: Value = response
            .json()
            .await
            .map_err(|e| LlmError::Http(e.to_string()))?;
        Ok(body
            .get("data")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|model| {
                        let id = model.get("id").and_then(Value::as_str)?;
                        Some(ModelInfo {
                            id: id.to_owned(),
                            context_limit: advertised_context(model),
                            effort_levels: advertised_efforts(model),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default())
    }
}

impl Provider for OpenAiProvider {
    fn list_models(&self) -> ModelsFuture<'_> {
        Box::pin(async move { Self::list_models(self).await })
    }

    fn stream(&self, request: ChatRequest) -> StreamFuture<'_> {
        Box::pin(async move {
            let url = format!("{}/chat/completions", self.endpoint);
            let body = json!({
                "model": request.model,
                "messages": request.messages,
                "tools": request.tools.iter().map(|t| json!({
                    "type": "function",
                    "function": {
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.parameters,
                    }
                })).collect::<Vec<_>>(),
                "stream": true,
                "stream_options": {"include_usage": true},
                "max_tokens": request.max_tokens,
                "reasoning_effort": request.reasoning_effort,
            });

            let response = self
                .client
                .post(&url)
                .bearer_auth(&self.api_key)
                .json(&body)
                .send()
                .await
                .map_err(|e| LlmError::Http(e.to_string()))?;

            let status = response.status();
            if !status.is_success() {
                let body = response.text().await.unwrap_or_default();
                return Err(LlmError::Status {
                    status: status.as_u16(),
                    body,
                });
            }

            let (tx, rx) = mpsc::channel(256);
            let mut stream = response.bytes_stream();
            tokio::spawn(async move {
                let mut parser = SseParser::default();
                let mut calls: Vec<ToolCall> = Vec::new();
                let mut prompt_tokens = 0;
                let mut completion_tokens = 0;
                let mut stop_reason = String::from("stop");

                while let Some(chunk) = stream.next().await {
                    let Ok(bytes) = chunk else { break };
                    for payload in parser.feed(&bytes) {
                        if payload == "[DONE]" {
                            continue;
                        }
                        let Ok(value) = serde_json::from_str::<Value>(&payload) else {
                            continue;
                        };
                        if let Some(usage) = value.get("usage").filter(|u| !u.is_null()) {
                            prompt_tokens = usage
                                .get("prompt_tokens")
                                .and_then(Value::as_u64)
                                .unwrap_or(0);
                            completion_tokens = usage
                                .get("completion_tokens")
                                .and_then(Value::as_u64)
                                .unwrap_or(0);
                        }
                        let Some(choice) = value
                            .get("choices")
                            .and_then(|c| c.get(0))
                            .filter(|c| !c.is_null())
                        else {
                            continue;
                        };
                        if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
                            stop_reason = reason.to_owned();
                        }
                        let delta = choice.get("delta").cloned().unwrap_or(Value::Null);
                        if let Some(text) = delta.get("content").and_then(Value::as_str) {
                            if !text.is_empty()
                                && tx.send(Delta::Text(text.to_owned())).await.is_err()
                            {
                                return;
                            }
                        }
                        if let Some(text) = delta.get("reasoning_content").and_then(Value::as_str) {
                            if !text.is_empty()
                                && tx.send(Delta::Reasoning(text.to_owned())).await.is_err()
                            {
                                return;
                            }
                        }
                        if let Some(tool_calls) = delta.get("tool_calls").and_then(Value::as_array)
                        {
                            for call in tool_calls {
                                let index =
                                    call.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                                while calls.len() <= index {
                                    calls.push(ToolCall {
                                        id: String::new(),
                                        name: String::new(),
                                        arguments: String::new(),
                                    });
                                }
                                let slot = &mut calls[index];
                                if let Some(id) = call.get("id").and_then(Value::as_str) {
                                    slot.id = id.to_owned();
                                }
                                if let Some(function) = call.get("function") {
                                    if let Some(name) = function.get("name").and_then(Value::as_str)
                                    {
                                        slot.name.push_str(name);
                                    }
                                    if let Some(args) =
                                        function.get("arguments").and_then(Value::as_str)
                                    {
                                        slot.arguments.push_str(args);
                                    }
                                }
                            }
                        }
                    }
                }

                for call in calls {
                    if call.name.is_empty() {
                        continue;
                    }
                    if tx.send(Delta::ToolCall(call)).await.is_err() {
                        return;
                    }
                }
                let _ = tx
                    .send(Delta::Done {
                        stop_reason,
                        prompt_tokens,
                        completion_tokens,
                    })
                    .await;
            });

            Ok(rx)
        })
    }
}

/// incremental server-sent-events splitter for the chat completions stream.
#[derive(Default)]
struct SseParser {
    buffer: Vec<u8>,
}

impl SseParser {
    fn feed(&mut self, chunk: &[u8]) -> Vec<String> {
        self.buffer.extend_from_slice(chunk);
        let mut out = Vec::new();
        while let Some(pos) = find(&self.buffer, b"\n\n") {
            let block: Vec<u8> = self.buffer.drain(..pos + 2).collect();
            for line in block.split(|b| *b == b'\n') {
                let line = String::from_utf8_lossy(line);
                let line = line.trim_end_matches('\r');
                if let Some(data) = line.strip_prefix("data:") {
                    let data = data.trim();
                    if !data.is_empty() {
                        out.push(data.to_owned());
                    }
                }
            }
        }
        out
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// stands in when no credentials are configured. the daemon still starts, so session
/// listing and replay work; only turns fail.
pub struct MissingProvider;

impl Provider for MissingProvider {
    fn stream(&self, _request: ChatRequest) -> StreamFuture<'_> {
        Box::pin(async { Err(LlmError::NoCredentials) })
    }
}

/// a provider that replays a fixed script. used by every test that must not touch the network.
pub struct ScriptedProvider {
    script: std::sync::Mutex<std::collections::VecDeque<Vec<Delta>>>,
    seen: std::sync::Mutex<Vec<ChatRequest>>,
    models: Vec<ModelInfo>,
}

impl ScriptedProvider {
    #[must_use]
    pub fn new(script: Vec<Vec<Delta>>) -> Self {
        Self {
            script: std::sync::Mutex::new(script.into_iter().collect()),
            seen: std::sync::Mutex::new(Vec::new()),
            models: Vec::new(),
        }
    }

    /// advertises the given model ids, as a real endpoint would.
    #[must_use]
    pub fn with_models(mut self, models: Vec<String>) -> Self {
        self.models = models
            .into_iter()
            .map(|id| ModelInfo {
                id,
                context_limit: None,
                effort_levels: Vec::new(),
            })
            .collect();
        self
    }

    /// advertises a model with the context window an endpoint would report for it.
    #[must_use]
    pub fn with_context(mut self, id: &str, context_limit: u64) -> Self {
        self.models = vec![ModelInfo {
            id: id.to_owned(),
            context_limit: Some(context_limit),
            effort_levels: Vec::new(),
        }];
        self
    }

    /// advertises a model that accepts these reasoning effort values.
    #[must_use]
    pub fn with_efforts(mut self, id: &str, levels: &[&str]) -> Self {
        self.models = vec![ModelInfo {
            id: id.to_owned(),
            context_limit: None,
            effort_levels: levels.iter().map(|level| (*level).to_owned()).collect(),
        }];
        self
    }

    /// every request the agent has made, in order.
    #[must_use]
    pub fn seen_requests(&self) -> Vec<ChatRequest> {
        self.seen
            .lock()
            .map(|seen| seen.clone())
            .unwrap_or_default()
    }
}

impl Provider for ScriptedProvider {
    fn list_models(&self) -> ModelsFuture<'_> {
        let models = self.models.clone();
        Box::pin(async move { Ok(models) })
    }

    fn stream(&self, request: ChatRequest) -> StreamFuture<'_> {
        Box::pin(async move {
            if let Ok(mut seen) = self.seen.lock() {
                seen.push(request);
            }
            let turn = self
                .script
                .lock()
                .map_err(|_| LlmError::Stream("script lock poisoned".to_owned()))?
                .pop_front()
                .unwrap_or_default();
            let (tx, rx) = mpsc::channel(256);
            tokio::spawn(async move {
                for delta in turn {
                    if tx.send(delta).await.is_err() {
                        return;
                    }
                }
            });
            Ok(rx)
        })
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn sse_parser_handles_split_chunks_and_multiple_events() {
        let mut parser = SseParser::default();
        assert_eq!(
            parser.feed(b"data: {\"a\":1}"),
            [] as [std::string::String; 0]
        );
        let out = parser.feed(b"\n\ndata: {\"b\":2}\n\n");
        assert_eq!(out, vec!["{\"a\":1}".to_owned(), "{\"b\":2}".to_owned()]);
    }

    #[test]
    fn sse_parser_ignores_comments_and_blank_lines() {
        let mut parser = SseParser::default();
        let out = parser.feed(b": keepalive\n\ndata: [DONE]\n\n");
        assert_eq!(out, vec!["[DONE]".to_owned()]);
    }

    #[tokio::test]
    async fn scripted_provider_replays_in_order() {
        let provider = ScriptedProvider::new(vec![vec![
            Delta::Text("hello".to_owned()),
            Delta::Done {
                stop_reason: "stop".to_owned(),
                prompt_tokens: 1,
                completion_tokens: 1,
            },
        ]]);
        let mut rx = provider
            .stream(ChatRequest {
                model: "m".to_owned(),
                messages: Vec::new(),
                tools: Vec::new(),
                max_tokens: None,
                reasoning_effort: None,
            })
            .await
            .expect("stream");

        assert_eq!(rx.recv().await, Some(Delta::Text("hello".to_owned())));
        assert!(matches!(rx.recv().await, Some(Delta::Done { .. })));
        assert_eq!(rx.recv().await, None);
    }
}
