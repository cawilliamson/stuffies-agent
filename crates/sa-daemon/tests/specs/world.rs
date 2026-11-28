//! the shared world every scenario acts in.
//!
//! one struct, because a scenario is a claim about the daemon and the daemon is one thing:
//! a workspace, a store, a manager with a scripted provider, and whatever the last call
//! answered. helpers here are the vocabulary the steps share, so a step reads as the
//! behaviour it checks rather than as setup.

use std::path::PathBuf;
use std::sync::Arc;

use sa_core::agent::ToolExecutor as _;
use sa_core::llm::{Delta, ModelInfo, Provider, ScriptedProvider};
use sa_core::manager::{Manager, ManagerConfig, NewTurn};
use sa_core::store::Store;
use sa_core::tools::ToolRegistry;
use sa_proto::session::{Event, SessionState};
use serde_json::{json, Value};

/// one scenario's world.
#[derive(Default, cucumber::World)]
pub struct Spec {
    /// the directory the scenario works in, which is also the workspace a tool sees.
    pub dir: Option<tempfile::TempDir>,
    /// a directory outside the workspace, for the escape scenarios.
    pub outside: Option<tempfile::TempDir>,
    /// where the daemon keeps what is its own: the database and the checkpoints. never the
    /// workspace, which is the whole point of the scenario that checks it.
    pub state: Option<tempfile::TempDir>,
    /// the script the provider replays, consumed one stream at a time.
    pub script: Vec<Vec<Delta>>,
    pub models: Vec<ModelInfo>,
    pub manager: Option<Arc<Manager>>,
    /// the provider the manager replays, kept so a later step can feed it another stream.
    pub provider: Option<Arc<ScriptedProvider>>,
    pub session: Option<String>,
    /// what a tool call answered, or the reason it did not.
    pub outcome: Option<Result<String, String>>,
    /// the last error any step produced, for the steps that expect one.
    pub error: Option<String>,
    /// a line of a listing or a rendered answer, when the step produced one.
    pub rendered: Option<String>,
    /// the wire: a dispatcher over the same manager, and one connection to drive it.
    pub wire: Option<(
        sa_core::dispatcher::Dispatcher,
        sa_core::dispatcher::ConnState,
    )>,
    /// the last response frame the wire produced.
    pub response: Option<Value>,
}

impl std::fmt::Debug for Spec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Spec")
            .field("session", &self.session)
            .field("outcome", &self.outcome)
            .finish_non_exhaustive()
    }
}

impl Spec {
    pub fn root(&self) -> PathBuf {
        self.dir
            .as_ref()
            .expect("a workspace step comes first")
            .path()
            .to_path_buf()
    }

    /// the workspace, made on first use so a scenario that opens with a call needs no given.
    pub fn ensure_dir(&mut self) -> PathBuf {
        if self.dir.is_none() {
            self.dir = Some(tempfile::tempdir().expect("tempdir"));
        }
        self.root()
    }

