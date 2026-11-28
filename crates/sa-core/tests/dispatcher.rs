//! the protocol surface: what a frame without an id does (spec 3.2).
#![allow(clippy::expect_used, clippy::panic)]

use std::sync::{Arc, Mutex};

use sa_core::dispatcher::{ConnState, Dispatcher};
use sa_core::llm::{Provider, ScriptedProvider};
use sa_core::manager::{Manager, ManagerConfig};
use sa_core::store::Store;
use sa_proto::rpc::{Id, Request};
use serde_json::json;

/// a dispatcher over an in-memory store, already handshaken: these tests are about what a
/// frame does after the gate, not about the gate.
fn handshaken() -> (Dispatcher, ConnState, Arc<Manager>, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let provider = Arc::new(ScriptedProvider::new(Vec::new()));
    let manager = Arc::new(Manager::new(
        Store::open_in_memory().expect("store"),
        Arc::new(ManagerConfig {
            host_fingerprint: "test-host".to_owned(),
            provider: Arc::clone(&provider) as Arc<dyn Provider>,
            max_tokens: None,
            lock_dir: dir.path().join("locks"),
            checkpoint_dir: dir.path().join("checkpoints"),
            turn_timeout_secs: 30,
            aux_budget_secs: 5,
            models: Arc::new(Mutex::new(Vec::new())),
            mcp: Arc::new(sa_core::mcp::Hub::empty()),
            lsp: Arc::new(sa_core::lsp::Hub::new()),
        }),
    ));
    let dispatcher = Dispatcher::new(Arc::clone(&manager));

    let mut conn = ConnState::new(1);
    let hello = Request::new(
        Id::Number(1),
        "hello",
        Some(json!({"protocol": 1, "client": "test", "client_version": "0"})),
    );
    assert!(
        dispatcher.handle(&mut conn, hello).is_some(),
        "the handshake is answered"
    );

    (dispatcher, conn, manager, dir)
}

/// a notification asks for the work and for no reply. dropping the frame unperformed is the
/// one failure a fire-and-forget caller cannot see: no error, no effect.
#[tokio::test]
async fn a_frame_without_an_id_is_performed_and_never_answered() {
    let (dispatcher, mut conn, manager, dir) = handshaken();

    let mut create = Request::new(
        Id::Number(2),
        "session.create",
        Some(json!({"cwd": dir.path().display().to_string()})),
    );
    create.id = None;
    assert!(
        dispatcher.handle(&mut conn, create).is_none(),
        "a notification is never answered"
    );

    // and the work was done, which is the half that was missing.
    assert_eq!(
        manager.list_sessions(false).expect("sessions").len(),
        1,
        "the session the notification asked for exists"
    );

    let list = Request::new(Id::Number(3), "session.list", Some(json!({})));
    let response = dispatcher
        .handle(&mut conn, list)
        .expect("a request is answered");
    let sessions = response.into_result().expect("result");
    assert_eq!(sessions.as_array().map(Vec::len), Some(1), "{sessions}");
}

/// an error from a notification has nobody to go to, so it is recorded rather than sent —
/// and it must not take the connection with it.
#[tokio::test]
async fn a_failing_notification_writes_no_frame_and_leaves_the_connection_usable() {
    let (dispatcher, mut conn, _manager, _dir) = handshaken();

    let mut unknown = Request::new(Id::Number(2), "no.such.method", None);
    unknown.id = None;
    assert!(
        dispatcher.handle(&mut conn, unknown).is_none(),
        "an unknown notification is still not answered"
    );

    let ping = Request::new(Id::Number(3), "ping", None);
    let response = dispatcher
        .handle(&mut conn, ping)
        .expect("the connection still answers");
    assert!(response.into_result().is_ok(), "ping succeeds");
}
