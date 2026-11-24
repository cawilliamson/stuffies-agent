//! an interrupted session must be resumable (spec 5.4, 5.6).
#![allow(clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use sa_core::llm::{Delta, Provider, ScriptedProvider};
use sa_core::manager::{Manager, ManagerConfig, NewTurn};
use sa_core::store::Store;
use sa_proto::session::SessionState;

fn manager(dir: &std::path::Path, script: Vec<Vec<Delta>>) -> Arc<Manager> {
    let store = Store::open(&dir.join("sa.db")).expect("store");
    let provider: Arc<dyn Provider> = Arc::new(ScriptedProvider::new(script));
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
            models: Arc::new(std::sync::Mutex::new(Vec::new())),
            mcp: Arc::new(sa_core::mcp::Hub::empty()),
            lsp: Arc::new(sa_core::lsp::Hub::new()),
        }),
    ))
}

fn script() -> Vec<Vec<Delta>> {
    vec![
        vec![
            Delta::Text("carrying on".to_owned()),
            Delta::Done {
                stop_reason: "stop".to_owned(),
                prompt_tokens: 1,
                completion_tokens: 1,
                cached_tokens: 0,
            },
        ],
        vec![
            Delta::Text("second".to_owned()),
            Delta::Done {
                stop_reason: "stop".to_owned(),
                prompt_tokens: 1,
                completion_tokens: 1,
                cached_tokens: 0,
            },
        ],
    ]
}

#[tokio::test]
async fn an_interrupted_session_can_take_a_new_turn() {
    let dir = tempfile::tempdir().expect("tempdir");
    let manager = manager(dir.path(), script());
    let session = manager
        .create_session(dir.path().to_str().expect("utf8"), None, "test")
        .expect("create")
        .session_id;

    // simulate what recovery does after a daemon restart mid-turn.
    manager
        .store()
        .set_session_state(&session, SessionState::Interrupted)
        .expect("interrupt");

    let started = manager.start_turn(&NewTurn {
        detach: true,
        ..NewTurn::new(&session, "keep going")
    });
    assert!(
        started.is_ok(),
        "an interrupted session must accept a turn: {:?}",
        started.err()
    );

    // and it must be back to a writable state.
    let row = manager
        .store()
        .get_session(&session)
        .expect("get")
        .expect("session");
    assert_eq!(row.state, SessionState::Active);
}

#[tokio::test]
async fn a_relocated_session_still_refuses_a_turn() {
    let dir = tempfile::tempdir().expect("tempdir");
    let manager = manager(dir.path(), script());
    let session = manager
        .create_session(dir.path().to_str().expect("utf8"), None, "test")
        .expect("create")
        .session_id;

    manager
        .store()
        .set_session_state(&session, SessionState::Relocated)
        .expect("relocate");

    let started = manager.start_turn(&NewTurn {
        detach: true,
        ..NewTurn::new(&session, "nope")
    });
    assert!(started.is_err(), "a relocated session is read-only");
}

#[tokio::test]
async fn attached_images_do_not_inflate_the_context_estimate() {
    let dir = tempfile::tempdir().expect("tempdir");
    let manager = manager(dir.path(), script());
    let session = manager
        .create_session(dir.path().to_str().expect("utf8"), None, "test")
        .expect("create")
        .session_id;

    let before = manager.estimate_tokens(&session);
    let image = sa_proto::session::ImageAttachment {
        media_type: "image/png".to_owned(),
        // a picture big enough to dwarf the prompt in the log.
        data: "A".repeat(400_000),
        name: Some("shot.png".to_owned()),
    };
    manager
        .start_turn(&NewTurn {
            detach: true,
            images: &[image],
            ..NewTurn::new(&session, "look")
        })
        .expect("start");

    let after = manager.estimate_tokens(&session);
    assert!(
        after < before + 1_000,
        "400k of base64 must not read as 100k tokens: {before} -> {after}"
    );
}
