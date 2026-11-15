//! the turn loop must speak the openai tool-calling protocol (spec 10.2).
#![allow(clippy::expect_used, clippy::panic)]

use std::sync::{Arc, Mutex};

use sa_core::agent::{
    Agent, AgentError, BoxFuture, EventSink, ToolExecutor, ToolOutcome, TurnRequest,
};
use sa_core::llm::{
    ChatMessage, ChatRequest, Delta, Provider, ScriptedProvider, ToolCall, ToolSpec,
};
use sa_core::store::StoreError;
use serde_json::{json, Value};

#[derive(Default)]
struct RecordingSink {
    events: Mutex<Vec<(String, Value)>>,
}

impl EventSink for RecordingSink {
    fn append(&self, kind: &str, payload: &Value) -> Result<u64, StoreError> {
        self.events
            .lock()
            .map_err(|_| StoreError::Lock)?
            .push((kind.to_owned(), payload.clone()));
        Ok(0)
    }

    fn is_cancelled(&self) -> bool {
        false
    }
}

struct FixedTool;

impl ToolExecutor for FixedTool {
    fn specs(&self) -> Vec<ToolSpec> {
        vec![ToolSpec {
            name: "read".to_owned(),
            description: "read a file".to_owned(),
            parameters: json!({"type": "object"}),
        }]
    }

    fn execute(&self, _call: ToolCall) -> BoxFuture<'_, Result<ToolOutcome, String>> {
        Box::pin(async {
            Ok(ToolOutcome {
                output: "file contents".to_owned(),
                exit_code: None,
            })
        })
    }
}

fn request() -> TurnRequest {
    TurnRequest {
        turn_id: "t1".to_owned(),
        prompt: "read main.rs".to_owned(),
        model: "m".to_owned(),
        history: Vec::new(),
        compaction_summary: None,
        context: None,
        max_tokens: None,
        reasoning_effort: None,
        images: Vec::new(),
    }
}

#[tokio::test]
async fn tool_results_are_addressed_to_the_call_that_requested_them() {
    let provider = Arc::new(ScriptedProvider::new(vec![
        vec![
            Delta::ToolCall(ToolCall {
                id: "call_abc".to_owned(),
                name: "read".to_owned(),
                arguments: json!({"path": "main.rs"}).to_string(),
            }),
            Delta::Done {
                stop_reason: "tool_calls".to_owned(),
                prompt_tokens: 5,
                completion_tokens: 5,
            },
        ],
        vec![
            Delta::Text("it prints hello".to_owned()),
            Delta::Done {
                stop_reason: "stop".to_owned(),
                prompt_tokens: 5,
                completion_tokens: 5,
            },
        ],
    ]));

    let sink = Arc::new(RecordingSink::default());
    let agent = Agent::new(
        Arc::clone(&provider) as Arc<dyn Provider>,
        Arc::new(FixedTool),
        Arc::clone(&sink) as Arc<dyn EventSink>,
    );
    agent.run_turn(request()).await.expect("turn");

    let seen: Vec<ChatRequest> = provider.seen_requests();
    assert_eq!(seen.len(), 2, "the agent must continue after a tool call");

    let messages = &seen[1].messages;
    let assistant = messages
        .iter()
        .find(|m| m.role == "assistant" && m.tool_calls.is_some())
        .expect("an assistant message carrying the tool calls");

    let calls = assistant.tool_calls.as_ref().expect("tool calls");
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].id, "call_abc");
    assert_eq!(calls[0].kind, "function");
    assert_eq!(calls[0].function.name, "read");
    assert_eq!(calls[0].function.arguments, r#"{"path":"main.rs"}"#);

    let result = messages
        .iter()
        .find(|m| m.role == "tool")
        .expect("a tool message carrying the result");
    assert_eq!(
        result.tool_call_id.as_deref(),
        Some("call_abc"),
        "the result must name the call it answers"
    );
    assert_eq!(
        result.content.as_ref().and_then(|c| c.as_text()),
        Some("file contents")
    );

    // the old shape smuggled results in as user turns, which made the model re-issue calls.
    assert!(
        !messages.iter().any(|m| m.role == "user"
            && m.content
                .as_ref()
                .and_then(|c| c.as_text())
                .is_some_and(|c| c.starts_with("tool "))),
        "tool results must not masquerade as user messages"
    );

    let kinds: Vec<String> = sink
        .events
        .lock()
        .expect("events")
        .iter()
        .map(|(kind, _)| kind.clone())
        .collect();
    assert!(kinds.contains(&"tool.completed".to_owned()), "{kinds:?}");
}

