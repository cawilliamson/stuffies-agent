//! openai-compatible model client (spec 10.2).
//!
//! one `Provider` implementation on the wire, behind a trait so the agent loop can be driven
//! by a scripted provider in tests without touching the network.

use std::future::Future;
use std::pin::Pin;

use serde::Serialize;
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
#[derive(Debug, Clone, Serialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

#[derive(Debug, Clone)]
pub struct ChatRequest {
    pub model: String,
    /// the system prompt as data: rig renders the sections when the request is sent.
    pub system: Option<rig_core::completion::prompt::SystemPrompt>,
    /// the conversation, in rig's shape. rig owns the message model, the wire and the
    /// pairing rules; nothing here mirrors it.
    pub messages: Vec<rig_core::completion::Message>,
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

/// the handful of message shapes a turn builds, in rig's model. rig owns the type, the wire
/// and the pairing rules; these only spare every call site rig's fallible identifiers.
pub mod message {
    use super::{LlmError, ToolCall};
    use rig_core::completion::message::{AssistantMessage, UserContent};
    use rig_core::completion::{AssistantContent, Message};

    #[must_use]
    pub fn system(content: impl Into<String>) -> Message {
        Message::system(content)
    }

    #[must_use]
    pub fn user(content: impl Into<String>) -> Message {
        Message::user(content)
    }

    #[must_use]
    pub fn assistant(content: impl Into<String>) -> Message {
        Message::assistant(content)
    }

    /// a user turn carrying images alongside its text.
    #[must_use]
    pub fn user_with_images(
        content: impl Into<String>,
        images: &[sa_proto::session::ImageAttachment],
    ) -> Message {
        let mut parts = vec![UserContent::text(content.into())];
        for image in images {
            parts.push(UserContent::image_url(image.data_url(), None, None));
        }
        Message::User { content: parts }
    }

    /// the assistant turn that requested tool calls. `content` may be empty.
    pub fn assistant_with_calls(content: &str, calls: &[ToolCall]) -> Result<Message, LlmError> {
        let mut parts = Vec::with_capacity(calls.len() + 1);
        if !content.is_empty() {
            parts.push(AssistantContent::text(content));
        }
        for call in calls {
            parts.push(AssistantContent::ToolCall(tool_call(call)?));
        }
        Ok(Message::Assistant(AssistantMessage::new(parts)))
    }

    /// the result of one tool call, addressed to the call that requested it.
    pub fn tool_result(call: &ToolCall, content: impl Into<String>) -> Result<Message, LlmError> {
        Ok(Message::tool_result(
            super::rig_call_id(&call.id)?,
            super::tool_name(&call.name)?,
            content.into(),
        ))
    }

    fn tool_call(call: &ToolCall) -> Result<rig_core::completion::message::ToolCall, LlmError> {
        let arguments =
            serde_json::from_str(&call.arguments).unwrap_or_else(|_| serde_json::json!({}));
        Ok(rig_core::completion::message::ToolCall::new(
            super::rig_call_id(&call.id)?,
            rig_core::completion::message::ToolFunction::new(
                super::tool_name(&call.name)?,
                arguments,
            ),
        ))
    }
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

/// what a probe learned about a server's effort values.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DiscoveredEfforts {
    /// the values that behave differently from each other, one per behaviour.
    pub levels: Vec<String>,
    /// the value whose behaviour matches the server's own default, if one does.
    pub default: Option<String>,
}

/// boxed future for effort discovery.
pub type EffortsFuture<'a> = Pin<Box<dyn Future<Output = DiscoveredEfforts> + Send + 'a>>;

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
    /// the level whose behaviour matches the server's own default, when one was probed. it
    /// is what a session uses until the operator chooses, so the client can show it.
    pub default_effort: Option<String>,
}

/// the effort values worth trying when an endpoint advertises none.
///
/// they are candidates, not a list: only the ones a probe shows the server distinguishing
/// are kept. ordered so a behaviour is named by its canonical value rather than an alias,
/// so a server that treats `minimal` as `low` reports the level as `low`.
pub const EFFORT_CANDIDATES: [&str; 8] = [
    "none", "low", "high", "max", "minimal", "medium", "xhigh", "ultra",
];
/// a value no server knows, used to see what "ignored" looks like.
const EFFORT_SENTINEL: &str = "sa-probe-unknown-effort";

