//! openai-compatible model client (spec 10.2).
//!
//! one `Provider` implementation on the wire, behind a trait so the agent loop can be driven
//! by a scripted provider in tests without touching the network.

use std::future::Future;
use std::pin::Pin;

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;
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
    /// the stream failed after it started. carried as a delta so the turn fails with the
    /// reason rather than completing with an empty answer.
    Failed(String),
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

/// talks to any openai-compatible chat completions endpoint, through rig's provider.
///
/// the wire work — sse framing, tool-call deltas, retries, the reasoning field a gateway
/// uses — is rig's. what stays here is the shape this project speaks: our message and tool
/// types, and the `Delta` stream the agent records.
pub struct OpenAiProvider {
    client: rig_core::providers::openai::OpenAI,
}

impl OpenAiProvider {
    pub fn new(endpoint: impl Into<String>, api_key: impl Into<String>) -> Result<Self, LlmError> {
        let mut config = rig_core::providers::openai::OpenAIConfig::new(api_key);
        config.base_url = endpoint.into().trim_end_matches('/').to_owned();
        Ok(Self {
            client: config.client(),
        })
    }

    /// reads `OPENAI_API_ENDPOINT` and `OPENAI_API_TOKEN` from the environment.
    pub fn from_env() -> Result<Self, LlmError> {
        let endpoint = std::env::var("OPENAI_API_ENDPOINT")
            .unwrap_or_else(|_| "https://api.openai.com/v1".to_owned());
        let api_key = std::env::var("OPENAI_API_TOKEN").map_err(|_| LlmError::NoCredentials)?;
        Self::new(endpoint, api_key)
    }

    /// asks the endpoint what it is serving, so the daemon never has to guess.
    pub async fn list_models(&self) -> Result<Vec<String>, LlmError> {
        let listed = self
            .client
            .list_models()
            .await
            .map_err(|err| LlmError::Http(err.to_string()))?;
        Ok(listed.data.into_iter().map(|model| model.id).collect())
    }
}

/// our transcript, in the shape rig sends.
fn rig_messages(messages: &[ChatMessage]) -> Result<Vec<rig_core::completion::Message>, LlmError> {
    use rig_core::completion::message::{AssistantMessage, UserContent};
    use rig_core::completion::{AssistantContent, Message};

    let mut out = Vec::new();
    // a tool result must name its tool, and the assistant turn that asked for it knows it.
    let mut names: std::collections::HashMap<String, String> = std::collections::HashMap::new();

    for message in messages {
        let text = message
            .content
            .as_ref()
            .and_then(MessageContent::as_text)
            .unwrap_or_default()
            .to_owned();
        match message.role.as_str() {
            "system" => out.push(Message::system(text)),
            "user" => {
                let content = match &message.content {
                    Some(MessageContent::Parts(parts)) => parts
                        .iter()
                        .map(|part| match part {
                            ContentPart::Text { text } => UserContent::text(text.clone()),
                            ContentPart::ImageUrl { image_url } => {
                                UserContent::image_url(image_url.url.clone(), None, None)
                            }
                        })
                        .collect(),
                    _ => vec![UserContent::text(text)],
                };
                out.push(Message::User { content });
            }
            "assistant" => {
                let mut content: Vec<AssistantContent> = Vec::new();
                if !text.is_empty() {
                    content.push(AssistantContent::text(text));
                }
                for call in message.tool_calls.iter().flatten() {
                    names.insert(call.id.clone(), call.function.name.clone());
                    let arguments: serde_json::Value =
                        serde_json::from_str(&call.function.arguments)
                            .unwrap_or_else(|_| serde_json::json!({}));
                    // built from the same CallId the result will use, or the wire sees two
                    // different ids for one call and the model re-issues it.
                    content.push(AssistantContent::ToolCall(
                        rig_core::completion::message::ToolCall::new(
                            rig_call_id(&call.id)?,
                            rig_core::completion::message::ToolFunction::new(
                                tool_name(&call.function.name)?,
                                arguments,
                            ),
                        ),
                    ));
                }
                out.push(Message::Assistant(AssistantMessage::new(content)));
            }
            "tool" => {
                let id = message.tool_call_id.clone().unwrap_or_default();
                let name = names.get(&id).cloned().unwrap_or_default();
                out.push(Message::tool_result(
                    rig_call_id(&id)?,
                    tool_name(&name)?,
                    text,
                ));
            }
            other => {
                return Err(LlmError::Stream(format!("unknown message role: {other}")));
            }
        }
    }
    Ok(out)
}

/// rig's identifiers are newtypes that refuse an empty string, so building one is a step
/// that can fail rather than a cast.
fn rig_call_id(raw: &str) -> Result<rig_core::completion::message::CallId, LlmError> {
    rig_core::completion::message::ProviderCallId::new(raw)
        .map(rig_core::completion::message::CallId::from)
        .ok_or_else(|| LlmError::Stream(format!("empty tool call id: {raw:?}")))
}

fn tool_name(raw: &str) -> Result<rig_core::completion::message::ToolName, LlmError> {
    rig_core::completion::message::ToolName::new(raw)
        .map_err(|_| LlmError::Stream(format!("empty tool name: {raw:?}")))
}

