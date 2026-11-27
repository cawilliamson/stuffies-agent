//! the context window and the effort levels must come from the endpoint, never from a
//! guess baked into this build.
#![allow(clippy::expect_used, clippy::panic)]

use std::sync::{Arc, Mutex};

use sa_core::llm::{Delta, ModelInfo, Provider, ScriptedProvider};
use sa_core::manager::{Manager, ManagerConfig};
use sa_core::store::Store;
use serde_json::json;

fn manager_with(dir: &std::path::Path, models: Vec<ModelInfo>) -> Arc<Manager> {
    let store = Store::open(&dir.join("sa.db")).expect("store");
    let provider: Arc<dyn Provider> = Arc::new(ScriptedProvider::new(Vec::<Vec<Delta>>::new()));
    Arc::new(Manager::new(
        store,
        Arc::new(ManagerConfig {
            host_fingerprint: "test-host".to_owned(),
            provider,
            max_tokens: None,
            lock_dir: dir.join("locks"),
            checkpoint_dir: dir.join("checkpoints"),
            turn_timeout_secs: 30,
            aux_budget_secs: 5,
            models: Arc::new(Mutex::new(models)),
            mcp: Arc::new(sa_core::mcp::Hub::empty()),
            lsp: Arc::new(sa_core::lsp::Hub::new()),
        }),
    ))
}

fn manager(dir: &std::path::Path, models: Vec<ModelInfo>) -> Arc<Manager> {
    manager_with(dir, models)
}

/// the same model, with the level its endpoint defaults to.
fn model_defaulting_to(id: &str, effort: &str) -> ModelInfo {
    ModelInfo {
        default_effort: Some(effort.to_owned()),
        ..model(id, None, &[])
    }
}

fn model(id: &str, context_limit: Option<u64>, efforts: &[&str]) -> ModelInfo {
    ModelInfo {
        id: id.to_owned(),
        context_limit,
        effort_levels: efforts.iter().map(|effort| (*effort).to_owned()).collect(),
        default_effort: None,
    }
}

#[test]
fn the_window_comes_from_the_model_and_is_unknown_when_it_is_not_advertised() {
    let dir = tempfile::tempdir().expect("tempdir");
    let manager = manager(dir.path(), vec![model("big", Some(1_000_000), &[])]);

    assert_eq!(
        manager.context_limit("big"),
        Some(1_000_000),
        "a 1m model must report 1m, not a default"
    );
    assert_eq!(
        manager.context_limit("silent"),
        None,
        "an endpoint that says nothing means unknown, not 128k"
    );
}

#[test]
fn the_used_figure_is_the_prompt_the_provider_counted() {
    let dir = tempfile::tempdir().expect("tempdir");
    let manager = manager(dir.path(), vec![model("big", Some(1_000_000), &[])]);
    let session = manager
        .create_session(dir.path().to_str().expect("utf8"), None, "test")
        .expect("create")
        .session_id;

    assert_eq!(manager.context_used(&session), None, "nothing measured yet");

    manager
        .append(
            &session,
            sa_proto::session::kinds::LLM_COMPLETED,
            &json!({"usage": {"prompt_tokens": 1234, "completion_tokens": 5}}),
        )
        .expect("append");

    assert_eq!(
        manager.context_used(&session),
        Some(1234),
        "the figure must be the provider's own count, not an estimate"
    );
}

#[test]
fn the_effort_levels_are_the_ones_the_endpoint_lists() {
    let dir = tempfile::tempdir().expect("tempdir");
    let manager = manager(
        dir.path(),
        vec![model("deepseek", Some(1_000_000), &["none", "low", "high"])],
    );

    assert_eq!(
        manager.effort_levels("deepseek"),
        vec!["none", "low", "high"],
        "the endpoint's own values, in its own order"
    );
    assert!(
        manager.effort_levels("unknown-model").is_empty(),
        "a model that advertises nothing gets no invented list"
    );
}

#[test]
fn only_an_advertised_effort_is_ever_sent() {
    let dir = tempfile::tempdir().expect("tempdir");
    let manager = manager(
        dir.path(),
        vec![model("deepseek", Some(1_000_000), &["none", "high"])],
    );

    assert_eq!(
        manager
            .reasoning_effort("deepseek", Some("high"))
            .as_deref(),
        Some("high"),
        "a choice the model accepts is kept"
    );
    assert_eq!(
        manager
            .reasoning_effort("deepseek", Some("medium"))
            .as_deref(),
        Some("none"),
        "a level the model never listed falls back to its first"
    );
    assert_eq!(
        manager.reasoning_effort("silent", Some("high")).as_deref(),
        Some("high"),
        "an explicit choice is honoured even when nothing was discovered"
    );
    assert_eq!(
        manager.reasoning_effort("silent", None),
        None,
        "with no choice, the server's own default applies"
    );
}

