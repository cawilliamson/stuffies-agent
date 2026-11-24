//! the turn loop must speak the openai tool-calling protocol (spec 10.2).
#![allow(clippy::expect_used, clippy::panic)]

use std::sync::{Arc, Mutex};

use sa_core::agent::{
    Agent, AgentError, BoxFuture, EventSink, ToolExecutor, ToolOutcome, TurnRequest,
};
use sa_core::llm::{message, ChatRequest, Delta, Provider, ScriptedProvider, ToolCall, ToolSpec};
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
        workspace: "/tmp/ws".to_owned(),
        history: Vec::new(),
        compaction_summary: None,
        context: None,
        max_tokens: None,
        reasoning_effort: None,
        images: Vec::new(),
        continue_final: false,
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
                cached_tokens: 0,
            },
        ],
        vec![
            Delta::Text("it prints hello".to_owned()),
            Delta::Done {
                stop_reason: "stop".to_owned(),
                prompt_tokens: 5,
                completion_tokens: 5,
                cached_tokens: 0,
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
    let calls = tool_calls(messages);
    assert_eq!(calls.len(), 1, "the assistant turn carries the call");
    assert_eq!(calls[0].id.to_string(), "call_abc");
    assert_eq!(calls[0].function.name.to_string(), "read");
    assert_eq!(
        serde_json::Value::Object(calls[0].function.arguments.clone()),
        serde_json::json!({"path": "main.rs"})
    );

    let results = tool_results(messages);
    assert_eq!(results.len(), 1, "a tool result carrying the output");
    assert_eq!(
        results[0].call.to_string(),
        "call_abc",
        "the result must name the call it answers"
    );
    assert_eq!(results[0].name.to_string(), "read");
    assert!(
        format!("{:?}", results[0].content).contains("file contents"),
        "the output must reach the model"
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
                cached_tokens: 0,
            },
        ],
        vec![
            Delta::Text("gave up".to_owned()),
            Delta::Done {
                stop_reason: "stop".to_owned(),
                prompt_tokens: 1,
                completion_tokens: 1,
                cached_tokens: 0,
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
    let results = tool_results(&seen[1].messages);
    assert_eq!(results.len(), 1, "a result even when the tool fails");
    assert_eq!(results[0].call.to_string(), "call_x");
    assert!(
        format!("{:?}", results[0].content).contains("permission denied"),
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
fn an_image_rides_along_with_the_prompt() {
    use rig_core::completion::{message::UserContent, Message};

    let image = sa_proto::session::ImageAttachment {
        media_type: "image/png".to_owned(),
        data: "QUJD".to_owned(),
        name: Some("shot.png".to_owned()),
    };
    let Message::User { content } = message::user_with_images("look", &[image]) else {
        panic!("a user message");
    };
    assert!(
        matches!(&content[0], UserContent::Text(text) if text.text == "look"),
        "the prompt comes first"
    );
    assert!(
        format!("{:?}", content[1]).contains("data:image/png;base64,QUJD"),
        "the picture rides with it: {:?}",
        content[1]
    );
}

#[tokio::test]
async fn the_system_prompt_names_the_tools_the_rules_and_the_workspace() {
    let provider = Arc::new(ScriptedProvider::new(vec![vec![Delta::Done {
        stop_reason: "stop".to_owned(),
        prompt_tokens: 1,
        completion_tokens: 1,
        cached_tokens: 0,
    }]]));
    let sink = Arc::new(RecordingSink::default());
    let agent = Agent::new(
        Arc::clone(&provider) as Arc<dyn Provider>,
        Arc::new(FixedTool),
        Arc::clone(&sink) as Arc<dyn EventSink>,
    );

    let _ = agent.run_turn(request()).await;

    let seen = provider.seen_requests();
    let system = seen
        .first()
        .and_then(|request| request.system.as_ref())
        .map(rig_core::completion::prompt::SystemPrompt::text)
        .expect("a system prompt");

    assert!(
        system.contains("You are stuffies-agent,"),
        "the agent names itself, not the crate prefix:\n{system}"
    );

    assert!(
        system.contains("<tools>"),
        "the prompt lists tools:\n{system}"
    );
    assert!(system.contains("read"), "the tool registry feeds the list");
    assert!(
        system.contains("<rules>"),
        "the prompt carries rules:\n{system}"
    );
    assert!(
        system.contains("the git workspace at /tmp/ws"),
        "the model is told the session's workspace, not the daemon's cwd:\n{system}"
    );
    // the workspace is stated once, not repeated per tool or per section.
    assert_eq!(
        system.matches("/tmp/ws").count(),
        1,
        "the workspace path appears once:\n{system}"
    );
}

/// a tool result larger than the cap is cut on a char boundary, marked, and recorded in
/// the event, so one flooding command cannot consume the context (spec 9).
#[tokio::test]
async fn a_flooding_tool_result_is_capped_and_marked() {
    struct Flood;
    impl ToolExecutor for Flood {
        fn specs(&self) -> Vec<ToolSpec> {
            vec![ToolSpec {
                name: "bash".to_owned(),
                description: "run".to_owned(),
                parameters: json!({"type": "object"}),
            }]
        }
        fn execute(&self, _call: ToolCall) -> BoxFuture<'_, Result<ToolOutcome, String>> {
            Box::pin(async {
                Ok(ToolOutcome {
                    // multi-byte throughout, so a naive byte cut would panic.
                    output: "é".repeat(sa_core::tools::MAX_TOOL_OUTPUT),
                    exit_code: Some(0),
                })
            })
        }
    }

    let provider = Arc::new(ScriptedProvider::new(vec![
        vec![
            Delta::ToolCall(ToolCall {
                id: "c1".to_owned(),
                name: "bash".to_owned(),
                arguments: json!({}).to_string(),
            }),
            Delta::Done {
                stop_reason: "tool_calls".to_owned(),
                prompt_tokens: 1,
                completion_tokens: 1,
                cached_tokens: 0,
            },
        ],
        vec![Delta::Done {
            stop_reason: "stop".to_owned(),
            prompt_tokens: 1,
            completion_tokens: 1,
            cached_tokens: 0,
        }],
    ]));
    let sink = Arc::new(RecordingSink::default());
    let agent = Agent::new(
        Arc::clone(&provider) as Arc<dyn Provider>,
        Arc::new(Flood),
        Arc::clone(&sink) as Arc<dyn EventSink>,
    );

    let _ = agent.run_turn(request()).await;

    let completed = sink
        .events
        .lock()
        .expect("events")
        .iter()
        .find(|(kind, _)| kind == "tool.completed")
        .map(|(_, payload)| payload.clone())
        .expect("a tool result");
    assert_eq!(completed["truncated"], json!(true), "the cut is recorded");
    let output = completed["output"].as_str().expect("output");
    assert!(output.len() <= sa_core::tools::MAX_TOOL_OUTPUT + 16);
    assert!(output.ends_with("[truncated]"), "the cut is marked");
}

/// a sink holding messages the operator typed while the turn was running.
struct SteeringSink {
    pending: Mutex<Vec<String>>,
}

impl EventSink for SteeringSink {
    fn append(&self, _kind: &str, _payload: &Value) -> Result<u64, StoreError> {
        Ok(0)
    }

    fn is_cancelled(&self) -> bool {
        false
    }

    fn take_steering(&self) -> Option<String> {
        let mut pending = self.pending.lock().ok()?;
        if pending.is_empty() {
            None
        } else {
            Some(pending.remove(0))
        }
    }
}

#[tokio::test]
async fn a_steering_message_reaches_the_next_model_round_trip() {
    let provider = Arc::new(ScriptedProvider::new(vec![
        vec![
            Delta::ToolCall(ToolCall {
                id: "call_1".to_owned(),
                name: "read".to_owned(),
                arguments: json!({"path": "main.rs"}).to_string(),
            }),
            Delta::Done {
                stop_reason: "tool_calls".to_owned(),
                prompt_tokens: 5,
                completion_tokens: 5,
                cached_tokens: 0,
            },
        ],
        vec![Delta::Done {
            stop_reason: "stop".to_owned(),
            prompt_tokens: 1,
            completion_tokens: 1,
            cached_tokens: 0,
        }],
    ]));
    let sink = Arc::new(SteeringSink {
        pending: Mutex::new(vec!["stop and run the tests".to_owned()]),
    });
    let agent = Agent::new(
        Arc::clone(&provider) as Arc<dyn Provider>,
        Arc::new(FixedTool),
        Arc::clone(&sink) as Arc<dyn EventSink>,
    );

    let _ = agent.run_turn(request()).await;

    let seen = provider.seen_requests();
    assert!(seen.len() >= 2, "the tool call forces another round trip");
    let carried = user_texts(&seen[1].messages)
        .iter()
        .any(|text| text == "stop and run the tests");
    assert!(
        carried,
        "the steering message must reach the model: {:?}",
        user_texts(&seen[1].messages)
    );
}

#[tokio::test]
async fn steering_is_delivered_one_message_per_round_trip() {
    // a queue of redirects is not one confused prompt: the first lands, the second waits
    // for the next round-trip, which is pi's one-at-a-time default.
    let tool_call = |id: &str| {
        vec![
            Delta::ToolCall(ToolCall {
                id: id.to_owned(),
                name: "read".to_owned(),
                arguments: json!({"path": "main.rs"}).to_string(),
            }),
            Delta::Done {
                stop_reason: "tool_calls".to_owned(),
                prompt_tokens: 1,
                completion_tokens: 1,
                cached_tokens: 0,
            },
        ]
    };
    let provider = Arc::new(ScriptedProvider::new(vec![
        tool_call("call_1"),
        tool_call("call_2"),
        vec![Delta::Done {
            stop_reason: "stop".to_owned(),
            prompt_tokens: 1,
            completion_tokens: 1,
            cached_tokens: 0,
        }],
    ]));
    let sink = Arc::new(SteeringSink {
        pending: Mutex::new(vec!["first".to_owned(), "second".to_owned()]),
    });
    let agent = Agent::new(
        Arc::clone(&provider) as Arc<dyn Provider>,
        Arc::new(FixedTool),
        Arc::clone(&sink) as Arc<dyn EventSink>,
    );

    let _ = agent.run_turn(request()).await;

    let seen = provider.seen_requests();
    assert!(seen.len() >= 3, "two tool calls force three round trips");
    assert!(
        user_texts(&seen[0].messages)
            .iter()
            .any(|text| text == "first"),
        "the first message lands on the first round trip"
    );
    assert!(
        !user_texts(&seen[0].messages)
            .iter()
            .any(|text| text == "second"),
        "the second waits its turn: {:?}",
        user_texts(&seen[0].messages)
    );
    assert!(
        user_texts(&seen[1].messages)
            .iter()
            .any(|text| text == "second"),
        "and lands on the next one: {:?}",
        user_texts(&seen[1].messages)
    );
}

/// a tool that never finishes on its own, like a long command.
struct HangingTool;

impl ToolExecutor for HangingTool {
    fn specs(&self) -> Vec<ToolSpec> {
        vec![ToolSpec {
            name: "read".to_owned(),
            description: "read a file".to_owned(),
            parameters: json!({"type": "object"}),
        }]
    }

    fn execute(&self, _call: ToolCall) -> BoxFuture<'_, Result<ToolOutcome, String>> {
        Box::pin(async {
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
            Ok(ToolOutcome {
                output: "never".to_owned(),
                exit_code: None,
            })
        })
    }
}

#[tokio::test]
async fn escape_abandons_a_running_tool_instead_of_waiting_for_it() {
    let provider = ScriptedProvider::new(vec![
        vec![
            Delta::ToolCall(ToolCall {
                id: "c1".to_owned(),
                name: "read".to_owned(),
                arguments: "{}".to_owned(),
            }),
            Delta::Done {
                stop_reason: "tool_calls".to_owned(),
                prompt_tokens: 1,
                completion_tokens: 1,
                cached_tokens: 0,
            },
        ],
        vec![
            Delta::Text("done".to_owned()),
            Delta::Done {
                stop_reason: "stop".to_owned(),
                prompt_tokens: 1,
                completion_tokens: 1,
                cached_tokens: 0,
            },
        ],
    ]);
    let sink = Arc::new(CancellingSink {
        cancelled: std::sync::atomic::AtomicBool::new(false),
    });
    let agent = Agent::new(
        Arc::new(provider) as Arc<dyn Provider>,
        Arc::new(HangingTool),
        Arc::clone(&sink) as Arc<dyn EventSink>,
    );

    let flag = Arc::clone(&sink);
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        flag.cancelled
            .store(true, std::sync::atomic::Ordering::SeqCst);
    });

    let started = std::time::Instant::now();
    let outcome =
        tokio::time::timeout(std::time::Duration::from_secs(5), agent.run_turn(request())).await;
    let elapsed = started.elapsed();

    assert!(
        matches!(outcome, Ok(Err(AgentError::Cancelled))),
        "expected the turn to be cancelled, got {outcome:?}"
    );
    assert!(
        elapsed < std::time::Duration::from_secs(2),
        "a hanging tool must be abandoned, not awaited: took {elapsed:?}"
    );
}