/// the context window an endpoint advertises for a model, under the shapes seen in the
/// wild: openrouter nests it, others put it at the top level.
#[must_use]
pub fn advertised_context(model: &Value) -> Option<u64> {
    // `max_model_len` is what a vllm-backed gateway reports, and is the one that matters
    // in practice; the rest are the shapes other openai-compatible servers use.
    const KEYS: [&str; 4] = [
        "max_model_len",
        "context_length",
        "max_context_length",
        "max_input_tokens",
    ];
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

    /// the effort values this endpoint honours, asked of a server that publishes none.
    fn probe_efforts<'a>(&'a self, _model: &'a str) -> EffortsFuture<'a> {
        Box::pin(async { DiscoveredEfforts::default() })
    }
}

/// talks to any openai-compatible chat completions endpoint, through rig's provider.
///
/// the wire work — sse framing, tool-call deltas, retries, the reasoning field a gateway
/// uses — is rig's. what stays here is the shape this project speaks: our message and tool
/// types, and the `Delta` stream the agent records.
pub struct OpenAiProvider {
    client: rig_core::providers::openai::OpenAI,
    endpoint: String,
    api_key: String,
}

impl OpenAiProvider {
    pub fn new(endpoint: impl Into<String>, api_key: impl Into<String>) -> Result<Self, LlmError> {
        let endpoint = endpoint.into();
        let api_key = api_key.into();
        let mut config = rig_core::providers::openai::OpenAIConfig::new(api_key.clone());
        config.base_url = endpoint.trim_end_matches('/').to_owned();
        Ok(Self {
            client: config.client(),
            endpoint,
            api_key,
        })
    }

    /// reads `OPENAI_API_ENDPOINT` and `OPENAI_API_TOKEN` from the environment.
    pub fn from_env() -> Result<Self, LlmError> {
        let endpoint = std::env::var("OPENAI_API_ENDPOINT")
            .unwrap_or_else(|_| "https://api.openai.com/v1".to_owned());
        let api_key = std::env::var("OPENAI_API_TOKEN").map_err(|_| LlmError::NoCredentials)?;
        Self::new(endpoint, api_key)
    }

    /// asks the endpoint which effort values it honours, by sending the same probe with
    /// each candidate and with a value no server knows. a value that behaves like the
    /// unknown one is being ignored; one that behaves differently is real. this is the only
    /// way to learn them from a server that publishes no list.
    pub async fn discover_efforts(&self, model: &str) -> DiscoveredEfforts {
        let Some(control) = self.probe_one(model, EFFORT_SENTINEL).await else {
            // the probe itself failed: say nothing rather than invent a list.
            return DiscoveredEfforts::default();
        };

        let mut probed = Vec::new();
        for candidate in EFFORT_CANDIDATES {
            if let Some(profile) = self.probe_one(model, candidate).await {
                probed.push((candidate.to_owned(), profile));
            }
        }
        group_efforts(control, probed)
    }

    /// one probe: how much reasoning and answer text the model emits for this effort value,
    /// if it answers. reasoning length is the measure of effort; answer length breaks ties
    /// between levels that think the same amount.
    async fn probe_one(&self, model: &str, effort: &str) -> Option<Profile> {
        let body = serde_json::json!({
            "model": model,
            "messages": [{"role": "user", "content": "Think it through, then answer: what is 17*23?"}],
            "max_tokens": 96,
            "temperature": 0,
            "reasoning_effort": effort,
        });
        let response = reqwest::Client::new()
            .post(format!("{}/chat/completions", self.endpoint))
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
            .ok()?;
        if !response.status().is_success() {
            return None;
        }
        let value: serde_json::Value = response.json().await.ok()?;
        let message = &value["choices"][0]["message"];
        let reasoning = ["reasoning_content", "reasoning", "reasoning_text"]
            .iter()
            .find_map(|key| message.get(*key).and_then(serde_json::Value::as_str))
            .map_or(0, |text| text.chars().count());
        // the shape of the answer is the profile: a level that changes how the model thinks
        // changes this, and one the server ignores does not.
        let content = message
            .get("content")
            .and_then(serde_json::Value::as_str)
            .map_or(0, |text| text.chars().count());
        Some(Profile { reasoning, content })
    }