#[test]
fn a_session_reports_the_effort_a_turn_will_use() {
    let dir = tempfile::tempdir().expect("tempdir");
    let manager = manager(dir.path(), vec![model_defaulting_to("m", "medium")]);

    let created = manager
        .create_session(dir.path().to_str().expect("utf8"), None, "test")
        .expect("create");
    let row = manager
        .store()
        .get_session(&created.session_id)
        .expect("get")
        .expect("session");
    assert_eq!(
        row.effort.as_deref(),
        Some("medium"),
        "a new session records the effort its turns use"
    );

    // a session made before that was stored reports the same, so every tab shows it.
    manager
        .store()
        .set_session_effort(&created.session_id, "")
        .expect("clear");
    let listed = manager.list_sessions(false).expect("list");
    assert_eq!(listed[0].effort.as_deref(), Some("medium"));
}

#[test]
fn auxiliary_calls_take_the_least_effort_the_model_offers() {
    let dir = tempfile::tempdir().expect("tempdir");
    let manager = manager(
        dir.path(),
        vec![model("m", None, &["none", "low", "high", "max"])],
    );

    assert_eq!(
        manager.cheapest_effort("m").as_deref(),
        Some("none"),
        "a title needs no thinking"
    );
    assert_eq!(
        manager.cheapest_effort("unknown-model"),
        None,
        "with nothing known, the server's own default applies"
    );
}

#[test]
fn a_new_session_records_which_context_files_its_directory_provides() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("AGENTS.md"), "be brief").expect("write");
    std::fs::write(dir.path().join("PERSONALITY.md"), "be dry").expect("write");
    let manager = manager(dir.path(), vec![model("big", Some(1_000_000), &[])]);
    let session = manager
        .create_session(dir.path().to_str().expect("utf8"), None, "test")
        .expect("create")
        .session_id;

    let (events, _) = manager
        .store()
        .events_since(&session, 0, 1000)
        .expect("events");
    let loaded = events
        .iter()
        .find(|event| event.kind == sa_proto::session::kinds::CONTEXT_LOADED)
        .expect("the session must say what it loaded");
    let groups = loaded.payload["groups"].as_array().expect("groups");
    let files: Vec<&str> = groups
        .iter()
        .flat_map(|group| group["files"].as_array().expect("files"))
        .filter_map(|file| file["path"].as_str())
        .collect();
    assert!(
        files.iter().any(|file| file.ends_with("PERSONALITY.md")),
        "soul must be listed: {files:?}"
    );
    assert!(
        files.iter().any(|file| file.ends_with("AGENTS.md")),
        "agents must be listed: {files:?}"
    );
    assert_eq!(
        loaded.payload["cwd"].as_str(),
        Some(dir.path().to_str().expect("utf8")),
        "the list belongs to the directory it came from"
    );
}

#[test]
fn changing_directory_records_the_context_for_the_new_one() {
    let first = tempfile::tempdir().expect("tempdir");
    let second = tempfile::tempdir().expect("tempdir");
    std::fs::write(first.path().join("AGENTS.md"), "first").expect("write");
    std::fs::write(second.path().join("RULES.md"), "second").expect("write");
    let manager = manager(first.path(), vec![model("big", Some(1_000_000), &[])]);
    let session = manager
        .create_session(first.path().to_str().expect("utf8"), None, "test")
        .expect("create")
        .session_id;

    manager
        .set_cwd(&session, second.path().to_str().expect("utf8"))
        .expect("cwd");

    let (events, _) = manager
        .store()
        .events_since(&session, 0, 1000)
        .expect("events");
    let last = events
        .iter()
        .rfind(|event| event.kind == sa_proto::session::kinds::CONTEXT_LOADED)
        .expect("a second listing");
    let files: Vec<&str> = last.payload["groups"]
        .as_array()
        .expect("groups")
        .iter()
        .flat_map(|group| group["files"].as_array().expect("files"))
        .filter_map(|file| file["path"].as_str())
        .collect();
    assert!(
        files.iter().any(|file| file.ends_with("RULES.md")),
        "the new directory's files: {files:?}"
    );
    assert!(
        !files.iter().any(|file| file.ends_with("AGENTS.md")),
        "the old directory's files must not carry over: {files:?}"
    );
}

/// reasoning is streamed to the operator and never replayed, and it is most of the log.
/// counting it made a session read as many times its real size, and every turn compacted.
#[test]
fn reasoning_is_not_part_of_the_context_estimate() {
    let dir = tempfile::tempdir().expect("tempdir");
    let manager = manager(dir.path(), vec![model("big", Some(1_000_000), &[])]);
    let session = manager
        .create_session(dir.path().to_str().expect("utf8"), None, "test")
        .expect("create")
        .session_id;

    manager
        .append(
            &session,
            sa_proto::session::kinds::TURN_STARTED,
            &json!({"prompt": "look at the repo"}),
        )
        .expect("append");
    for _ in 0..200 {
        manager
            .append(
                &session,
                sa_proto::session::kinds::LLM_REASONING,
                &json!({"text": "x".repeat(4000)}),
            )
            .expect("append");
    }

    let estimate = manager.estimate_tokens(&session);
    assert!(
        estimate < 100,
        "800k characters of reasoning must not read as 200k tokens: {estimate}"
    );
}

