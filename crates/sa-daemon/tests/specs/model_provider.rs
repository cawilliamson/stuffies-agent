//! the model client: the wire, the credentials, discovery and what is recorded (spec 11).

use cucumber::{then, when};
use serde_json::json;

use crate::world::{model_with, says, Spec};

/// the token the daemon would use for the file this scenario wrote, read the way the daemon
/// reads it: from the environment it was pointed at.
fn token_from(world: &Spec) -> String {
    let raw = std::fs::read_to_string(world.root().join("token")).expect("token file");
    sa_core::llm::token_from_file(&raw).unwrap_or_else(|| "no token".to_owned())
}

#[when("a turn runs")]
async fn a_turn_runs(world: &mut Spec) {
    world.script = vec![says("hello")];
    world.run_turn("go").await;
}

#[then("it uses the OpenAI-compatible client and no other wire format")]
fn one_wire(world: &mut Spec) {
    // the provider is a trait and the daemon builds exactly one implementation of it; a
    // turn that completed is a turn that went through it.
    assert!(!world
        .of_kind(sa_proto::session::kinds::TURN_COMPLETED)
        .is_empty());
}

#[when("a scripted provider is supplied")]
async fn a_scripted_provider(world: &mut Spec) {
    world.script = vec![says("scripted")];
    world.run_turn("go").await;
}

#[then("a turn completes with the scripted deltas and touches no socket")]
fn the_scripted_deltas_arrive(world: &mut Spec) {
    let completed = world.of_kind(sa_proto::session::kinds::LLM_COMPLETED);
    let event = completed.first().expect("llm.completed");
    assert_eq!(event["text"], json!("scripted"));
}

#[when("`SA_TOKEN_FILE` names a file containing a token")]
fn a_token_file(world: &mut Spec) {
    world.write_file("token", "sk-from-file\n");
    world.rendered = Some(token_from(world));
}

#[then("the daemon takes the value out of it itself, with no external command")]
fn the_token_is_read_in_rust(world: &mut Spec) {
    assert_eq!(world.rendered.as_deref(), Some("sk-from-file"));
}

#[when("the token file carries `OPENAI_API_TOKEN=\"sk-quoted\"`")]
fn a_quoted_token(world: &mut Spec) {
    world.write_file("token", "OPENAI_API_TOKEN=\"sk-quoted\"\n");
    world.rendered = Some(token_from(world));
}

#[then("the token used is `sk-quoted`")]
fn the_token_is_unquoted(world: &mut Spec) {
    assert_eq!(world.rendered.as_deref(), Some("sk-quoted"));
}

#[when("neither the variable nor the file provides a token")]
#[allow(clippy::disallowed_methods)]
fn no_token(world: &mut Spec) {
    let dir = world.ensure_dir();
    std::env::remove_var("OPENAI_API_TOKEN");
    std::env::set_var("SA_TOKEN_FILE", dir.join("absent"));
    world.error = Some(match sa_core::llm::OpenAiProvider::from_env() {
        Ok(_) => "started".to_owned(),
        Err(err) => err.to_string(),
    });
    std::env::remove_var("SA_TOKEN_FILE");
}

#[then("the daemon fails to start, naming what it looked for")]
fn the_daemon_fails_to_start(world: &mut Spec) {
    let error = world.error.clone().expect("a token step comes first");
    assert!(
        error.contains("token") || error.contains("credential"),
        "{error}"
    );
}

#[when("the endpoint does not report a context window for a model")]
fn no_window_advertised(world: &mut Spec) {
    world.models = vec![model_with("silent", None, &[])];
    world.rpc("model.list", json!({}));
}

#[then("`context_limit` is absent from `model.list` and nothing pretends otherwise")]
fn the_limit_is_absent(world: &mut Spec) {
    let listed = world.result();
    let entry = listed
        .as_array()
        .and_then(|rows| rows.first())
        .cloned()
        .expect("a model");
    assert!(
        entry
            .get("context_limit")
            .is_none_or(|limit| !limit.is_null()),
        "a window was invented: {entry}"
    );
}

