//! behaviour tests for the v1.0 daemon skeleton (spec 14, appendix A).
#![allow(clippy::expect_used, clippy::panic)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use sa_core::llm::{Provider, ScriptedProvider};
use sa_daemon::{run, Config};
use sa_proto::rpc::PROTOCOL_VERSION;
use sa_proto::MAX_LINE_LEN;
use serde_json::{json, Value};
use tokio::net::UnixStream;
use tokio_util::codec::{Framed, LinesCodec};

struct TestDaemon {
    dir: tempfile::TempDir,
    socket: PathBuf,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    handle: tokio::task::JoinHandle<()>,
}

impl TestDaemon {
    async fn start() -> Self {
        Self::start_with(Arc::new(ScriptedProvider::new(Vec::new()))).await
    }

    async fn start_with(provider: Arc<dyn Provider>) -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let cfg = Config::at(dir.path());
        let socket = cfg.socket_path.clone();
        let (stop, wait) = tokio::sync::oneshot::channel();
        let handle = tokio::spawn(async move {
            run(cfg, provider, async move {
                let _ = wait.await;
            })
            .await
            .expect("daemon run");
        });

        for _ in 0..500 {
            if socket.exists() {
                return Self {
                    dir,
                    socket,
                    stop: Some(stop),
                    handle,
                };
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("daemon socket never appeared at {}", socket.display());
    }

    async fn connect(&self) -> Framed<UnixStream, LinesCodec> {
        let stream = UnixStream::connect(&self.socket).await.expect("connect");
        Framed::new(stream, LinesCodec::new_with_max_length(MAX_LINE_LEN))
    }

    fn dir_path(&self) -> &Path {
        self.dir.path()
    }

    async fn shutdown(mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        let _ = self.handle.await;
    }
}

async fn call(
    framed: &mut Framed<UnixStream, LinesCodec>,
    id: i64,
    method: &str,
    params: Value,
) -> Value {
    let line = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
    framed.send(line.to_string()).await.expect("send");
    // a notification the daemon pushes while the call is in flight is not this call's
    // reply, so it is read past rather than mistaken for one.
    loop {
        let raw = framed
            .next()
            .await
            .expect("a frame")
            .expect("a valid frame");
        let frame: Value = serde_json::from_str(&raw).expect("json");
        if frame.get("id").and_then(Value::as_i64) == Some(id) {
            return frame;
        }
    }
}

fn hello(protocol: u32) -> Value {
    json!({"protocol": protocol, "client": "test", "client_version": "0"})
}

#[tokio::test]
async fn socket_is_0600_and_dir_is_0700() {
    let daemon = TestDaemon::start().await;

    let socket_mode = std::fs::metadata(&daemon.socket)
        .expect("socket metadata")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(socket_mode, 0o600, "socket mode");

    let runtime_dir = daemon.dir_path().join("run");
    let dir_mode = std::fs::metadata(&runtime_dir)
        .expect("dir metadata")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(dir_mode, 0o700, "runtime dir mode");

    daemon.shutdown().await;
}

#[tokio::test]
async fn serves_ping_over_socket() {
    let daemon = TestDaemon::start().await;
    let mut framed = daemon.connect().await;

    let handshake = call(&mut framed, 1, "hello", hello(PROTOCOL_VERSION)).await;
    assert_eq!(handshake["result"]["protocol"], PROTOCOL_VERSION);

    let ping = call(&mut framed, 2, "ping", json!({})).await;
    assert!(ping["error"].is_null(), "ping failed: {ping}");
    assert_eq!(ping["result"]["sessions"], 0);
    assert!(ping["result"]["version"].is_string());

    daemon.shutdown().await;
}

#[tokio::test]
async fn rejects_ping_before_hello() {
    let daemon = TestDaemon::start().await;
    let mut framed = daemon.connect().await;

    let response = call(&mut framed, 1, "ping", json!({})).await;
    assert_eq!(response["error"]["code"], -32003, "handshake_required");

    daemon.shutdown().await;
}

#[tokio::test]
async fn rejects_version_mismatch_naming_both() {
    let daemon = TestDaemon::start().await;
    let mut framed = daemon.connect().await;

    let response = call(&mut framed, 1, "hello", hello(PROTOCOL_VERSION + 1)).await;
    assert_eq!(response["error"]["code"], -32000, "version_mismatch");

    let text = response["error"]["message"].as_str().expect("message");
    assert!(text.contains(&PROTOCOL_VERSION.to_string()), "{text}");
    assert!(text.contains(&(PROTOCOL_VERSION + 1).to_string()), "{text}");

    daemon.shutdown().await;
}

#[tokio::test]
async fn rejects_oversized_line_and_closes() {
    let daemon = TestDaemon::start().await;
    let mut framed = daemon.connect().await;

    call(&mut framed, 1, "hello", hello(PROTOCOL_VERSION)).await;

    let oversized = "a".repeat(MAX_LINE_LEN + 1);
    framed.send(oversized).await.expect("send oversized line");

    let raw = framed
        .next()
        .await
        .expect("an error frame")
        .expect("a valid frame");
    let response: Value = serde_json::from_str(&raw).expect("json");
    assert_eq!(response["error"]["code"], -32002, "line_too_long");

    // the connection is closed afterwards, not left desynchronised.
    assert!(framed.next().await.is_none(), "connection must close");

    daemon.shutdown().await;
}

#[tokio::test]
async fn malformed_json_is_a_parse_error() {
    let daemon = TestDaemon::start().await;
    let mut framed = daemon.connect().await;

    framed.send("{not json".to_owned()).await.expect("send");
    let raw = framed.next().await.expect("a frame").expect("valid frame");
    let response: Value = serde_json::from_str(&raw).expect("json");
    assert_eq!(response["error"]["code"], -32700, "parse_error");

    daemon.shutdown().await;
}

#[tokio::test]
async fn shutdown_removes_the_socket() {
    let daemon = TestDaemon::start().await;
    let socket = daemon.socket.clone();
    assert!(socket.exists());

    daemon.shutdown().await;
    assert!(!socket.exists(), "socket must be removed on shutdown");
}

#[tokio::test]
async fn refuses_to_bind_over_a_live_daemon() {
    let daemon = TestDaemon::start().await;

    let cfg = Config::at(daemon.dir_path());
    let err = run(cfg, Arc::new(ScriptedProvider::new(Vec::new())), async {})
        .await
        .expect_err("must refuse");
    assert!(
        err.to_string().contains("another daemon"),
        "unexpected error: {err}"
    );

    daemon.shutdown().await;
}

#[tokio::test]
async fn bind_creates_a_missing_socket_directory() {
    let dir = tempfile::tempdir().expect("tempdir");
    let socket = dir.path().join("deep").join("nested").join("daemon.sock");
    assert!(!socket.parent().expect("parent").exists());

    let listener = sa_daemon::socket::bind(&socket).expect("bind");
    drop(listener);

    assert!(socket.exists(), "socket must be created");
    let mode = std::fs::metadata(socket.parent().expect("parent"))
        .expect("metadata")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o700, "created directory must be 0700");
}