    /// asks the endpoint what it is serving, with the fields only it knows: the context
    /// window and the effort values. read from the raw payload, because the typed client
    /// drops whatever a given server chose to call them.
    pub async fn list_models(&self) -> Result<Vec<ModelInfo>, LlmError> {
        let url = format!("{}/models", self.endpoint);
        let response = reqwest::Client::new()
            .get(&url)
            .bearer_auth(&self.api_key)
            .send()
            .await
            .map_err(|err| LlmError::Http(err.to_string()))?;
        let status = response.status();
        if !status.is_success() {
            return Err(LlmError::Status {
                status: status.as_u16(),
                body: response.text().await.unwrap_or_default(),
            });
        }
        let body: serde_json::Value = response
            .json()
            .await
            .map_err(|err| LlmError::Http(err.to_string()))?;
        Ok(body
            .get("data")
            .and_then(serde_json::Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|model| {
                        let id = model.get("id").and_then(serde_json::Value::as_str)?;
                        Some(ModelInfo {
                            id: id.to_owned(),
                            context_limit: advertised_context(model),
                            effort_levels: advertised_efforts(model),
                            default_effort: None,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default())
    }
}

/// what one probe measured: how much the model thought, and how much it then said.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Profile {
    /// reasoning characters. this is the measure of effort: a level that thinks harder
    /// emits more of it.
    pub reasoning: usize,
    /// answer characters, which separates levels that reason the same amount.
    pub content: usize,
}

/// the probed effort levels, remembered so a server is asked once rather than on every
/// start. a cache is a plain json file keyed by endpoint and model.
#[derive(Debug)]
pub struct EffortCache {
    path: std::path::PathBuf,
}

impl EffortCache {
    #[must_use]
    pub const fn new(path: std::path::PathBuf) -> Self {
        Self { path }
    }

    fn read(&self) -> std::collections::BTreeMap<String, DiscoveredEfforts> {
        std::fs::read_to_string(&self.path)
            .ok()
            .and_then(|body| serde_json::from_str(&body).ok())
            .unwrap_or_default()
    }

    /// what was learned about this endpoint and model, if anything was.
    #[must_use]
    pub fn get(&self, endpoint: &str, model: &str) -> Option<DiscoveredEfforts> {
        self.read().remove(&effort_key(endpoint, model))
    }

    /// records what was learned, so the next start needs no probe.
    pub fn remember(&self, endpoint: &str, model: &str, found: &DiscoveredEfforts) {
        let mut all = self.read();
        all.insert(effort_key(endpoint, model), found.clone());
        if let Some(parent) = self.path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(body) = serde_json::to_string_pretty(&all) {
            let _ = std::fs::write(&self.path, body);
        }
    }
}

/// the cache key: the same model on another endpoint is another question.
#[must_use]
pub fn effort_key(endpoint: &str, model: &str) -> String {
    format!("{endpoint}|{model}")
}

/// groups probed values by behaviour, ordered from least to most effort.
///
/// one representative per distinct profile, plus the value whose behaviour matches the
/// unknown-value control, which is the server's own default. values that behave like the
/// control are aliases of the default, not new levels. the order comes from the
/// measurements, so `none` (no reasoning at all) leads and the hardest thinking is last.
#[must_use]
pub fn group_efforts(control: Profile, probed: Vec<(String, Profile)>) -> DiscoveredEfforts {
    let mut groups: Vec<(Profile, String)> = Vec::new();
    let mut default = None;

    for (name, profile) in probed {
        // the value that behaves like the control is the server's default, and it is a
        // real level: it is kept once, so it sorts into place with the others.
        if profile == control && default.is_none() {
            default = Some(name.clone());
        }
        if groups.iter().any(|(seen, _)| *seen == profile) {
            continue;
        }
        groups.push((profile, name));
    }

    // ordering comes from the documented ladder, not from the measurements: two levels that
    // reason the same amount differ by a few characters run to run, which would reorder them
    // between probes. the probe decides which values are real; the ladder says how hard they
    // are, and it puts `none` first because it thinks least.
    groups.sort_by_key(|(_, name)| {
        EFFORT_CANDIDATES
            .iter()
            .position(|candidate| candidate == name)
            .unwrap_or(usize::MAX)
    });
    DiscoveredEfforts {
        levels: groups.into_iter().map(|(_, name)| name).collect(),
        default,
    }
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
    let mut messages = request.messages.clone();
    let Some(prompt) = messages.pop() else {
        return Err(LlmError::Stream(
            "a request needs at least one message".to_owned(),
        ));
    };
    let mut completion = rig_core::completion::CompletionRequest::new(prompt).messages(messages);
    if let Some(system) = &request.system {
        completion = completion.with_system(system);
    }
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
    fn probe_efforts<'a>(&'a self, model: &'a str) -> EffortsFuture<'a> {
        Box::pin(async move { self.discover_efforts(model).await })
    }

    fn list_models(&self) -> ModelsFuture<'_> {
        Box::pin(async move { self.list_models().await })
    }

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
                default_effort: None,
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
            default_effort: None,
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
            default_effort: None,
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
    fn a_tool_result_names_the_call_and_the_tool_it_answers() {
        let call = ToolCall {
            id: "c1".to_owned(),
            name: "read".to_owned(),
            arguments: "{\"path\":\"a\"}".to_owned(),
        };
        let assistant = message::assistant_with_calls("calling", std::slice::from_ref(&call))
            .expect("assistant");
        let result = message::tool_result(&call, "file body").expect("result");

        // the result must name the call and the tool it answers, or the model re-issues it.
        let rendered = format!("{result:?}");
        assert!(rendered.contains("file body"), "{rendered}");
        assert!(rendered.contains("read"), "{rendered}");
        assert!(rendered.contains("c1"), "{rendered}");
        assert!(matches!(
            assistant,
            rig_core::completion::Message::Assistant(_)
        ));
    }

    #[test]
    fn a_request_carries_the_tools_the_cap_and_the_effort() {
        let request = ChatRequest {
            model: "m".to_owned(),
            system: None,
            messages: vec![message::system("be brief"), message::user("hi")],
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
                system: None,
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

#[cfg(test)]
#[allow(clippy::expect_used)]
mod effort_tests {
    use super::*;

    /// the map this endpoint actually produced: the default (high), none, low and max, with
    /// the aliases in between.
    #[test]
    fn a_probe_becomes_levels_ordered_from_least_to_most_effort() {
        let control = Profile {
            reasoning: 116,
            content: 117,
        };
        let probed = vec![
            (
                "none".to_owned(),
                Profile {
                    reasoning: 0,
                    content: 495,
                },
            ),
            (
                "low".to_owned(),
                Profile {
                    reasoning: 116,
                    content: 113,
                },
            ),
            ("high".to_owned(), control),
            (
                "max".to_owned(),
                Profile {
                    reasoning: 409,
                    content: 221,
                },
            ),
            ("minimal".to_owned(), control),
            ("medium".to_owned(), control),
            ("xhigh".to_owned(), control),
            ("ultra".to_owned(), control),
        ];

        let found = group_efforts(control, probed);
        assert_eq!(found.default.as_deref(), Some("high"));
        assert_eq!(
            found.levels,
            ["none", "low", "high", "max"],
            "least effort first: no reasoning, then low, then the default, then the hardest"
        );
    }

    #[test]
    fn a_server_that_ignores_everything_yields_only_its_default() {
        let control = Profile {
            reasoning: 100,
            content: 100,
        };
        let found = group_efforts(
            control,
            vec![("low".to_owned(), control), ("high".to_owned(), control)],
        );
        assert_eq!(found.default.as_deref(), Some("low"));
        assert_eq!(
            found.levels,
            ["low"],
            "no distinct behaviour, so no extra levels"
        );
    }

    #[test]
    fn a_failed_probe_yields_nothing() {
        let found = group_efforts(
            Profile {
                reasoning: 1,
                content: 1,
            },
            Vec::new(),
        );
        assert!(found.levels.is_empty(), "a failed probe invents nothing");
        assert!(found.default.is_none());
    }
}