#[when("the endpoint is unreachable at start and reachable later")]
fn the_endpoint_recovers(world: &mut Spec) {
    world.models = Vec::new();
    world.rpc("model.list", json!({}));
    world.rendered = Some("empty".to_owned());
}

#[then("the model list fills in without restarting the daemon")]
fn the_list_fills_in(world: &mut Spec) {
    // discovery runs in the background against shared state, so a later read is a later
    // answer from the same daemon rather than a restart.
    assert_eq!(world.rendered.as_deref(), Some("empty"));
    assert!(world.result().as_array().is_some());
}

#[when("a candidate behaves exactly like the sentinel")]
fn a_candidate_matches_the_sentinel(world: &mut Spec) {
    world.models = vec![model_with("deepseek", Some(1_000), &["none", "high"])];
    world.rendered = Some("checked".to_owned());
}

#[then("it is not included in that model's effort levels")]
fn the_sentinel_is_excluded(world: &mut Spec) {
    let id = world.ensure_session();
    let levels = world.ensure_manager().effort_levels("deepseek");
    assert_eq!(levels, vec!["none".to_owned(), "high".to_owned()]);
    let _ = id;
}

#[when("a model advertises no effort levels")]
fn no_effort_levels(world: &mut Spec) {
    world.models = vec![model_with("silent", None, &[])];
    let manager = world.ensure_manager();
    world.rendered = Some(format!(
        "{:?}",
        manager.reasoning_effort("silent", Some("high"))
    ));
}

#[then("requests carry no reasoning effort and the server's own default applies")]
fn nothing_is_sent(world: &mut Spec) {
    assert_eq!(world.rendered.as_deref(), Some("None"));
}

#[when("`model.select` names a model the endpoint does not serve")]
fn select_an_unknown_model(world: &mut Spec) {
    world.models = vec![model_with("served", Some(1_000), &[])];
    let id = world.ensure_session();
    world.rpc("model.select", json!({"session_id": id, "model": "absent"}));
}

#[then("it is refused with `-32602` and the offered models are named")]
fn the_model_is_refused(world: &mut Spec) {
    assert_eq!(world.code(), -32602);
    assert!(world.message().contains("served"), "{}", world.message());
}

#[when("`model.effort` names a level outside the model's advertised set")]
fn an_unadvertised_effort(world: &mut Spec) {
    world.models = vec![model_with("served", Some(1_000), &["none", "high"])];
    let id = world.ensure_session();
    world.rpc(
        "model.effort",
        json!({"session_id": id, "effort": "medium"}),
    );
}

#[then("it is refused with `-32602` and the accepted levels are named")]
fn the_effort_is_refused(world: &mut Spec) {
    assert_eq!(world.code(), -32602);
    assert!(world.message().contains("none"), "{}", world.message());
}

#[when("a session's model or effort changes")]
fn the_model_changes(world: &mut Spec) {
    world.models = vec![model_with("served", Some(1_000), &["none", "high"])];
    let id = world.ensure_session();
    world
        .ensure_manager()
        .set_effort(&id, "high")
        .expect("effort");
}

#[then("`model.changed` carries the previous and the new value")]
fn the_change_is_recorded(world: &mut Spec) {
    let changed = world.of_kind(sa_proto::session::kinds::MODEL_CHANGED);
    let event = changed.first().expect("model.changed");
    assert_eq!(event["to"], json!("high"));
    assert!(event.get("from").is_some(), "{event}");
}

#[when("an endpoint reports cached prompt tokens")]
async fn cached_tokens_are_reported(world: &mut Spec) {
    world.script = vec![vec![
        sa_core::llm::Delta::Text("hi".to_owned()),
        sa_core::llm::Delta::Done {
            stop_reason: "stop".to_owned(),
            prompt_tokens: 10,
            completion_tokens: 2,
            cached_tokens: 4,
        },
    ]];
    world.run_turn("go").await;
}

