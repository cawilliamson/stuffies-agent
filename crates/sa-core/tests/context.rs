//! the context figure must come from the model and the provider, never from a guess.
#![allow(clippy::expect_used, clippy::panic)]

use std::sync::{Arc, Mutex};

use sa_core::llm::{Delta, ModelInfo, Provider, ScriptedProvider};
use sa_core::manager::{Manager, ManagerConfig};
use sa_core::store::Store;
use serde_json::json;

fn manager(
    dir: &std::path::Path,
    models: Vec<ModelInfo>,
    override_limit: Option<u64>,
) -> Arc<Manager> {
    let store = Store::open(&dir.join("sa.db")).expect("store");
    let provider: Arc<dyn Provider> = Arc::new(ScriptedProvider::new(Vec::<Vec<Delta>>::new()));
    Arc::new(Manager::new(
        store,
        Arc::new(ManagerConfig {
            host_fingerprint: "test-host".to_owned(),
            provider,
            max_tokens: None,
            reasoning_effort: None,
            context_limit: override_limit,
            lock_dir: dir.join("locks"),
            turn_timeout_secs: 30,
            aux_budget_secs: 5,
            default_effort: "medium".to_owned(),
            models: Arc::new(Mutex::new(models)),
        }),
    ))
}

fn model(id: &str, context_limit: Option<u64>) -> ModelInfo {
    ModelInfo {
        id: id.to_owned(),
        context_limit,
    }
}

#[test]
fn the_window_comes_from_the_model_and_is_unknown_when_it_is_not_advertised() {
    let dir = tempfile::tempdir().expect("tempdir");
    let manager = manager(dir.path(), vec![model("big", Some(1_000_000))], None);

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
fn an_operator_override_wins_over_the_endpoint() {
    let dir = tempfile::tempdir().expect("tempdir");
    let manager = manager(
        dir.path(),
        vec![model("big", Some(1_000_000))],
        Some(64_000),
    );
    assert_eq!(manager.context_limit("big"), Some(64_000));
}

#[test]
fn the_used_figure_is_the_prompt_the_provider_counted() {
    let dir = tempfile::tempdir().expect("tempdir");
    let manager = manager(dir.path(), vec![model("big", Some(1_000_000))], None);
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