/// a window taken from seq 0 is the oldest events, which on a long session is the beginning
/// of a conversation that has moved on.
#[test]
fn the_history_window_is_the_newest_events() {
    let dir = tempfile::tempdir().expect("tempdir");
    let manager = manager(dir.path(), vec![model("big", Some(1_000_000), &[])]);
    let session = manager
        .create_session(dir.path().to_str().expect("utf8"), None, "test")
        .expect("create")
        .session_id;
    for n in 0..40 {
        manager
            .append(
                &session,
                sa_proto::session::kinds::TURN_STARTED,
                &json!({"prompt": format!("step {n}")}),
            )
            .expect("append");
    }

    let tail = manager.store().events_tail(&session, 5).expect("tail");
    assert_eq!(tail.len(), 5, "five events were asked for");
    assert_eq!(
        tail.last().expect("an event").payload["prompt"],
        "step 39",
        "the window ends on the newest event"
    );
    assert_eq!(
        tail.first().expect("an event").payload["prompt"],
        "step 35",
        "and starts five events back, not at the beginning"
    );
}

/// a manager whose provider replays a script, for the calls that summarise.
fn manager_scripted(
    dir: &std::path::Path,
    models: Vec<ModelInfo>,
    script: Vec<Vec<Delta>>,
) -> (Arc<Manager>, Arc<ScriptedProvider>) {
    let store = Store::open(&dir.join("sa.db")).expect("store");
    let scripted = Arc::new(ScriptedProvider::new(script));
    let provider: Arc<dyn Provider> = scripted.clone();
    let manager = Arc::new(Manager::new(
        store,
        Arc::new(ManagerConfig {
            host_fingerprint: "test-host".to_owned(),
            provider,
            max_tokens: None,
            lock_dir: dir.join("locks"),
            checkpoint_dir: dir.join("checkpoints"),
            turn_timeout_secs: 30,
            aux_budget_secs: 5,
            models: Arc::new(Mutex::new(models)),
            mcp: Arc::new(sa_core::mcp::Hub::empty()),
            lsp: Arc::new(sa_core::lsp::Hub::new()),
        }),
    ));
    (manager, scripted)
}

fn summary_stream() -> Vec<Delta> {
    vec![
        Delta::Text("earlier: the operator asked for a change and it was made.".to_owned()),
        Delta::Done {
            stop_reason: "stop".to_owned(),
            prompt_tokens: 40,
            completion_tokens: 12,
            cached_tokens: 0,
        },
    ]
}

/// a compaction folds the old turns into a summary and then stops: the next turn finds
/// nothing new to fold, so it does not summarise the same range again and again.
#[tokio::test]
async fn a_compaction_folds_the_old_turns_once() {
    let dir = tempfile::tempdir().expect("tempdir");
    // a small window, so a handful of turns trips the trigger.
    let (manager, scripted) = manager_scripted(
        dir.path(),
        vec![model("small", Some(4_000), &[])],
        vec![summary_stream(), summary_stream()],
    );
    let session = manager
        .create_session(dir.path().to_str().expect("utf8"), None, "test")
        .expect("create")
        .session_id;
    for n in 0..8 {
        manager
            .append(
                &session,
                sa_proto::session::kinds::TURN_STARTED,
                &json!({"prompt": format!("step {n}")}),
            )
            .expect("append");
        manager
            .append(
                &session,
                sa_proto::session::kinds::TOOL_COMPLETED,
                &json!({"output": "y".repeat(3_000)}),
            )
            .expect("append");
    }

    let summary = manager
        .maybe_compact(&session, "small")
        .await
        .expect("compaction");
    assert!(summary.is_some(), "the window was over the trigger");

    let applied = manager
        .store()
        .last_event_of_kind(&session, &[sa_proto::session::kinds::COMPACTION_APPLIED])
        .expect("read")
        .expect("a compaction event");
    let to_seq = applied.payload["to_seq"].as_u64().expect("to_seq");
    assert!(to_seq > 0, "the folded range is not empty");
    assert_eq!(
        applied.payload["from_seq"],
        json!(1),
        "it starts at the top"
    );
    assert!(
        applied.payload["summary"]
            .as_str()
            .is_some_and(|s| !s.is_empty()),
        "the summary is in the event"
    );

    // nothing new to fold: the second check must not call the model again.
    let again = manager
        .maybe_compact(&session, "small")
        .await
        .expect("compaction");
    assert!(
        again.is_none(),
        "the same range must not be summarised twice"
    );
    assert_eq!(
        scripted.seen_requests().len(),
        1,
        "one summarisation, not one per turn"
    );
}
