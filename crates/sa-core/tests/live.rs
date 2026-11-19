//! a live check against the configured endpoint. ignored by default because it needs
//! credentials: `cargo test -p sa-core --test live -- --ignored --test-threads=1 --nocapture`.
//! one at a time: a shared endpoint throttles parallel callers, which reads as a flake.
#![allow(clippy::expect_used, clippy::panic, clippy::print_stderr)]

use sa_core::llm::{message, ChatRequest, Delta, OpenAiProvider, Provider, ToolCall, ToolSpec};
use serde_json::json;

#[tokio::test]
#[ignore = "needs OPENAI_API_TOKEN; run with --ignored"]
async fn the_provider_streams_an_answer() {
    let provider = OpenAiProvider::from_env().expect("credentials");
    let mut rx = provider
        .stream(ChatRequest {
            model: std::env::var("SA_MODEL").unwrap_or_default(),
            messages: vec![message::user("Reply with exactly the word: hello")],
            tools: Vec::new(),
            max_tokens: Some(64),
            reasoning_effort: None,
        })
        .await
        .expect("stream");

    let mut text = String::new();
    let mut done = false;
    while let Some(delta) = rx.recv().await {
        match delta {
            Delta::Text(chunk) => text.push_str(&chunk),
            Delta::Reasoning(chunk) => eprintln!("[reasoning] {chunk:?}"),
            Delta::ToolCall(call) => eprintln!("[tool] {call:?}"),
            Delta::Failed(reason) => panic!("provider failed: {reason}"),
            Delta::Done {
                stop_reason,
                prompt_tokens,
                completion_tokens,
            } => {
                eprintln!("[done] {stop_reason} {prompt_tokens} in / {completion_tokens} out");
                done = true;
            }
        }
    }
    eprintln!("TEXT: {text:?}");
    assert!(done, "the stream must end with a done delta");
    assert!(!text.trim().is_empty(), "the model must answer");
}

#[tokio::test]
#[ignore = "needs OPENAI_API_TOKEN; run with --ignored"]
async fn a_tool_call_and_its_result_round_trip() {
    let provider = OpenAiProvider::from_env().expect("credentials");
    let model = std::env::var("SA_MODEL").unwrap_or_default();
    let tools = vec![ToolSpec {
        name: "read".to_owned(),
        description: "Read a file as text.".to_owned(),
        parameters: json!({"type": "object", "properties": {"path": {"type": "string"}}, "required": ["path"]}),
    }];
    let ask = message::user("Read the file Cargo.toml with the read tool, then say what it is.");

    // a model may answer directly instead of calling the tool; that is a property of the
    // model, not of this code, so the phase is retried rather than failed.
    let mut calls = Vec::new();
    for _ in 0..5 {
        let mut rx = provider
            .stream(ChatRequest {
                model: model.clone(),
                messages: vec![ask.clone()],
                tools: tools.clone(),
                max_tokens: Some(256),
                reasoning_effort: None,
            })
            .await
            .expect("stream");
        while let Some(delta) = rx.recv().await {
            match delta {
                Delta::ToolCall(call) => calls.push(call),
                Delta::Failed(reason) => panic!("provider failed: {reason}"),
                _ => {}
            }
        }
        if !calls.is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    assert!(!calls.is_empty(), "the model must call the tool");
    eprintln!("[call] {:?}", calls[0]);

    // the assistant turn must carry the call verbatim, and the result must name the tool.
    let assistant =
        message::assistant_with_calls("", std::slice::from_ref(&calls[0])).expect("assistant");
    let result = message::tool_result(
        &ToolCall {
            id: calls[0].id.clone(),
            name: calls[0].name.clone(),
            arguments: calls[0].arguments.clone(),
        },
        "[package]\nname = \"sa\"\nversion = \"0.1.0\"",
    )
    .expect("result");

    let mut rx = provider
        .stream(ChatRequest {
            model,
            messages: vec![ask, assistant, result],
            tools,
            max_tokens: Some(256),
            reasoning_effort: None,
        })
        .await
        .expect("stream");
    let mut text = String::new();
    while let Some(delta) = rx.recv().await {
        match delta {
            Delta::Text(chunk) => text.push_str(&chunk),
            Delta::Failed(reason) => panic!("provider failed after the tool result: {reason}"),
            _ => {}
        }
    }
    eprintln!("ANSWER: {text:?}");
    assert!(
        !text.trim().is_empty(),
        "the model must answer after the tool result"
    );
}

#[tokio::test]
#[ignore = "needs OPENAI_API_TOKEN; run with --ignored"]
async fn the_catalog_is_read_from_the_endpoint() {
    let provider = OpenAiProvider::from_env().expect("credentials");
    let models = provider.list_models().await.expect("catalog");
    assert!(!models.is_empty(), "the endpoint must list a model");
    eprintln!("catalog: {models:?}");

    // the window is whatever the endpoint says, under whatever it calls the field.
    assert!(
        models[0].context_limit.is_some(),
        "the context window must be discovered, not assumed"
    );
}