/// our request, in the shape rig sends.
fn rig_request(request: &ChatRequest) -> Result<rig_core::completion::CompletionRequest, LlmError> {
    use rig_core::completion::ToolDefinition;

    // rig sends the history first and the "prompt" last, so the newest message is the
    // prompt and everything before it is history. the other way round reorders the
    // conversation and the model loses the thread.
    let mut messages = rig_messages(&request.messages)?;
    let Some(prompt) = messages.pop() else {
        return Err(LlmError::Stream(
            "a request needs at least one message".to_owned(),
        ));
    };
    let mut completion = rig_core::completion::CompletionRequest::new(prompt).messages(messages);
    if let Some(max_tokens) = request.max_tokens {
        completion = completion.max_tokens(max_tokens);
    }
    if !request.tools.is_empty() {
        completion = completion.tools(
            request
                .tools
                .iter()
                .filter_map(|tool| {
                    Some(ToolDefinition::new(
                        tool_name(&tool.name).ok()?,
                        tool.description.clone(),
                        tool.parameters.clone(),
                    ))
                })
                .collect::<Vec<ToolDefinition>>(),
        );
    }
    if let Some(effort) = &request.reasoning_effort {
        completion =
            completion.additional_params(serde_json::json!({ "reasoning_effort": effort }));
    }
    Ok(completion)
}

impl Provider for OpenAiProvider {
    fn stream(&self, request: ChatRequest) -> StreamFuture<'_> {
        Box::pin(async move {
            use rig_core::streaming::{Item, StreamEvent};

            // the chat-completions wire: openai-compatible gateways serve this route,
            // while the default wire is the responses api and 404s on them.
            let model = self.client.chat(request.model.clone());
            let mut stream = model
                .stream(rig_request(&request)?)
                .map_err(|err| LlmError::Http(err.to_string()))?;

            let (tx, rx) = tokio::sync::mpsc::channel(256);
            tokio::spawn(async move {
                use futures_util::StreamExt as _;

                while let Some(item) = stream.next().await {
                    let event = match item {
                        Ok(Item::Event(event)) => event,
                        Ok(Item::Unknown(_)) => continue,
                        Err(err) => {
                            let _ = tx.send(Delta::Failed(err.to_string())).await;
                            return;
                        }
                    };
                    let delta = match event {
                        StreamEvent::Text { text, .. } => Delta::Text(text),
                        StreamEvent::Reasoning { text, .. } => Delta::Reasoning(text),
                        StreamEvent::End {
                            content: rig_core::completion::AssistantContent::ToolCall(call),
                            ..
                        } => Delta::ToolCall(ToolCall {
                            id: call.id.to_string(),
                            name: call.function.name.to_string(),
                            arguments: serde_json::Value::Object(call.function.arguments)
                                .to_string(),
                        }),
                        _ => continue,
                    };
                    if tx.send(delta).await.is_err() {
                        return;
                    }
                }

                // usage and the stop reason come from the accumulated reply.
                let partial = stream.partial();
                let _ = tx
                    .send(Delta::Done {
                        stop_reason: "stop".to_owned(),
                        prompt_tokens: partial.usage.input_tokens.unwrap_or_default(),
                        completion_tokens: partial.usage.output_tokens.unwrap_or_default(),
                    })
                    .await;
            });

            Ok(rx)
        })
    }
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
    fn a_transcript_maps_onto_the_providers_shape() {
        use rig_core::completion::Message;

        let messages = vec![
            ChatMessage::system("be brief"),
            ChatMessage::user("hello"),
            ChatMessage {
                role: "assistant".to_owned(),
                content: Some(MessageContent::Text("calling".to_owned())),
                tool_calls: Some(vec![WireToolCall {
                    id: "c1".to_owned(),
                    kind: "function".to_owned(),
                    function: WireFunction {
                        name: "read".to_owned(),
                        arguments: "{\"path\":\"a\"}".to_owned(),
                    },
                }]),
                tool_call_id: None,
            },
            ChatMessage {
                role: "tool".to_owned(),
                content: Some(MessageContent::Text("file body".to_owned())),
                tool_calls: None,
                tool_call_id: Some("c1".to_owned()),
            },
        ];

        let mapped = rig_messages(&messages).expect("mapped");
        assert!(matches!(mapped[0], Message::System { .. }));
        assert!(matches!(mapped[1], Message::User { .. }));
        assert!(matches!(mapped[2], Message::Assistant(_)));
        // the result must name the tool it answers, or the model cannot pair them.
        let rendered = format!("{:?}", mapped[3]);
        assert!(rendered.contains("file body"), "{rendered}");
        assert!(
            rendered.contains("read"),
            "the tool keeps its name: {rendered}"
        );
    }

    #[test]
    fn a_request_carries_the_tools_the_cap_and_the_effort() {
        let request = ChatRequest {
            model: "m".to_owned(),
            messages: vec![ChatMessage::system("be brief"), ChatMessage::user("hi")],
            tools: vec![ToolSpec {
                name: "read".to_owned(),
                description: "Read a file.".to_owned(),
                parameters: serde_json::json!({"type": "object"}),
            }],
            max_tokens: Some(64),
            reasoning_effort: Some("high".to_owned()),
        };

        let built = format!("{:?}", rig_request(&request).expect("request"));
        assert!(
            built.contains("be brief"),
            "the system turn survives: {built}"
        );
        assert!(built.contains("hi"), "the user turn survives: {built}");
        assert!(built.contains("read"), "{built}");
        assert!(built.contains("64"), "the cap is carried: {built}");
        assert!(built.contains("high"), "the effort is carried: {built}");
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