#[tokio::test]
async fn a_failed_tool_still_answers_the_call() {
    struct FailingTool;
    impl ToolExecutor for FailingTool {
        fn specs(&self) -> Vec<ToolSpec> {
            Vec::new()
        }
        fn execute(&self, _call: ToolCall) -> BoxFuture<'_, Result<ToolOutcome, String>> {
            Box::pin(async { Err("permission denied".to_owned()) })
        }
    }

    let provider = Arc::new(ScriptedProvider::new(vec![
        vec![
            Delta::ToolCall(ToolCall {
                id: "call_x".to_owned(),
                name: "bash".to_owned(),
                arguments: "{}".to_owned(),
            }),
            Delta::Done {
                stop_reason: "tool_calls".to_owned(),
                prompt_tokens: 1,
                completion_tokens: 1,
            },
        ],
        vec![
            Delta::Text("gave up".to_owned()),
            Delta::Done {
                stop_reason: "stop".to_owned(),
                prompt_tokens: 1,
                completion_tokens: 1,
            },
        ],
    ]));

    let agent = Agent::new(
        Arc::clone(&provider) as Arc<dyn Provider>,
        Arc::new(FailingTool),
        Arc::new(RecordingSink::default()) as Arc<dyn EventSink>,
    );
    agent.run_turn(request()).await.expect("turn");

    let seen = provider.seen_requests();
    let result = seen[1]
        .messages
        .iter()
        .find(|m| m.role == "tool")
        .expect("a tool message even on failure");
    assert_eq!(result.tool_call_id.as_deref(), Some("call_x"));
    assert!(
        result
            .content
            .as_ref()
            .and_then(|c| c.as_text())
            .is_some_and(|c| c.contains("permission denied")),
        "the failure must reach the model"
    );
}

/// a provider that accepts the request and then never sends anything.
struct StalledProvider;

impl Provider for StalledProvider {
    fn stream(&self, _request: ChatRequest) -> sa_core::llm::StreamFuture<'_> {
        Box::pin(async {
            let (tx, rx) = tokio::sync::mpsc::channel(4);
            // leaked so the channel stays open: the stream simply stalls.
            let _leaked = Box::leak(Box::new(tx));
            Ok(rx)
        })
    }
}

struct CancellingSink {
    cancelled: std::sync::atomic::AtomicBool,
}

impl EventSink for CancellingSink {
    fn append(&self, _kind: &str, _payload: &Value) -> Result<u64, StoreError> {
        Ok(0)
    }

    fn is_cancelled(&self) -> bool {
        self.cancelled.load(std::sync::atomic::Ordering::SeqCst)
    }
}

#[tokio::test]
async fn escape_interrupts_a_stalled_turn_without_waiting_for_a_delta() {
    let sink = Arc::new(CancellingSink {
        cancelled: std::sync::atomic::AtomicBool::new(false),
    });
    let agent = Agent::new(
        Arc::new(StalledProvider),
        Arc::new(FixedTool),
        Arc::clone(&sink) as Arc<dyn EventSink>,
    );

    let flag = Arc::clone(&sink);
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        flag.cancelled
            .store(true, std::sync::atomic::Ordering::SeqCst);
    });

    let started = std::time::Instant::now();
    let outcome =
        tokio::time::timeout(std::time::Duration::from_secs(5), agent.run_turn(request())).await;
    let elapsed = started.elapsed();

    assert!(
        matches!(outcome, Ok(Err(AgentError::Cancelled))),
        "expected a cancellation, got {outcome:?}"
    );
    assert!(
        elapsed < std::time::Duration::from_secs(2),
        "took {elapsed:?}"
    );
}

#[test]
fn images_serialise_as_content_parts_and_plain_text_stays_a_string() {
    let plain = serde_json::to_value(ChatMessage::user("hello")).expect("json");
    assert_eq!(plain["content"], "hello", "text-only stays a bare string");

    let image = sa_proto::session::ImageAttachment {
        media_type: "image/png".to_owned(),
        data: "QUJD".to_owned(),
    };
    let with_image =
        serde_json::to_value(ChatMessage::user_with_images("look", &[image])).expect("json");
    assert_eq!(with_image["content"][0]["type"], "text");
    assert_eq!(with_image["content"][0]["text"], "look");
    assert_eq!(with_image["content"][1]["type"], "image_url");
    assert_eq!(
        with_image["content"][1]["image_url"]["url"],
        "data:image/png;base64,QUJD"
    );
}