#[tokio::test]
async fn a_turn_runs_past_any_fixed_number_of_rounds() {
    // 60 tool rounds then an answer: more rounds than any cap we used to impose.
    let rounds = 60;
    let mut script: Vec<Vec<Delta>> = Vec::new();
    for index in 0..rounds {
        script.push(vec![
            Delta::ToolCall(ToolCall {
                id: format!("c{index}"),
                name: "read".to_owned(),
                arguments: "{}".to_owned(),
            }),
            Delta::Done {
                stop_reason: "tool_calls".to_owned(),
                prompt_tokens: 1,
                completion_tokens: 1,
                cached_tokens: 0,
            },
        ]);
    }
    script.push(vec![
        Delta::Text("finally".to_owned()),
        Delta::Done {
            stop_reason: "stop".to_owned(),
            prompt_tokens: 1,
            completion_tokens: 1,
            cached_tokens: 0,
        },
    ]);

    let provider = ScriptedProvider::new(script);
    let sink = Arc::new(RecordingSink::default());
    let agent = Agent::new(
        Arc::new(provider) as Arc<dyn Provider>,
        Arc::new(FixedTool),
        Arc::clone(&sink) as Arc<dyn EventSink>,
    );

    let turn = agent
        .run_turn(request())
        .await
        .expect("the turn must finish");
    assert_eq!(turn.text, "finally");
}

