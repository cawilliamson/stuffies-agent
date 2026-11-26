//! session manager: the only writer of session state (spec 2.4).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use sa_proto::error::{code, RpcError};
use sa_proto::session::kinds;
use sa_proto::session::{
    AttachResult, CreateSessionResult, Event, ReplayResult, SessionState, SessionSummary,
    TurnStartResult, TurnStatus, Usage,
};
use serde_json::{json, Value};
use tokio::sync::mpsc;

use crate::agent::{Agent, EventSink, ToolExecutor, TurnRequest};
use crate::llm::Provider;
use crate::store::{NewSession, SessionRow, Store, StoreError};
use crate::tools::ToolRegistry;
use std::fmt::Write as _;

/// live queue bound for one client (spec 3.3).
pub const CLIENT_QUEUE: usize = 4096;
/// compaction fires at this fraction of the model context limit (spec 10.4).
pub const COMPACTION_TRIGGER: f64 = 0.80;
/// turns kept verbatim across a compaction.
pub const COMPACTION_RETAIN_TURNS: usize = 4;
/// rough token estimate. ponytail: four characters per token, no tokeniser dependency.
pub const CHARS_PER_TOKEN: u64 = 4;

#[derive(Debug, thiserror::Error)]
pub enum ManagerError {
    #[error("store: {0}")]
    Store(#[from] StoreError),
    #[error("rpc: {0}")]
    Rpc(#[from] RpcError),
    #[error("llm: {0}")]
    Llm(#[from] crate::llm::LlmError),
}

impl ManagerError {
    #[must_use]
    pub fn into_rpc(self) -> RpcError {
        match self {
            Self::Rpc(err) => err,
            Self::Store(err) => RpcError::new(code::INTERNAL_ERROR, err.to_string()),
            Self::Llm(err) => RpcError::new(code::INTERNAL_ERROR, err.to_string()),
        }
    }
}

fn not_found(id: &str) -> RpcError {
    RpcError::new(code::SESSION_NOT_FOUND, format!("session not found: {id}"))
}

pub struct ManagerConfig {
    pub host_fingerprint: String,
    pub provider: Arc<dyn Provider>,
    pub max_tokens: Option<u64>,
    pub lock_dir: PathBuf,
    /// where checkpoints live. outside the workspace, always (spec 8.4).
    pub checkpoint_dir: PathBuf,
    pub turn_timeout_secs: u64,
    /// how long an auxiliary model call (title, compaction) may take before it is abandoned.
    /// without this a stalled provider blocks the turn before the agent even starts.
    pub aux_budget_secs: u64,
    /// what the endpoint reports it serves. shared so a background refresher can fill it in.
    pub models: Arc<Mutex<Vec<crate::llm::ModelInfo>>>,
    /// the mcp servers this daemon reached. their tools join the built-ins for every turn.
    pub mcp: Arc<crate::mcp::Hub>,
    /// the language servers this daemon started. they answer about the code and report on
    /// what a turn changed.
    pub lsp: Arc<crate::lsp::Hub>,
}

struct Subscription {
    conn: u64,
    session_id: String,
    exclude_kinds: Vec<String>,
}

struct RunningTurn {
    turn_id: String,
    cancel: Arc<AtomicBool>,
    /// when the turn started, so a status query can say how long it has been running
    /// rather than reporting the moment it was asked.
    since: i64,
}

/// what the operator queued while a session was running a turn (spec 3.7).
///
/// the queue belongs to the session, not to one turn: a follow-up outlives the turn that
/// was running when it was typed, and continues the session as the next one.
#[derive(Default)]
struct Queue {
    /// delivered between model round-trips, one per round-trip.
    steering: Vec<String>,
    /// delivered after the running turn finishes, as a new turn.
    follow_up: Vec<String>,
}

impl Queue {
    fn state(&self) -> sa_proto::session::QueueState {
        sa_proto::session::QueueState {
            steering: self.steering.clone(),
            follow_up: self.follow_up.clone(),
        }
    }
}

#[derive(Default)]
struct Inner {
    next_conn: u64,
    next_sub: u64,
    conns: HashMap<u64, mpsc::Sender<Value>>,
    subs: HashMap<u64, Subscription>,
    turns: HashMap<String, RunningTurn>,
    queues: HashMap<String, Queue>,
}

/// what the goal loop knows about a session, read back from its own log.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct GoalStatus {
    pub active: bool,
    pub text: String,
    pub iterations: u32,
    pub max_iterations: u32,
    pub summary: Option<String>,
}

/// what a caller asks a turn to be.
#[derive(Debug, Clone, Copy)]
pub struct NewTurn<'a> {
    pub session_id: &'a str,
    pub prompt: &'a str,
    pub images: &'a [sa_proto::session::ImageAttachment],
    pub detach: bool,
    /// the client that asked, when one did.
    pub client: Option<&'a str>,
    /// the interrupted turn this one carries on from, when it does (spec 5.6).
    pub resumed_from: Option<&'a str>,
    /// continue the last assistant message rather than answering a new prompt.
    pub continue_final: bool,
}

impl<'a> NewTurn<'a> {
    /// a turn asked for by a client, with no interruption behind it.
    #[must_use]
    pub const fn new(session_id: &'a str, prompt: &'a str) -> Self {
        Self {
            session_id,
            prompt,
            images: &[],
            detach: false,
            client: None,
            resumed_from: None,
            continue_final: false,
        }
    }
}

pub struct Manager {
    store: Store,
    config: Arc<ManagerConfig>,
    inner: Mutex<Inner>,
}

impl Manager {
    #[must_use]
    pub fn new(store: Store, config: Arc<ManagerConfig>) -> Self {
        Self {
            store,
            config,
            inner: Mutex::new(Inner::default()),
        }
    }

    #[must_use]
    pub const fn store(&self) -> &Store {
        &self.store
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Inner>, ManagerError> {
        self.inner
            .lock()
            .map_err(|_| ManagerError::Rpc(RpcError::new(code::INTERNAL_ERROR, "manager lock")))
    }

    // ---- connections ------------------------------------------------

    pub fn register_connection(&self) -> Result<(u64, mpsc::Receiver<Value>), ManagerError> {
        let (tx, rx) = mpsc::channel(CLIENT_QUEUE);
        let mut inner = self.lock()?;
        inner.next_conn += 1;
        let id = inner.next_conn;
        inner.conns.insert(id, tx);
        Ok((id, rx))
    }

    pub fn drop_connection(&self, conn: u64) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.conns.remove(&conn);
            inner.subs.retain(|_, sub| sub.conn != conn);
        }
    }