    /// a file in the workspace, with its parents.
    pub fn write_file(&mut self, path: &str, body: &str) {
        let target = self.ensure_dir().join(path);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).expect("mkdir");
        }
        std::fs::write(&target, body).expect("write");
    }

    pub fn read_file(&self, path: &str) -> String {
        std::fs::read_to_string(self.root().join(path)).expect("read")
    }

    pub fn exists(&self, path: &str) -> bool {
        self.root().join(path).exists()
    }

    /// a directory outside the workspace holding one file, and a link to it from inside.
    pub fn link_out(&mut self, name: &str) {
        let root = self.ensure_dir();
        let outside = tempfile::tempdir().expect("tempdir");
        std::fs::write(outside.path().join("secret"), "s").expect("write");
        std::os::unix::fs::symlink(outside.path(), root.join(name)).expect("symlink");
        self.outside = Some(outside);
    }

    pub fn outside_root(&self) -> PathBuf {
        self.outside
            .as_ref()
            .expect("a link step comes first")
            .path()
            .to_path_buf()
    }

    /// a registry over the workspace. rebuilt each time, because it holds only the path.
    /// hands the provider the streams this step set up, so a later turn has one to replay.
    pub fn feed_script(&mut self) {
        if let Some(provider) = &self.provider {
            for stream in std::mem::take(&mut self.script) {
                provider.push(stream);
            }
        }
    }

    pub fn state_dir(&self) -> PathBuf {
        self.state
            .as_ref()
            .expect("a manager step comes first")
            .path()
            .to_path_buf()
    }

    pub fn registry(&self) -> ToolRegistry {
        ToolRegistry::new(self.root())
    }

    /// calls a built-in tool and keeps what it answered.
    pub async fn call_tool(&mut self, name: &str, args: Value) {
        self.ensure_dir();
        let registry = self.registry();
        let call = sa_core::llm::ToolCall {
            id: "spec".to_owned(),
            name: name.to_owned(),
            arguments: args.to_string(),
        };
        self.outcome = Some(registry.execute(call).await.map(|outcome| outcome.output));
    }

    /// calls a tool that needs the daemon's hubs, which the plain registry does not carry.
    pub async fn call_lsp(&mut self, args: Value) {
        self.ensure_dir();
        let registry = ToolRegistry::with_hubs(
            self.root(),
            std::sync::Arc::new(sa_core::mcp::Hub::empty()),
            std::sync::Arc::new(sa_core::lsp::Hub::new()),
        );
        let call = sa_core::llm::ToolCall {
            id: "spec".to_owned(),
            name: "lsp".to_owned(),
            arguments: args.to_string(),
        };
        self.outcome = Some(registry.execute(call).await.map(|outcome| outcome.output));
    }

    /// the answer to the last call, or `None` when the language server could not be reached.
    /// a server that never answered says nothing about the file, so the claim is not tested
    /// rather than tested and failed: the machine's language server is the variable.
    pub fn answered_with_server(&self) -> Option<&str> {
        match self.outcome.as_ref()? {
            Ok(text) => Some(text),
            Err(error) => {
                eprintln!("no language server answered, skipping: {error}");
                None
            }
        }
    }

    /// the answer to the last tool call, or a panic naming what went wrong instead.
    pub fn answered(&self) -> &str {
        match self.outcome.as_ref().expect("a call step comes first") {
            Ok(text) => text,
            Err(error) => panic!("the call failed: {error}"),
        }
    }

    /// the refusal to the last tool call.
    pub fn refused(&self) -> &str {
        match self.outcome.as_ref().expect("a call step comes first") {
            Ok(text) => panic!("the call was accepted: {text}"),
            Err(error) => error,
        }
    }

    // ---- the daemon -------------------------------------------------

    /// a manager over a store in the workspace, replaying the script the scenario set.
    pub fn ensure_manager(&mut self) -> Arc<Manager> {
        if let Some(manager) = &self.manager {
            return Arc::clone(manager);
        }
        self.ensure_dir();
        let state = tempfile::tempdir().expect("state");
        let store = Store::open(&state.path().join("sa.db")).expect("store");
        let scripted = Arc::new(ScriptedProvider::new(std::mem::take(&mut self.script)));
        let provider: Arc<dyn Provider> = scripted.clone();
        let manager = Arc::new(Manager::new(
            store,
            Arc::new(ManagerConfig {
                host_fingerprint: "test-host".to_owned(),
                provider,
                max_tokens: None,
                lock_dir: state.path().join("locks"),
                checkpoint_dir: state.path().join("checkpoints"),
                turn_timeout_secs: 30,
                aux_budget_secs: 5,
                models: Arc::new(std::sync::Mutex::new(std::mem::take(&mut self.models))),
                mcp: Arc::new(sa_core::mcp::Hub::empty()),
                lsp: Arc::new(sa_core::lsp::Hub::new()),
            }),
        ));
        self.state = Some(state);
        self.provider = Some(scripted);
        self.manager = Some(Arc::clone(&manager));
        manager
    }

    /// a session in the workspace, made on first use.
    pub fn ensure_session(&mut self) -> String {
        if let Some(id) = &self.session {
            return id.clone();
        }
        let dir = self.ensure_dir();
        let manager = self.ensure_manager();
        let id = manager
            .create_session(dir.to_str().expect("utf8"), None, "test")
            .expect("create")
            .session_id;
        self.session = Some(id.clone());
        id
    }

    const fn ensure_manager_ref(&self) -> &Arc<Manager> {
        self.manager.as_ref().expect("a manager step comes first")
    }

    /// appends an event to the session's log, the way the daemon does.
    pub fn record(&self, kind: &str, payload: Value) {
        let id = self.session.as_ref().expect("a session step comes first");
        self.ensure_manager_ref()
            .append(id, kind, &payload)
            .expect("append");
    }

    /// the session's events, oldest first.
    pub fn events(&self) -> Vec<Event> {
        let id = self.session.as_ref().expect("a session step comes first");
        self.ensure_manager_ref()
            .store()
            .events_since(id, 0, 10_000)
            .expect("events")
            .0
    }

    /// the payloads of one kind, in order.
    pub fn of_kind(&self, kind: &str) -> Vec<Value> {
        self.events()
            .into_iter()
            .filter(|event| event.kind == kind)
            .map(|event| event.payload)
            .collect()
    }

    pub fn set_state(&mut self, state: SessionState) {
        let id = self.ensure_session();
        self.ensure_manager()
            .store()
            .set_session_state(&id, state)
            .expect("state");
    }

    /// runs one turn to its end and waits for the terminal event, because the manager starts
    /// a turn in the background and a scenario has to be able to look at the log afterwards.
    pub async fn run_turn(&mut self, prompt: &str) {
        let id = self.ensure_session();
        let manager = self.ensure_manager();
        self.feed_script();
        let before = manager.store().last_seq(&id).unwrap_or_default();
        manager
            .start_turn(&NewTurn {
                detach: true,
                ..NewTurn::new(&id, prompt)
            })
            .expect("start");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while std::time::Instant::now() < deadline {
            let (events, _) = manager
                .store()
                .events_since(&id, before, 1_000)
                .expect("events");
            if events
                .iter()
                .any(|event| sa_proto::session::kinds::TURN_TERMINAL.contains(&event.kind.as_str()))
            {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("the turn never finished");
    }

    // ---- the wire ---------------------------------------------------

    /// a handshaken dispatcher over the same manager, so a scenario can drive the protocol
    /// the way a client does.
    pub fn ensure_wire(&mut self) {
        if self.wire.is_some() {
            return;
        }
        let manager = self.ensure_manager();
        let dispatcher = sa_core::dispatcher::Dispatcher::new(manager);
        let mut conn = sa_core::dispatcher::ConnState::new(1);
        let hello = sa_proto::rpc::Request::new(
            sa_proto::rpc::Id::Number(1),
            "hello",
            Some(json!({"protocol": 1, "client": "spec", "client_version": "0"})),
        );
        let _ = dispatcher.handle(&mut conn, hello);
        self.wire = Some((dispatcher, conn));
    }

    /// sends one request over the wire and keeps the frame that came back.
    pub fn rpc(&mut self, method: &str, params: Value) {
        self.ensure_wire();
        let (dispatcher, conn) = self.wire.as_mut().expect("a wire step comes first");
        let request =
            sa_proto::rpc::Request::new(sa_proto::rpc::Id::Number(7), method, Some(params));
        let response = dispatcher.handle(conn, request);
        self.response = Some(response.map_or(Value::Null, |frame| {
            serde_json::to_value(frame).expect("frame")
        }));
    }

    /// sends a raw request frame, for the ones that are malformed on purpose.
    pub fn rpc_request(&mut self, request: sa_proto::rpc::Request) {
        self.ensure_wire();
        let (dispatcher, conn) = self.wire.as_mut().expect("a wire step comes first");
        let response = dispatcher.handle(conn, request);
        self.response = Some(response.map_or(Value::Null, |frame| {
            serde_json::to_value(frame).expect("frame")
        }));
    }

    /// the error code of the last response, when it carried one.
    pub fn code(&self) -> i64 {
        self.response
            .as_ref()
            .and_then(|frame| frame["error"]["code"].as_i64())
            .unwrap_or_else(|| panic!("no error code in {:?}", self.response))
    }

    /// the message of the last response's error.
    pub fn message(&self) -> String {
        self.response
            .as_ref()
            .and_then(|frame| frame["error"]["message"].as_str())
            .unwrap_or_default()
            .to_owned()
    }

    /// the result of the last response, or a panic naming the error it carried.
    pub fn result(&self) -> Value {
        self.response
            .as_ref()
            .and_then(|frame| frame.get("result"))
            .cloned()
            .unwrap_or_else(|| panic!("no result in {:?}", self.response))
    }
}

/// a local http server that answers every request with `body`, so a scenario about the
/// network needs no network. it runs until the process ends, which for a scenario is the
/// same thing.
pub async fn serve(body: Vec<u8>) -> String {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            let body = body.clone();
            tokio::spawn(async move {
                let mut buf = [0_u8; 8192];
                let _ = stream.read(&mut buf).await;
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(head.as_bytes()).await;
                let _ = stream.write_all(&body).await;
                let _ = stream.flush().await;
            });
        }
    });
    format!("http://{addr}/")
}

/// a scripted stream that answers with one sentence and stops.
pub fn says(text: &str) -> Vec<Delta> {
    vec![
        Delta::Text(text.to_owned()),
        Delta::Done {
            stop_reason: "stop".to_owned(),
            prompt_tokens: 1,
            completion_tokens: 1,
            cached_tokens: 0,
        },
    ]
}

/// a model with a window and the effort values it accepts.
pub fn model_with(id: &str, limit: Option<u64>, efforts: &[&str]) -> ModelInfo {
    ModelInfo {
        id: id.to_owned(),
        context_limit: limit,
        effort_levels: efforts.iter().map(|e| (*e).to_owned()).collect(),
        default_effort: None,
    }
}