/// the tool calls an assistant turn asked for.
fn tool_calls(
    messages: &[rig_core::completion::Message],
) -> Vec<rig_core::completion::message::ToolCall> {
    use rig_core::completion::{message::AssistantContent, Message};
    messages
        .iter()
        .flat_map(|message| match message {
            Message::Assistant(turn) => turn
                .content
                .iter()
                .filter_map(|content| match content {
                    AssistantContent::ToolCall(call) => Some(call.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>(),
            _ => Vec::new(),
        })
        .collect()
}

/// the tool results the transcript answers calls with.
fn tool_results(
    messages: &[rig_core::completion::Message],
) -> Vec<rig_core::completion::message::ToolResult> {
    use rig_core::completion::{message::UserContent, Message};
    messages
        .iter()
        .flat_map(|message| match message {
            Message::User { content } => content
                .iter()
                .filter_map(|content| match content {
                    UserContent::ToolResult(result) => Some(result.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>(),
            _ => Vec::new(),
        })
        .collect()
}

/// the plain text the operator sent.
fn user_texts(messages: &[rig_core::completion::Message]) -> Vec<String> {
    use rig_core::completion::{message::UserContent, Message};
    messages
        .iter()
        .flat_map(|message| match message {
            Message::User { content } => content
                .iter()
                .filter_map(|content| match content {
                    UserContent::Text(text) => Some(text.text.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>(),
            _ => Vec::new(),
        })
        .collect()
}

/// a stored event, as the store would hand it back.
fn stored(seq: u64, kind: &str, payload: Value) -> sa_proto::session::Event {
    sa_proto::session::Event {
        session_id: "s1".to_owned(),
        seq,
        kind: kind.to_owned(),
        at: 0,
        payload,
    }
}

#[tokio::test]
async fn a_follow_up_turn_sees_what_the_last_one_did() {
    // the regression: history was rebuilt from prompts and prose alone, so every turn began
    // with no record of the files it had read and re-derived the workspace from scratch.
    let provider = Arc::new(ScriptedProvider::new(vec![vec![
        Delta::Text("nothing to do".to_owned()),
        Delta::Done {
            stop_reason: "stop".to_owned(),
            prompt_tokens: 1,
            completion_tokens: 1,
            cached_tokens: 0,
        },
    ]]));
    let agent = Agent::new(
        Arc::clone(&provider) as Arc<dyn Provider>,
        Arc::new(FixedTool),
        Arc::new(RecordingSink::default()) as Arc<dyn EventSink>,
    );

    let mut follow_up = request();
    follow_up.prompt = "and now the tests".to_owned();
    follow_up.history = vec![
        stored(1, "turn.started", json!({"prompt": "read main.rs"})),
        stored(
            2,
            "llm.completed",
            json!({"text": "reading it", "stop_reason": "tool_calls", "purpose": "turn"}),
        ),
        stored(
            3,
            "tool.requested",
            json!({"call_id": "call_abc", "tool": "read", "args": {"path": "main.rs"}}),
        ),
        stored(
            4,
            "tool.completed",
            json!({"call_id": "call_abc", "output": "fn main() {}", "exit_code": 0}),
        ),
        stored(
            5,
            "llm.completed",
            json!({"text": "it is a hello world", "stop_reason": "stop", "purpose": "turn"}),
        ),
    ];

    agent.run_turn(follow_up).await.expect("turn");

    let seen = provider.seen_requests();
    let messages = &seen[0].messages;
    let texts = user_texts(messages);
    assert!(
        texts.iter().any(|text| text == "read main.rs"),
        "the earlier prompt must be carried: {texts:?}"
    );
    let calls = tool_calls(messages);
    assert_eq!(calls.len(), 1, "the earlier tool call must be carried");
    assert_eq!(calls[0].id.to_string(), "call_abc");
    let results = tool_results(messages);
    assert_eq!(results.len(), 1, "the earlier tool result must be carried");
    assert!(
        format!("{:?}", results[0].content).contains("fn main() {}"),
        "with its output: {:?}",
        results[0].content
    );
}

#[tokio::test]
async fn every_round_of_tool_calls_in_the_history_keeps_its_own_results() {
    // the regression: the pairing state was never reset between rounds, so the second
    // round's call was never announced and rig dropped its result as an orphan. the model
    // then re-read files it had already read.
    let provider = Arc::new(ScriptedProvider::new(vec![vec![
        Delta::Text("done".to_owned()),
        Delta::Done {
            stop_reason: "stop".to_owned(),
            prompt_tokens: 1,
            completion_tokens: 1,
            cached_tokens: 0,
        },
    ]]));
    let agent = Agent::new(
        Arc::clone(&provider) as Arc<dyn Provider>,
        Arc::new(FixedTool),
        Arc::new(RecordingSink::default()) as Arc<dyn EventSink>,
    );

    let mut follow_up = request();
    follow_up.prompt = "and now the tests".to_owned();
    follow_up.history = vec![
        stored(1, "turn.started", json!({"prompt": "read both files"})),
        stored(
            2,
            "llm.completed",
            json!({"text": "reading the first", "stop_reason": "tool_calls", "purpose": "turn"}),
        ),
        stored(
            3,
            "tool.requested",
            json!({"call_id": "call_one", "tool": "read", "args": {"path": "one.rs"}}),
        ),
        stored(
            4,
            "tool.completed",
            json!({"call_id": "call_one", "output": "fn one() {}", "exit_code": 0}),
        ),
        stored(
            5,
            "llm.completed",
            json!({"text": "reading the second", "stop_reason": "tool_calls", "purpose": "turn"}),
        ),
        stored(
            6,
            "tool.requested",
            json!({"call_id": "call_two", "tool": "read", "args": {"path": "two.rs"}}),
        ),
        stored(
            7,
            "tool.completed",
            json!({"call_id": "call_two", "output": "fn two() {}", "exit_code": 0}),
        ),
        stored(
            8,
            "llm.completed",
            json!({"text": "both read", "stop_reason": "stop", "purpose": "turn"}),
        ),
    ];

    agent.run_turn(follow_up).await.expect("turn");

    let seen = provider.seen_requests();
    let messages = &seen[0].messages;
    let calls = tool_calls(messages);
    let ids: Vec<String> = calls.iter().map(|call| call.id.to_string()).collect();
    assert_eq!(
        ids,
        ["call_one", "call_two"],
        "both rounds must announce their own calls"
    );
    let results = tool_results(messages);
    assert_eq!(
        results.len(),
        2,
        "both results must survive the pairing rules"
    );
    assert!(
        format!("{:?}", results[1].content).contains("fn two() {}"),
        "the second round's output must reach the model: {:?}",
        results[1].content
    );
}

#[tokio::test]
async fn a_round_that_reports_tool_calls_but_makes_none_ends_the_turn() {
    // the regression: a stop reason of tool_calls with no calls in it looped forever,
    // because the loop only ended when the reason was not tool_calls.
    let provider = Arc::new(ScriptedProvider::new(vec![vec![
        Delta::Text("thinking".to_owned()),
        Delta::Done {
            stop_reason: "tool_calls".to_owned(),
            prompt_tokens: 1,
            completion_tokens: 1,
            cached_tokens: 0,
        },
    ]]));
    let agent = Agent::new(
        Arc::clone(&provider) as Arc<dyn Provider>,
        Arc::new(FixedTool),
        Arc::new(RecordingSink::default()) as Arc<dyn EventSink>,
    );

    let outcome =
        tokio::time::timeout(std::time::Duration::from_secs(2), agent.run_turn(request()))
            .await
            .expect("a round with no calls must not repeat")
            .expect("turn");

    assert_eq!(outcome.text, "thinking");
    assert_eq!(
        provider.seen_requests().len(),
        1,
        "the turn must end rather than ask again"
    );
}

#[tokio::test]
async fn a_truncated_answer_is_recorded_as_the_reason_it_stopped() {
    let provider = Arc::new(ScriptedProvider::new(vec![vec![
        Delta::Text("half a sen".to_owned()),
        Delta::Done {
            stop_reason: "length".to_owned(),
            prompt_tokens: 1,
            completion_tokens: 1,
            cached_tokens: 0,
        },
    ]]));
    let sink = Arc::new(RecordingSink::default());
    let agent = Agent::new(
        Arc::clone(&provider) as Arc<dyn Provider>,
        Arc::new(FixedTool),
        Arc::clone(&sink) as Arc<dyn EventSink>,
    );
    let outcome = agent.run_turn(request()).await.expect("turn");
    assert_eq!(
        outcome.finish_reason, "length",
        "a turn cut by the output budget is not a clean stop"
    );
}

#[tokio::test]
async fn a_continued_turn_carries_on_the_answer_that_was_cut_off() {
    // a bare dot resumes: the conversation ends on the answer being continued, and no
    // prompt is put in front of the model.
    let provider = Arc::new(ScriptedProvider::new(vec![vec![
        Delta::Text("ku".to_owned()),
        Delta::Done {
            stop_reason: "stop".to_owned(),
            prompt_tokens: 1,
            completion_tokens: 1,
            cached_tokens: 0,
        },
    ]]));
    let agent = Agent::new(
        Arc::clone(&provider) as Arc<dyn Provider>,
        Arc::new(FixedTool),
        Arc::new(RecordingSink::default()) as Arc<dyn EventSink>,
    );

    let mut request = request();
    request.prompt = ".".to_owned();
    request.continue_final = true;
    request.history = vec![
        stored(1, "turn.started", json!({"prompt": "write a haiku"})),
        stored(
            2,
            "llm.completed",
            json!({"text": "an old pond", "stop_reason": "cancelled", "purpose": "turn"}),
        ),
    ];

    agent.run_turn(request).await.expect("turn");

    let seen = provider.seen_requests();
    assert_eq!(seen.len(), 1);
    assert!(
        seen[0].continue_final,
        "the request asks the endpoint to continue the last message"
    );
    let messages = &seen[0].messages;
    assert!(
        !user_texts(messages).iter().any(|text| text == "."),
        "a bare dot is not sent to the model: {:?}",
        user_texts(messages)
    );
    assert!(
        matches!(
            messages.last(),
            Some(rig_core::completion::Message::Assistant(_))
        ),
        "the conversation ends on the answer being continued"
    );
}

#[tokio::test]
async fn a_cancelled_turn_records_what_it_had_written() {
    // without this the next turn has nothing to carry on from: the partial answer would
    // exist only as deltas, which no transcript rebuild reads.
    use std::sync::atomic::{AtomicBool, Ordering};

    struct CancelAfterFirst {
        written: AtomicBool,
        events: Mutex<Vec<(String, Value)>>,
    }

    impl EventSink for CancelAfterFirst {
        fn append(&self, kind: &str, payload: &Value) -> Result<u64, StoreError> {
            if kind == "llm.delta" {
                self.written.store(true, Ordering::SeqCst);
            }
            self.events
                .lock()
                .map_err(|_| StoreError::Lock)?
                .push((kind.to_owned(), payload.clone()));
            Ok(0)
        }

        fn is_cancelled(&self) -> bool {
            self.written.load(Ordering::SeqCst)
        }
    }

    /// a stream that says one thing and then never closes, as a stalled endpoint does.
    struct HangingProvider;

    impl Provider for HangingProvider {
        fn stream(&self, _request: ChatRequest) -> sa_core::llm::StreamFuture<'_> {
            Box::pin(async {
                let (tx, rx) = tokio::sync::mpsc::channel(8);
                tokio::spawn(async move {
                    let _ = tx.send(Delta::Text("an old".to_owned())).await;
                    std::future::pending::<()>().await;
                });
                Ok(rx)
            })
        }
    }

    let sink = Arc::new(CancelAfterFirst {
        written: AtomicBool::new(false),
        events: Mutex::new(Vec::new()),
    });
    let agent = Agent::new(
        Arc::new(HangingProvider) as Arc<dyn Provider>,
        Arc::new(FixedTool),
        Arc::clone(&sink) as Arc<dyn EventSink>,
    );

    let cancelled = agent.run_turn(request()).await;
    assert!(
        matches!(cancelled, Err(AgentError::Cancelled)),
        "the turn was cancelled"
    );

    let events = sink.events.lock().expect("events");
    let completed = events
        .iter()
        .find(|(kind, payload)| kind == "llm.completed" && payload["stop_reason"] == "cancelled")
        .expect("the partial answer is recorded as a completed message");
    assert_eq!(
        completed.1["text"], "an old",
        "with the text it had written"
    );
}