    /// tells every connected client that the set of sessions changed, so each window can
    /// bring its tab strip back in line. sessions are not owned by a client, so this is a
    /// plain announcement rather than a handover.
    fn announce_sessions(&self) {
        let Ok(inner) = self.inner.lock() else { return };
        for conn in inner.conns.keys().copied() {
            Self::push(
                &inner,
                conn,
                json!({"jsonrpc": "2.0", "method": "sessions.changed", "params": {}}),
            );
        }
    }

    fn push(inner: &Inner, conn: u64, value: Value) {
        if let Some(tx) = inner.conns.get(&conn) {
            if tx.try_send(value).is_err() {
                tracing::debug!(conn, "client queue full, notification dropped");
            }
        }
    }

    fn notify_event(&self, session_id: &str, seq: u64, kind: &str, payload: &Value) {
        let Ok(inner) = self.inner.lock() else { return };
        for sub in inner.subs.values() {
            if sub.session_id != session_id {
                continue;
            }
            if sub.exclude_kinds.iter().any(|excluded| excluded == kind) {
                continue;
            }
            let event = json!({
                "session_id": session_id,
                "seq": seq,
                "kind": kind,
                "payload": payload,
            });
            Self::push(
                &inner,
                sub.conn,
                json!({"jsonrpc": "2.0", "method": "event", "params": event}),
            );
        }
    }

    // ---- events -----------------------------------------------------

    pub fn append(&self, session_id: &str, kind: &str, payload: &Value) -> Result<u64, StoreError> {
        // every payload carries its own version (spec 5.2), so a reader can refuse one it
        // does not understand rather than guess at it. it is set here, the one write path,
        // so no caller can forget it.
        let mut payload = payload.clone();
        if let Value::Object(map) = &mut payload {
            map.entry("v").or_insert(json!(1));
        }
        let seq = self.store.append_event(session_id, kind, &payload)?;
        // what was just written is what is pushed: re-reading it from the store is a second
        // query per event, and every streamed delta is an event.
        self.notify_event(session_id, seq, kind, &payload);
        Ok(seq)
    }

    // ---- sessions ---------------------------------------------------

    pub fn create_session(
        &self,
        cwd: &str,
        title: Option<String>,
        client: &str,
    ) -> Result<CreateSessionResult, ManagerError> {
        let id = uuid::Uuid::new_v4().to_string();
        // an explicit title is the operator's choice and is never overwritten by the model.
        let titled = title.is_some();
        let canonical = std::fs::canonicalize(cwd).unwrap_or_else(|_| PathBuf::from(cwd));
        let row = self.store.create_session(&NewSession {
            id: id.clone(),
            title: title.unwrap_or_else(|| default_title(&canonical)),
            cwd: canonical.display().to_string(),
            repo_id: None,
            model: {
                let model = self.default_model();
                (!model.is_empty()).then_some(model)
            },
            // the level the server defaults to, when the probe found it; an operator
            // override otherwise. never a value this build invented.
            effort: self.default_effort_for(&self.default_model()),
            owner_host: self.config.host_fingerprint.clone(),
            owner_epoch: 0,
        })?;

        let seq = self.append(
            &id,
            kinds::SESSION_CREATED,
            &json!({
                "title": row.title,
                "cwd": row.cwd,
                "client": client,
                "titled": titled,
                "owner": {
                    "host_fingerprint": self.config.host_fingerprint,
                    "epoch": 0,
                },
                "knowledge_namespaces": ["shared"],
            }),
        )?;

        // what the model will be given for this directory, recorded so the operator can
        // see it rather than guess.
        let _ = self.announce_context(&id, &row.cwd);

        self.announce_sessions();
        Ok(CreateSessionResult {
            session_id: id,
            seq,
        })
    }

    /// what the endpoint reports it serves. may be empty until discovery succeeds.
    #[must_use]
    pub fn models(&self) -> Vec<crate::llm::ModelInfo> {
        self.config
            .models
            .lock()
            .map(|models| models.clone())
            .unwrap_or_default()
    }

    /// the model turns use when a session has not selected one. empty when not yet known.
    #[must_use]
    pub fn default_model(&self) -> String {
        self.served_model(|_| true)
            .map(|model| model.id)
            .unwrap_or_default()
    }

    /// what the endpoint said about one model. one lock, one lookup: every caller below
    /// asks the same question, and none of them needs the whole catalog.
    fn served_model(
        &self,
        which: impl Fn(&crate::llm::ModelInfo) -> bool,
    ) -> Option<crate::llm::ModelInfo> {
        self.config
            .models
            .lock()
            .ok()?
            .iter()
            .find(|model| which(model))
            .cloned()
    }

    /// the context window for a model, as the endpoint advertised it. `None` means unknown,
    /// and nothing downstream pretends otherwise.
    #[must_use]
    pub fn context_limit(&self, model: &str) -> Option<u64> {
        self.served_model(|known| known.id == model)
            .and_then(|known| known.context_limit)
    }

    /// the mcp servers this daemon reached, for the operator to inspect.
    #[must_use]
    pub fn mcp_status(&self) -> Vec<crate::mcp::ServerStatus> {
        self.config.mcp.status()
    }

    /// the language servers this build knows about, and which are installed and running.
    #[must_use]
    pub fn lsp_status(&self) -> Vec<crate::lsp::ServerStatus> {
        self.config.lsp.status()
    }

    /// the effort a session should start on: the level the server itself defaults to. none
    /// when the endpoint did not say, which leaves the server's own behaviour alone.
    #[must_use]
    pub fn default_effort_for(&self, model: &str) -> Option<String> {
        self.served_model(|known| known.id == model)
            .and_then(|known| known.default_effort)
    }

    /// the least effort this model offers, for the calls that only need a sentence: a title
    /// or a summary. none when nothing is known, which leaves the server's own default.
    #[must_use]
    pub fn cheapest_effort(&self, model: &str) -> Option<String> {
        self.effort_levels(model).first().cloned()
    }

    /// the reasoning effort values this model accepts, in the endpoint's own order. empty
    /// means the endpoint advertises none, and none is sent.
    #[must_use]
    pub fn effort_levels(&self, model: &str) -> Vec<String> {
        self.served_model(|known| known.id == model)
            .map(|known| known.effort_levels)
            .unwrap_or_default()
    }