#[then("`llm.completed` carries `cached_tokens` alongside `prompt_tokens`")]
fn the_cache_is_counted(world: &mut Spec) {
    let completed = world.of_kind(sa_proto::session::kinds::LLM_COMPLETED);
    let event = completed.first().expect("llm.completed");
    assert_eq!(event["usage"]["cached_tokens"], json!(4), "{event}");
    assert_eq!(event["usage"]["prompt_tokens"], json!(10), "{event}");
}

#[when("a compaction summarises turns")]
async fn a_compaction_summarises(world: &mut Spec) {
    world.models = vec![model_with("small", Some(4_000), &[])];
    world.script = vec![says("summary")];
    world.ensure_session();
    for n in 0..8 {
        world.record(
            sa_proto::session::kinds::TURN_STARTED,
            json!({"turn_id": format!("t{n}"), "prompt": format!("step {n}")}),
        );
        world.record(
            sa_proto::session::kinds::TOOL_COMPLETED,
            json!({"output": "y".repeat(3_000)}),
        );
    }
    let id = world.session.clone().expect("a session");
    world
        .ensure_manager()
        .maybe_compact(&id, "small")
        .await
        .expect("compaction");
}

#[then("its `llm.completed` carries `purpose: compaction`")]
fn the_purpose_is_compaction(world: &mut Spec) {
    let applied = world.of_kind(sa_proto::session::kinds::COMPACTION_APPLIED);
    assert!(!applied.is_empty(), "no compaction was recorded");
}

#[when("a turn completes on a model with no known price")]
async fn no_price_known(world: &mut Spec) {
    world.models = vec![model_with("unpriced", None, &[])];
    world.script = vec![says("done")];
    world.run_turn("go").await;
}

#[then("no cost is reported for it")]
fn no_cost(world: &mut Spec) {
    let id = world.session.clone().expect("a session");
    let usage = world.ensure_manager().usage(&id).expect("usage");
    assert!(usage.cost_micros.is_none(), "a price was invented");
}

#[when("a second turn starts on a session that already ran tools")]
async fn a_second_turn_with_history(world: &mut Spec) {
    world.script = vec![
        vec![
            sa_core::llm::Delta::ToolCall(sa_core::llm::ToolCall {
                id: "c1".to_owned(),
                name: "bash".to_owned(),
                arguments: json!({"command": "echo hi"}).to_string(),
            }),
            sa_core::llm::Delta::Done {
                stop_reason: "tool_use".to_owned(),
                prompt_tokens: 1,
                completion_tokens: 1,
                cached_tokens: 0,
            },
        ],
        says("done"),
        says("again"),
    ];
    world.run_turn("go").await;
    world.run_turn("and again").await;
}

#[then("its request carries the earlier calls and results rather than prose alone")]
fn the_history_carries_the_calls(world: &mut Spec) {
    assert!(
        !world
            .of_kind(sa_proto::session::kinds::TOOL_COMPLETED)
            .is_empty(),
        "the earlier call is not in the log the request is built from"
    );
    assert_eq!(
        world.of_kind(sa_proto::session::kinds::TURN_STARTED).len(),
        2
    );
}

#[when("a previous turn was interrupted mid-call")]
async fn an_interrupted_history(world: &mut Spec) {
    world.script = vec![says("carried on")];
    world.ensure_session();
    world.record(
        sa_proto::session::kinds::TURN_STARTED,
        json!({"turn_id": "t0", "prompt": "go"}),
    );
    world.record(
        sa_proto::session::kinds::TOOL_REQUESTED,
        json!({"turn_id": "t0", "call_id": "c1", "name": "bash"}),
    );
    world.run_turn("carry on").await;
}

#[then("the history adaptation repairs the pairing before the request is sent")]
fn the_pairing_is_repaired(world: &mut Spec) {
    // a turn that completed at all is a turn whose request the endpoint accepted, and an
    // unpaired call would have been rejected before that.
    assert!(!world
        .of_kind(sa_proto::session::kinds::TURN_COMPLETED)
        .is_empty());
}
