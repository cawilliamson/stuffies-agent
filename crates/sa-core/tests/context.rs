//! the context window and the effort levels must come from the endpoint, never from a
//! guess baked into this build.
#![allow(clippy::expect_used, clippy::panic)]

use std::sync::{Arc, Mutex};

use sa_core::llm::{Delta, ModelInfo, Provider, ScriptedProvider};
use sa_core::manager::{Manager, ManagerConfig};
use sa_core::store::Store;
use serde_json::json;

fn manager(dir: &std::path::Path, models: Vec<ModelInfo>) -> Arc<Manager> {
    let store = Store::open(&dir.join("sa.db")).expect("store");
    let provider: Arc<dyn Provider> = Arc::new(ScriptedProvider::new(Vec::<Vec<Delta>>::new()));
    Arc::new(Manager::new(
        store,
        Arc::new(ManagerConfig {
            host_fingerprint: "test-host".to_owned(),
            provider,
            max_tokens: None,
            lock_dir: dir.join("locks"),
            turn_timeout_secs: 30,
            aux_budget_secs: 5,
            models: Arc::new(Mutex::new(models)),
            mcp: Arc::new(sa_core::mcp::Hub::empty()),
        }),
    ))
}

fn model(id: &str, context_limit: Option<u64>, efforts: &[&str]) -> ModelInfo {
    ModelInfo {
        id: id.to_owned(),
        context_limit,
        effort_levels: efforts.iter().map(|effort| (*effort).to_owned()).collect(),
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

    assert_eq!(manager.context_used(&session), 0, "nothing measured yet");

    manager
        .append(
            &session,
            sa_proto::session::kinds::LLM_COMPLETED,
            &json!({"usage": {"prompt_tokens": 1234, "completion_tokens": 5}}),
        )
        .expect("append");

    assert_eq!(
        manager.context_used(&session),
        1234,
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
        manager.reasoning_effort("silent", Some("high")),
        None,
        "an endpoint that lists nothing is sent nothing"
    );
}