    /// the effort to put on a request: the session's own choice when the model accepts it,
    /// otherwise the model's first advertised value. nothing at all when it advertises none.
    #[must_use]
    pub fn reasoning_effort(&self, model: &str, chosen: Option<&str>) -> Option<String> {
        // nothing chosen means the server's own default: imposing the first advertised
        // level would silently pick whatever the endpoint happens to list first.
        let chosen = chosen?;
        let levels = self.effort_levels(model);
        if levels.is_empty() || levels.iter().any(|level| level == chosen) {
            return Some(chosen.to_owned());
        }
        // a stored level the model no longer offers.
        levels.first().cloned()
    }

    /// changes a session's reasoning effort, recording it in the log.
    pub fn set_effort(&self, id: &str, effort: &str) -> Result<(), ManagerError> {
        let from = self.session_field(id, |row| row.effort.clone())?;
        self.store.set_session_effort(id, effort)?;
        self.append(
            id,
            kinds::MODEL_CHANGED,
            &json!({"from": from, "to": effort}),
        )?;
        Ok(())
    }

    /// changes which model a session runs on, recording it in the log so a replayed
    /// transcript says which model produced which message.
    pub fn set_model(&self, id: &str, model: &str) -> Result<(), ManagerError> {
        let from = self.session_field(id, |row| row.model.clone())?;
        self.store.set_session_model(id, model)?;
        self.append(
            id,
            kinds::MODEL_CHANGED,
            &json!({"from": from, "to": model}),
        )?;
        Ok(())
    }

    /// one field of a session's row, or an error naming the session that is not there.
    fn session_field<T>(
        &self,
        id: &str,
        field: impl FnOnce(&SessionRow) -> T,
    ) -> Result<T, ManagerError> {
        let row = self
            .store
            .get_session(id)?
            .ok_or_else(|| ManagerError::Rpc(not_found(id)))?;
        Ok(field(&row))
    }

    /// the model a session runs on, or the daemon's default when it has not chosen one.
    pub fn session_model(&self, id: &str) -> Result<String, ManagerError> {
        self.session_field(id, |row| row.model.clone())
            .map(|model| model.unwrap_or_else(|| self.default_model()))
    }

    pub fn list_sessions(
        &self,
        include_deleted: bool,
    ) -> Result<Vec<SessionSummary>, ManagerError> {
        let mut sessions = self.store.list_sessions(include_deleted)?;
        for session in &mut sessions {
            // the effort a turn will actually use, not a blank where none was stored.
            if session.effort.as_deref().unwrap_or_default().is_empty() {
                let model = session.model.clone().unwrap_or_default();
                session.effort = self.default_effort_for(&model);
            }
        }
        Ok(sessions)
    }

    /// repoints a session at another directory. tools and context follow it from the next
    /// turn, so this is the whole of "change where this tab works".
    pub fn set_cwd(&self, id: &str, cwd: &str) -> Result<String, ManagerError> {
        let canonical = std::fs::canonicalize(cwd).map_err(|err| {
            ManagerError::Rpc(RpcError::new(code::INVALID_PARAMS, format!("{cwd}: {err}")))
        })?;
        if !canonical.is_dir() {
            return Err(ManagerError::Rpc(RpcError::new(
                code::INVALID_PARAMS,
                format!("not a directory: {cwd}"),
            )));
        }
        let path = canonical.display().to_string();
        self.store.set_session_cwd(id, &path)?;
        self.append(id, kinds::SESSION_CWD, &json!({"cwd": path}))?;
        // the directory changed, so what applies to it changed too.
        let _ = self.announce_context(id, &path);
        Ok(path)
    }

    /// records which context files a working directory provides. this is the list the model
    /// is given, so it is read from the same place the turn reads it.
    fn announce_context(&self, session_id: &str, cwd: &str) -> Result<(), ManagerError> {
        let groups: Vec<serde_json::Value> =
            crate::workspace::context_listing(std::path::Path::new(cwd), None)
                .into_iter()
                .map(|group| {
                    let files: Vec<serde_json::Value> = group
                        .files
                        .iter()
                        .map(|file| {
                            json!({
                                "path": file.path,
                                "relative": file.relative,
                                "provider": file.provider,
                                "tokens": file.tokens,
                            })
                        })
                        .collect();
                    json!({"root": group.root, "files": files})
                })
                .collect();
        self.append(
            session_id,
            kinds::CONTEXT_LOADED,
            &json!({"cwd": cwd, "groups": groups}),
        )?;
        Ok(())
    }

    pub fn rename_session(&self, id: &str, title: &str) -> Result<(), ManagerError> {
        self.store.rename_session(id, title)?;
        self.append(id, kinds::SESSION_RENAMED, &json!({"title": title}))?;
        Ok(())
    }

    pub fn delete_session(&self, id: &str) -> Result<(), ManagerError> {
        if self.lock()?.turns.contains_key(id) {
            return Err(ManagerError::Rpc(RpcError::new(
                code::SESSION_BUSY,
                format!("session {id} has a turn in flight"),
            )));
        }
        self.store.set_session_state(id, SessionState::Deleted)?;
        self.append(
            id,
            kinds::SESSION_STATE,
            &json!({"state": "deleted", "reason": "session.delete"}),
        )?;
        self.announce_sessions();
        Ok(())
    }

    /// brings a deleted session back, so an old thread can be picked up again. restoring a
    /// session that is already live is not an error.
    pub fn restore_session(&self, id: &str) -> Result<(), ManagerError> {
        let Some(row) = self.store.get_session(id)? else {
            return Err(ManagerError::Rpc(not_found(id)));
        };
        if row.state != SessionState::Deleted {
            return Ok(());
        }
        self.store.set_session_state(id, SessionState::Active)?;
        self.append(
            id,
            kinds::SESSION_STATE,
            &json!({"state": "active", "reason": "session.restore"}),
        )?;
        self.announce_sessions();
        Ok(())
    }

    /// every checkpoint of the session's workspace, newest first.
    pub fn checkpoints(
        &self,
        id: &str,
    ) -> Result<Vec<crate::workspace::CheckpointEntry>, ManagerError> {
        let Some(row) = self.store.get_session(id)? else {
            return Err(ManagerError::Rpc(not_found(id)));
        };
        let workspace = std::path::PathBuf::from(&row.cwd);
        Ok(
            crate::workspace::list_checkpoints(&workspace, &self.config.checkpoint_dir)
                .unwrap_or_default(),
        )
    }