#[test]
fn the_default_state_dir_is_absolute_and_outside_the_cwd() {
    // an unset XDG_STATE_HOME must fall back to $HOME/.local/state, never to `./.sa`.
    let cfg = Config::from_env();
    assert!(
        cfg.state_dir.is_absolute(),
        "state dir must be absolute, got {}",
        cfg.state_dir.display()
    );
    assert!(
        !cfg.state_dir.starts_with("."),
        "state dir must not be relative: {}",
        cfg.state_dir.display()
    );
}

// ---- standalone (spec 4.4) ------------------------------------------------

/// a standalone run serves the protocol over a socketpair and touches neither the daemon's
/// socket nor its database: the whole point is testing one build without disturbing a
/// daemon that may be running beside it.
#[tokio::test]
async fn standalone_serves_over_a_socketpair_and_writes_no_state() {
    let dir = tempfile::tempdir().expect("tempdir");
    let cfg = Config::at(dir.path());
    let socket = cfg.socket_path.clone();
    let db = cfg.db_path.clone();

    let client = sa_daemon::standalone(cfg, Arc::new(ScriptedProvider::new(Vec::new())))
        .await
        .expect("standalone backend");
    let mut framed = Framed::new(client, LinesCodec::new_with_max_length(MAX_LINE_LEN));

    let hello = call(&mut framed, 1, "hello", hello(PROTOCOL_VERSION)).await;
    assert!(hello["result"]["protocol"].is_u64(), "{hello}");

    let ping = call(&mut framed, 2, "ping", json!({})).await;
    assert_eq!(ping["result"]["sessions"], 0, "a fresh in-memory store");

    assert!(
        !socket.exists(),
        "standalone must not bind the daemon's socket at {}",
        socket.display()
    );
    assert!(
        !db.exists(),
        "standalone must not create the daemon's database at {}",
        db.display()
    );
}

/// sessions created by a standalone run do not outlive it, so a one-off test cannot leave
/// rows behind for the daemon to reconcile.
#[tokio::test]
async fn standalone_sessions_are_discarded_with_the_run() {
    let dir = tempfile::tempdir().expect("tempdir");
    let cfg = Config::at(dir.path());
    let db = cfg.db_path.clone();

    for _ in 0..2 {
        let client =
            sa_daemon::standalone(cfg.clone(), Arc::new(ScriptedProvider::new(Vec::new())))
                .await
                .expect("standalone backend");
        let mut framed = Framed::new(client, LinesCodec::new_with_max_length(MAX_LINE_LEN));
        call(&mut framed, 1, "hello", hello(PROTOCOL_VERSION)).await;
        let created = call(
            &mut framed,
            2,
            "session.create",
            json!({"cwd": dir.path().display().to_string()}),
        )
        .await;
        assert!(created["result"]["session_id"].is_string(), "{created}");

        let listed = call(
            &mut framed,
            3,
            "session.list",
            json!({"include_deleted": false}),
        )
        .await;
        assert_eq!(
            listed["result"].as_array().map(Vec::len),
            Some(1),
            "{listed}"
        );
    }

    assert!(!db.exists(), "no database was ever created");
}