    /// restores the workspace to a checkpoint and records that it happened, so the transcript
    /// shows the rewind where it occurred rather than leaving the change unexplained.
    pub fn rewind(&self, id: &str, checkpoint: &str) -> Result<usize, ManagerError> {
        let Some(row) = self.store.get_session(id)? else {
            return Err(ManagerError::Rpc(not_found(id)));
        };
        let workspace = std::path::PathBuf::from(&row.cwd);
        let files = crate::workspace::rewind(&workspace, &self.config.checkpoint_dir, checkpoint)
            .map_err(|err| {
            ManagerError::Rpc(RpcError::new(code::INVALID_PARAMS, err.to_string()))
        })?;
        self.append(
            id,
            kinds::CHECKPOINT,
            &json!({"rewound_to": checkpoint, "files": files}),
        )?;
        Ok(files)
    }

    pub fn usage(&self, id: &str) -> Result<Usage, ManagerError> {
        let (events, _) = self.store.events_since(id, 0, 100_000)?;
        let mut usage = Usage {
            prompt_tokens: 0,
            completion_tokens: 0,
            cached_tokens: 0,
            // the endpoint prices nothing, so nothing is claimed: absent, not zero.
            cost_micros: None,
        };
        for event in events {
            if event.kind != kinds::LLM_COMPLETED {
                continue;
            }
            let Some(u) = event.payload.get("usage") else {
                continue;
            };
            usage.prompt_tokens += u.get("prompt_tokens").and_then(Value::as_u64).unwrap_or(0);
            usage.completion_tokens += u
                .get("completion_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            usage.cached_tokens += u.get("cached_tokens").and_then(Value::as_u64).unwrap_or(0);
        }
        Ok(usage)
    }

    // ---- attach / replay --------------------------------------------

    /// history for one session. no ownership: every client may read and write the same
    /// session at once, which is what lets several windows show the same tab.
    pub fn attach(
        &self,
        session_id: &str,
        from_seq: u64,
        limit: usize,
    ) -> Result<AttachResult, ManagerError> {
        let Some(row) = self.store.get_session(session_id)? else {
            return Err(ManagerError::Rpc(not_found(session_id)));
        };
        let (events, more) = self.store.events_since(session_id, from_seq, limit)?;
        let next_seq = events.last().map_or(from_seq, |e| e.seq);
        Ok(AttachResult {
            session: row.summary(),
            events,
            more,
            next_seq,
        })
    }

    pub fn replay(
        &self,
        session_id: &str,
        from_seq: u64,
        to_seq: Option<u64>,
        limit: usize,
    ) -> Result<ReplayResult, ManagerError> {
        let (events, more) = self
            .store
            .events_between(session_id, from_seq, to_seq, limit)?;
        let next_seq = events.last().map_or(from_seq, |e| e.seq);
        Ok(ReplayResult {
            events,
            more,
            next_seq,
        })
    }

    pub fn subscribe(
        &self,
        conn: u64,
        session_id: &str,
        exclude_kinds: Vec<String>,
    ) -> Result<u64, ManagerError> {
        let mut inner = self.lock()?;
        inner.next_sub += 1;
        let id = inner.next_sub;
        inner.subs.insert(
            id,
            Subscription {
                conn,
                session_id: session_id.to_owned(),
                exclude_kinds,
            },
        );
        Ok(id)
    }

    pub fn unsubscribe(&self, id: u64) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.subs.remove(&id);
        }
    }

    // ---- turns ------------------------------------------------------

    pub fn start_turn(
        self: &Arc<Self>,
        request: &NewTurn<'_>,
    ) -> Result<TurnStartResult, ManagerError> {
        let session_id = request.session_id;
        let prompt = request.prompt;
        let images = request.images;
        let detach = request.detach;
        let Some(row) = self.store.get_session(session_id)? else {
            return Err(ManagerError::Rpc(not_found(session_id)));
        };
        // `interrupted` is resumable by definition (spec 5.4): a turn died with the daemon
        // and the operator may simply carry on.
        if row.state == SessionState::Interrupted {
            self.store
                .set_session_state(session_id, SessionState::Active)?;
            self.append(
                session_id,
                kinds::SESSION_STATE,
                &json!({"state": "active", "reason": "turn resumed after interruption"}),
            )?;
        } else if !row.state.is_writable() {
            return Err(ManagerError::Rpc(RpcError::new(
                code::STALE_OWNER,
                format!(
                    "session {session_id} is {} and cannot take a turn",
                    row.state.as_str()
                ),
            )));
        }

        let turn_id = uuid::Uuid::new_v4().to_string();
        let cancel = Arc::new(AtomicBool::new(false));
        // the slot is taken under the same lock as the check: a check and an insert in two
        // lock windows let two turns both find the session idle and both start.
        {
            let mut inner = self.lock()?;
            if inner.turns.contains_key(session_id) {
                return Err(ManagerError::Rpc(RpcError::new(
                    code::SESSION_BUSY,
                    format!("session {session_id} already has a turn in flight"),
                )));
            }
            inner.turns.insert(
                session_id.to_owned(),
                RunningTurn {
                    turn_id: turn_id.clone(),
                    cancel: Arc::clone(&cancel),
                    since: crate::store::now_seconds(),
                },
            );
        }

        // the model and effort this turn runs at, decided once so the log records what
        // actually produced each message rather than what the session is set to now.
        let model = row.model.clone().unwrap_or_else(|| self.default_model());
        let effort = self.reasoning_effort(&model, row.effort.as_deref());
        let started = self.append(session_id, kinds::TURN_STARTED, &{
            // absent rather than null when they do not apply: a field that is not set
            // should not be in the log at all.
            let mut payload = json!({
                "turn_id": turn_id,
                "prompt": prompt,
                // kept in the log so the transcript can show the picture inline, and so
                // a replayed session reads the same as the live one.
                "images": images,
                "model": model,
                "effort": effort,
                "detach": detach,
            });
            if let Some(client) = request.client {
                payload["client"] = json!(client);
            }
            if let Some(resumed_from) = request.resumed_from {
                payload["resumed_from"] = json!(resumed_from);
            }
            if request.continue_final {
                payload["continue_final"] = json!(true);
            }
            payload
        });
        let seq = match started {
            Ok(seq) => seq,
            Err(err) => {
                // the slot was taken before the log had the turn in it: give it back.
                if let Ok(mut inner) = self.inner.lock() {
                    inner.turns.remove(session_id);
                }
                return Err(ManagerError::Store(err));
            }
        };

        let manager = Arc::clone(self);
        let session = session_id.to_owned();
        let prompt = prompt.to_owned();
        let images = images.to_vec();
        let turn = turn_id.clone();
        // copied out of the request: the task outlives the borrow it arrived on.
        let continue_final = request.continue_final;
        tokio::spawn(async move {
            manager
                .run_turn(
                    session,
                    turn,
                    prompt,
                    images,
                    row,
                    model,
                    effort,
                    cancel,
                    detach,
                    continue_final,
                )
                .await;
        });

        Ok(TurnStartResult { turn_id, seq })
    }

    // the turn's inputs are independent; bundling them would only move the list.
    #[allow(clippy::too_many_arguments)]
    async fn run_turn(
        self: Arc<Self>,
        session_id: String,
        turn_id: String,
        prompt: String,
        images: Vec<sa_proto::session::ImageAttachment>,
        row: SessionRow,
        model: String,
        effort: Option<String>,
        cancel: Arc<AtomicBool>,
        detach: bool,
        continue_final: bool,
    ) {
        let budget = std::time::Duration::from_secs(self.config.aux_budget_secs.max(1));

        // the workspace lock serialises sessions that share a repository (spec 7.4).
        let workspace = PathBuf::from(&row.cwd);
        let _lock = match crate::workspace::lock(&workspace, &self.config.lock_dir) {
            Ok(guard) => Some(guard),
            Err(err) => {
                tracing::warn!(%err, "running without the workspace lock");
                None
            }
        };

        // an unattended turn never starts without a checkpoint (spec 8.4).
        if detach {
            let seq = self.store.last_seq(&session_id).unwrap_or(0);
            match crate::workspace::checkpoint(
                &workspace,
                &self.config.checkpoint_dir,
                &session_id,
                seq,
            ) {
                Ok(checkpoint) => {
                    let _ = self.append(
                        &session_id,
                        kinds::CHECKPOINT,
                        &crate::workspace::checkpoint_payload(&checkpoint),
                    );
                    let _ = crate::workspace::prune_checkpoints(
                        &workspace,
                        &self.config.checkpoint_dir,
                        &session_id,
                        10,
                    );
                }
                // a non-git workspace cannot be checkpointed; that is recorded, not fatal.
                Err(crate::workspace::WorkspaceError::NotARepository(_)) => {
                    let _ = self.append(
                        &session_id,
                        kinds::SESSION_STATE,
                        &json!({
                            "state": "active",
                            "reason": "checkpoint_unavailable: workspace is not a git repository",
                        }),
                    );
                }
                Err(err) => {
                    tracing::warn!(%err, "checkpoint failed");
                    let _ = self.append(
                        &session_id,
                        kinds::TURN_INTERRUPTED,
                        &json!({
                            "turn_id": turn_id,
                            "cause": format!("checkpoint failed: {err}"),
                        }),
                    );
                    if let Ok(mut inner) = self.inner.lock() {
                        inner.turns.remove(&session_id);
                    }
                    self.continue_with_follow_up(&session_id, detach, false);
                    return;
                }
            }
        }

        let sink = Arc::new(TurnSink {
            manager: Arc::clone(&self),
            session_id: session_id.clone(),
            cancel: Arc::clone(&cancel),
        });
        let tools: Arc<dyn ToolExecutor> = Arc::new(ToolRegistry::with_hubs(
            PathBuf::from(&row.cwd),
            Arc::clone(&self.config.mcp),
            Arc::clone(&self.config.lsp),
        ));

        // compaction is checked before the turn, and only ever appends (spec 10.4).
        match tokio::time::timeout(budget, self.maybe_compact(&session_id, &model)).await {
            Ok(Err(err)) => tracing::warn!(%err, "compaction skipped"),
            Err(_) => tracing::warn!(%session_id, "compaction timed out"),
            Ok(Ok(_)) => {}
        }

        let agent = Agent::new(Arc::clone(&self.config.provider), tools, sink);
        let all = self
            .store
            .events_since(&session_id, 0, 100_000)
            .map(|(events, _)| events)
            .unwrap_or_default();
        let compaction = all
            .iter()
            .rev()
            .find(|e| e.kind == kinds::COMPACTION_APPLIED)
            .map(|e| {
                (
                    e.payload.get("to_seq").and_then(Value::as_u64).unwrap_or(0),
                    e.payload
                        .get("summary")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                )
            });
        let (history, compaction_summary) = match compaction {
            Some((to_seq, summary)) => (
                all.iter().filter(|e| e.seq > to_seq).cloned().collect(),
                Some(summary),
            ),
            None => (all, None),
        };

        let request = TurnRequest {
            turn_id: turn_id.clone(),
            prompt,
            model: model.clone(),
            workspace: workspace.display().to_string(),
            history,
            compaction_summary,
            // the daemon's own cwd belongs to no session, so only the workspace and its
            // deployed config dir are searched.
            context: crate::workspace::context(&workspace, None),
            images,
            max_tokens: self.config.max_tokens,
            // the value recorded with the turn, so the log and the request agree.
            reasoning_effort: effort,
            continue_final,
        };

        // a stalled model stream must not hang the session forever (spec 14, v1.1).
        let Ok(outcome) = tokio::time::timeout(
            std::time::Duration::from_secs(self.config.turn_timeout_secs.max(1)),
            agent.run_turn(request),
        )
        .await
        else {
            let _ = self.append(
                &session_id,
                kinds::TURN_INTERRUPTED,
                &json!({
                    "turn_id": turn_id,
                    "cause": format!("timeout after {}s", self.config.turn_timeout_secs),
                }),
            );
            self.continue_with_follow_up(&session_id, detach, false);
            return;
        };

        let completed = match outcome {
            Ok(outcome) => {
                let _ = self.append(
                    &session_id,
                    kinds::TURN_COMPLETED,
                    &json!({
                        "turn_id": turn_id,
                        "finish_reason": outcome.finish_reason,
                        "usage": {
                            "prompt_tokens": outcome.usage.prompt_tokens,
                            "completion_tokens": outcome.usage.completion_tokens,
                            "cached_tokens": outcome.usage.cached_tokens,
                        },
                        // what the model's view costs now, against the window it has. the
                        // figure is the prompt the endpoint counted, not an estimate.
                        "context": {
                            "tokens": self.context_used(&session_id),
                            "limit": self.context_limit(&model),
                        },
                    }),
                );
                true
            }
            Err(err) => {
                let _ = self.append(
                    &session_id,
                    kinds::TURN_INTERRUPTED,
                    &json!({"turn_id": turn_id, "cause": err.to_string()}),
                );
                false
            }
        };

        // a queued follow-up continues the session as its next turn (spec 3.7).
        self.continue_with_follow_up(&session_id, detach, completed);
    }

    /// hands a message to the turn that is already running, rather than refusing it.
    pub fn steer(&self, session_id: &str, prompt: &str) -> Result<(), ManagerError> {
        self.enqueue(session_id, prompt, false)
    }

    /// queues a message for after the running turn finishes (spec 3.7).
    pub fn follow_up(&self, session_id: &str, prompt: &str) -> Result<(), ManagerError> {
        self.enqueue(session_id, prompt, true)
    }

    fn enqueue(&self, session_id: &str, prompt: &str, follow_up: bool) -> Result<(), ManagerError> {
        let state = {
            let mut inner = self.lock()?;
            if !inner.turns.contains_key(session_id) {
                return Err(ManagerError::Rpc(RpcError::new(
                    code::SESSION_NOT_FOUND,
                    format!("session {session_id} has no turn to steer"),
                )));
            }
            let queue = inner.queues.entry(session_id.to_owned()).or_default();
            if follow_up {
                queue.follow_up.push(prompt.to_owned());
            } else {
                queue.steering.push(prompt.to_owned());
            }
            queue.state()
        };
        self.append(session_id, kinds::TURN_QUEUED, &json!(state))?;
        Ok(())
    }

    /// takes back everything queued, so a client can put it back in the editor (spec 3.7).
    pub fn dequeue(&self, session_id: &str) -> Result<sa_proto::session::QueueState, ManagerError> {
        let state = {
            let mut inner = self.lock()?;
            inner
                .queues
                .remove(session_id)
                .map_or_else(sa_proto::session::QueueState::default, |queue| {
                    queue.state()
                })
        };
        self.append(session_id, kinds::TURN_QUEUED, &json!(state))?;
        Ok(state)
    }

    /// takes one queued message back out, so a client can edit it or drop it (spec 3.7).
    ///
    /// `queue` names which of the two the index refers to. an index past the end is not an
    /// error: a message may have been delivered between the client drawing it and the
    /// click landing, and the caller only wanted it gone.
    pub fn drop_queued(
        &self,
        session_id: &str,
        queue: &str,
        index: usize,
    ) -> Result<sa_proto::session::QueueState, ManagerError> {
        // the name is checked before anything is looked up: a wrong one is a client bug,
        // whether or not this session happens to have anything queued.
        if !matches!(queue, "steering" | "follow_up") {
            return Err(ManagerError::Rpc(RpcError::new(
                code::INVALID_PARAMS,
                format!("unknown queue: {queue}"),
            )));
        }
        let state = {
            let mut inner = self.lock()?;
            let Some(queued) = inner.queues.get_mut(session_id) else {
                return Ok(sa_proto::session::QueueState::default());
            };
            let list = if queue == "steering" {
                &mut queued.steering
            } else {
                &mut queued.follow_up
            };
            if index < list.len() {
                list.remove(index);
            }
            queued.state()
        };
        self.append(session_id, kinds::TURN_QUEUED, &json!(state))?;
        Ok(state)
    }

    /// one steering message, so a redirect lands between round-trips rather than as a batch.
    fn take_steering(&self, session_id: &str) -> Option<String> {
        let (prompt, state) = {
            let mut inner = self.lock().ok()?;
            let queue = inner.queues.get_mut(session_id)?;
            if queue.steering.is_empty() {
                return None;
            }
            let prompt = queue.steering.remove(0);
            (prompt, queue.state())
        };
        // the delivery is the record: a replay puts the message in the transcript at the
        // point the model actually saw it, not at the point it was typed.
        let _ = self.append(session_id, kinds::TURN_STEERED, &json!({"prompt": prompt}));
        let _ = self.append(session_id, kinds::TURN_QUEUED, &json!(state));
        Some(prompt)
    }

    /// removes the finished turn and hands back the next queued message (spec 3.7).
    fn settle_turn(&self, session_id: &str) -> Option<String> {
        let (next, state) = {
            let mut inner = self.lock().ok()?;
            inner.turns.remove(session_id);
            let mut next = None;
            let (state, empty) = if let Some(queue) = inner.queues.get_mut(session_id) {
                // anything still queued when the turn ends has not reached the model yet. it
                // becomes the next turn rather than sitting there forever: steering first,
                // because it was typed first.
                if !queue.steering.is_empty() {
                    next = Some(queue.steering.remove(0));
                } else if !queue.follow_up.is_empty() {
                    next = Some(queue.follow_up.remove(0));
                }
                let state = queue.state();
                let empty = queue.steering.is_empty() && queue.follow_up.is_empty();
                (Some(state), empty)
            } else {
                (None, false)
            };
            if empty {
                inner.queues.remove(session_id);
            }
            (next, state)
        };
        // taking the message out is a queue change like any other, and the view only learns
        // about the queue from this event: without it a delivered message stays on screen as
        // though it were still waiting.
        if let Some(state) = state {
            let _ = self.append(session_id, kinds::TURN_QUEUED, &json!(state));
        }
        next
    }

    /// ends the turn, and runs whatever is still queued as the session's next one.
    ///
    /// `completed` says whether the turn actually finished. a cancel, a timeout or a failed
    /// checkpoint is not a reason to press on with a goal, so a nudge is only queued after a
    /// turn that ran to its own end.
    fn continue_with_follow_up(self: &Arc<Self>, session_id: &str, detach: bool, completed: bool) {
        if completed {
            if let Some(nudge) = self.nudge_goal(session_id) {
                let _ = self.enqueue(session_id, &nudge, true);
            }
        }
        if let Some(prompt) = self.settle_turn(session_id) {
            let turn = NewTurn {
                detach,
                ..NewTurn::new(session_id, &prompt)
            };
            if let Err(err) = self.start_turn(&turn) {
                tracing::warn!(%err, %session_id, "queued message could not start a turn");
            }
        }
    }

    pub fn cancel_turn(&self, session_id: &str) -> Result<(), ManagerError> {
        let inner = self.lock()?;
        let Some(turn) = inner.turns.get(session_id) else {
            return Ok(());
        };
        turn.cancel.store(true, Ordering::SeqCst);
        Ok(())
    }

    pub fn turn_status(&self, session_id: &str) -> TurnStatus {
        let running = self.lock().ok().and_then(|inner| {
            inner
                .turns
                .get(session_id)
                .map(|turn| (turn.turn_id.clone(), turn.since))
        });
        TurnStatus {
            state: if running.is_some() { "running" } else { "idle" }.to_owned(),
            since: running
                .as_ref()
                .map_or_else(crate::store::now_seconds, |(_, since)| *since),
            turn_id: running.map(|(turn_id, _)| turn_id),
        }
    }

    // ---- goals ------------------------------------------------------

    /// a goal that has not converged in this many nudges is not going to. the loop stops and
    /// says so rather than spending the rest of the session circling the same wall.
    pub const GOAL_MAX_NUDGES: u32 = 20;

    /// the goal loop's state, derived from the log rather than held beside it: the last thing
    /// that was asked for, and whatever ended it afterwards.
    pub fn goal_status(&self, session_id: &str) -> Result<GoalStatus, ManagerError> {
        let Some(set) = self
            .store
            .last_event_of_kind(session_id, &[kinds::GOAL_SET])?
        else {
            return Ok(GoalStatus::default());
        };
        let done = self
            .store
            .last_event_of_kind(session_id, &[kinds::GOAL_DONE])?;
        let stopped = self
            .store
            .last_event_of_kind(session_id, &[kinds::GOAL_STOPPED])?;
        let nudged = self
            .store
            .last_event_of_kind(session_id, &[kinds::GOAL_NUDGED])?;

        // only an event newer than the goal itself belongs to it; one from a goal that was
        // already closed is history, not state.
        let after = |event: &Option<Event>| event.as_ref().is_some_and(|event| event.seq > set.seq);
        let mut status = GoalStatus {
            active: !after(&done) && !after(&stopped),
            text: set.payload["text"].as_str().unwrap_or_default().to_owned(),
            max_iterations: u32::try_from(
                set.payload["max_iterations"]
                    .as_u64()
                    .unwrap_or_else(|| u64::from(Self::GOAL_MAX_NUDGES)),
            )
            .unwrap_or(Self::GOAL_MAX_NUDGES),
            ..GoalStatus::default()
        };
        if let Some(nudge) = nudged.as_ref().filter(|event| event.seq > set.seq) {
            let spent = nudge.payload["iteration"].as_u64().unwrap_or_default();
            status.iterations = u32::try_from(spent).unwrap_or(u32::MAX);
        }
        if let Some(done) = done.as_ref().filter(|event| event.seq > set.seq) {
            status.summary = done.payload["summary"].as_str().map(str::to_owned);
        }
        Ok(status)
    }

    /// asks a session to keep working until `goal_done`. nothing is started here: the caller
    /// opens the turn, and the loop only ever continues a turn that ended on its own.
    pub fn set_goal(
        &self,
        session_id: &str,
        text: &str,
        max_iterations: u32,
    ) -> Result<GoalStatus, ManagerError> {
        let text = text.trim();
        if text.is_empty() {
            return Err(ManagerError::Rpc(RpcError::new(
                code::INVALID_PARAMS,
                "a goal needs text",
            )));
        }
        self.append(
            session_id,
            kinds::GOAL_SET,
            &json!({"text": text, "max_iterations": max_iterations.max(1)}),
        )?;
        self.goal_status(session_id)
    }

    /// closes the loop. the reason is recorded so the log says who ended it.
    pub fn stop_goal(&self, session_id: &str, reason: &str) -> Result<GoalStatus, ManagerError> {
        self.append(session_id, kinds::GOAL_STOPPED, &json!({"reason": reason}))?;
        self.goal_status(session_id)
    }

    /// queues the next nudge for an open goal and hands back its text. the count comes from
    /// the log, so a nudge that was already spent is never spent twice, even across a restart.
    fn nudge_goal(&self, session_id: &str) -> Option<String> {
        let status = self.goal_status(session_id).ok()?;
        if !status.active {
            return None;
        }
        if status.iterations >= status.max_iterations {
            let _ = self.append(
                session_id,
                kinds::GOAL_STOPPED,
                &json!({"reason": "exhausted"}),
            );
            return None;
        }
        let iteration = status.iterations.saturating_add(1);
        self.append(
            session_id,
            kinds::GOAL_NUDGED,
            &json!({"iteration": iteration, "max_iterations": status.max_iterations}),
        )
        .ok()?;
        Some(nudge_text(&status.text, iteration, status.max_iterations))
    }

    // ---- compaction --------------------------------------------------

    /// the size of the model's view as the provider last reported it. this is the prompt
    /// the endpoint actually counted, not an estimate of the log's bytes.
    #[must_use]
    pub fn context_used(&self, session_id: &str) -> u64 {
        self.store
            .last_event_of_kind(session_id, &[kinds::LLM_COMPLETED])
            .ok()
            .flatten()
            .and_then(|event| {
                event
                    .payload
                    .get("usage")
                    .and_then(|usage| usage.get("prompt_tokens"))
                    .and_then(Value::as_u64)
            })
            .unwrap_or_default()
    }

    /// estimates the token cost of the log, used only where a provider figure does not
    /// exist yet.
    #[must_use]
    pub fn estimate_tokens(&self, session_id: &str) -> u64 {
        let Ok((events, _)) = self.store.events_since(session_id, 0, 100_000) else {
            return 0;
        };
        let mut tokens = 0_u64;
        for event in &events {
            // attached pictures live in the log so the transcript can draw them, but the
            // model's view carries only the prompt: counting base64 would show a few
            // screenshots as most of the window.
            let mut payload = event.payload.clone();
            if let Value::Object(map) = &mut payload {
                if map.get("images").is_some_and(Value::is_array) {
                    map.insert("images".to_owned(), Value::Array(Vec::new()));
                }
            }
            tokens += crate::tokens::estimate_rounded(&payload.to_string());
        }
        tokens
    }

    /// summarises the oldest turns when the session approaches the context limit (spec 10.4).
    ///
    /// the log keeps every original event; only the model's view changes.
    pub async fn maybe_compact(
        &self,
        session_id: &str,
        model: &str,
    ) -> Result<Option<String>, ManagerError> {
        // without an advertised window there is nothing honest to compare against, so
        // compaction waits rather than firing on a guess.
        let Some(limit) = self.context_limit(model) else {
            return Ok(None);
        };
        let threshold = (limit as f64 * COMPACTION_TRIGGER) as u64;
        let before = self
            .context_used(session_id)
            .max(self.estimate_tokens(session_id));
        if before < threshold {
            return Ok(None);
        }

        let (events, _) = self.store.events_since(session_id, 0, 100_000)?;
        let starts: Vec<u64> = events
            .iter()
            .filter(|e| e.kind == kinds::TURN_STARTED)
            .map(|e| e.seq)
            .collect();
        if starts.len() <= COMPACTION_RETAIN_TURNS {
            return Ok(None);
        }
        let cutoff = starts[starts.len() - COMPACTION_RETAIN_TURNS];
        let to_seq = cutoff.saturating_sub(1);
        if to_seq == 0 {
            return Ok(None);
        }

        let mut transcript = String::new();
        for event in events.iter().filter(|e| e.seq <= to_seq) {
            match event.kind.as_str() {
                kinds::TURN_STARTED => {
                    if let Some(prompt) = event.payload.get("prompt").and_then(Value::as_str) {
                        let _ = write!(transcript, "\nuser: {prompt}\n\n");
                    }
                }
                kinds::LLM_COMPLETED => {
                    if let Some(text) = event.payload.get("text").and_then(Value::as_str) {
                        let _ = writeln!(transcript, "assistant: {text}");
                    }
                }
                _ => {}
            }
        }

        let mut rx = self
            .config
            .provider
            .stream(crate::llm::ChatRequest {
                model: model.to_owned(),
                system: None,
                messages: vec![crate::llm::message::user(format!(
                    "Summarise the following work log in at most 400 words. Keep decisions, \
                     file paths and open questions. Do not add commentary.\n{transcript}"
                ))],
                tools: Vec::new(),
                max_tokens: Some(1024),
                // summarising is reading, not solving: the least effort the model offers.
                reasoning_effort: self.cheapest_effort(model),
                continue_final: false,
            })
            .await?;

        let mut summary = String::new();
        let mut usage = Usage {
            prompt_tokens: 0,
            completion_tokens: 0,
            cached_tokens: 0,
            cost_micros: None,
        };
        while let Some(delta) = rx.recv().await {
            match delta {
                crate::llm::Delta::Text(chunk) => summary.push_str(&chunk),
                crate::llm::Delta::Done {
                    prompt_tokens,
                    completion_tokens,
                    cached_tokens,
                    ..
                } => {
                    usage.prompt_tokens = prompt_tokens;
                    usage.completion_tokens = completion_tokens;
                    usage.cached_tokens = cached_tokens;
                }
                _ => {}
            }
        }
        if summary.trim().is_empty() {
            return Ok(None);
        }

        self.append(
            session_id,
            kinds::COMPACTION_APPLIED,
            &json!({
                "from_seq": 1,
                "to_seq": to_seq,
                "summary": summary,
                "tokens_before": before,
                "tokens_after": summary.len() as u64 / CHARS_PER_TOKEN,
                "trigger": "auto",
                "usage": usage,
            }),
        )?;

        Ok(Some(summary))
    }

    /// re-drives the most recent interrupted turn (spec 5.6).
    pub fn resume_turn(
        self: &Arc<Self>,
        session_id: &str,
    ) -> Result<TurnStartResult, ManagerError> {
        let last = self.store.last_event_of_kind(
            session_id,
            &[
                kinds::TURN_INTERRUPTED,
                kinds::TURN_COMPLETED,
                kinds::TURN_STARTED,
            ],
        )?;
        let Some(last) = last else {
            return Err(ManagerError::Rpc(RpcError::new(
                code::INVALID_PARAMS,
                format!("session {session_id} has no turn to resume"),
            )));
        };
        if last.kind != kinds::TURN_INTERRUPTED {
            return Err(ManagerError::Rpc(RpcError::new(
                code::INVALID_PARAMS,
                format!("session {session_id} has no interrupted turn"),
            )));
        }
        let prompt = last
            .payload
            .get("prompt")
            .and_then(Value::as_str)
            .unwrap_or("continue")
            .to_owned();
        let resumed_from = last
            .payload
            .get("turn_id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();

        self.store
            .set_session_state(session_id, SessionState::Active)?;
        // an interrupted turn is redriven by the daemon, not by the client that was watching
        // it, so the log records where it resumed from and no client.
        self.start_turn(&NewTurn {
            detach: true,
            resumed_from: Some(&resumed_from),
            ..NewTurn::new(session_id, &prompt)
        })
    }

    // ---- recovery ----------------------------------------------------

    /// appends a terminal event for every turn that died with the daemon (spec 5.6).
    pub fn recover(&self) -> Result<usize, ManagerError> {
        let open = self.store.open_turns()?;
        let count = open.len();
        for turn in open {
            for call_id in &turn.pending_calls {
                self.append(
                    &turn.session_id,
                    kinds::TOOL_FAILED,
                    &json!({
                        "turn_id": turn.turn_id,
                        "call_id": call_id,
                        "error": "daemon restarted mid-call",
                        "class": "interrupted",
                    }),
                )?;
            }
            self.append(
                &turn.session_id,
                kinds::TURN_INTERRUPTED,
                &json!({
                    "turn_id": turn.turn_id,
                    "cause": "daemon_restart",
                    "pending_call_id": turn.pending_calls.first(),
                }),
            )?;
            self.store
                .set_session_state(&turn.session_id, SessionState::Interrupted)?;
        }
        Ok(count)
    }
}

struct TurnSink {
    manager: Arc<Manager>,
    session_id: String,
    cancel: Arc<AtomicBool>,
}

impl EventSink for TurnSink {
    fn append(&self, kind: &str, payload: &Value) -> Result<u64, StoreError> {
        self.manager.append(&self.session_id, kind, payload)
    }

    fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }

    fn take_steering(&self) -> Option<String> {
        self.manager.take_steering(&self.session_id)
    }
}

/// what the loop says when the agent has stopped without finishing. the first nudge is plain;
/// later ones carry the count, name the failure mode, and say when the loop gives up.
fn nudge_text(goal: &str, iteration: u32, max: u32) -> String {
    let mut out = String::new();
    if iteration > 1 {
        let _ = write!(out, "Nudge {iteration} of {max}. ");
    }
    let _ = write!(out, "Keep working on the goal: {goal}\n\n");
    out.push_str(
        "Do not report exploratory progress (\"investigated\", \"explored as far as possible\") \
         as an outcome. Either keep working toward the goal, or -- when it is genuinely solved \
         -- call the goal_done tool with a one-line summary of the result. If a blocker makes \
         the goal impossible, call goal_done and say exactly what blocks it.\n",
    );
    if iteration > 1 {
        out.push_str(
            "If the same approach has already failed, change it rather than repeating it.\n",
        );
    }
    if max.saturating_sub(iteration) <= 2 {
        let _ = writeln!(
            out,
            "The loop stops after {max} nudges; this is one of the last."
        );
    }
    out
}

/// the last component of a working directory: what a session is called when nothing has
/// named it, and what its tab shows.
fn default_title(cwd: &std::path::Path) -> String {
    cwd.file_name().map_or_else(
        || "session".to_owned(),
        |n| n.to_string_lossy().into_owned(),
    )
}
