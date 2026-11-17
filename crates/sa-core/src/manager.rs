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
    pub reasoning_effort: Option<String>,
    /// an operator override. absent means the endpoint's advertised window is used, and if
    /// that is absent too the limit is genuinely unknown rather than guessed.
    pub context_limit: Option<u64>,
    pub lock_dir: PathBuf,
    pub turn_timeout_secs: u64,
    /// how long an auxiliary model call (title, compaction) may take before it is abandoned.
    /// without this a stalled provider blocks the turn before the agent even starts.
    pub aux_budget_secs: u64,
    /// reasoning effort applied to new sessions when none is chosen.
    pub default_effort: String,
    /// what the endpoint reports it serves. shared so a background refresher can fill it in.
    pub models: Arc<Mutex<Vec<crate::llm::ModelInfo>>>,
}

struct Subscription {
    conn: u64,
    session_id: String,
    exclude_kinds: Vec<String>,
}

struct RunningTurn {
    turn_id: String,
    cancel: Arc<AtomicBool>,
    /// messages typed while the turn runs, drained by the agent between iterations.
    steering: Arc<std::sync::Mutex<Vec<String>>>,
}

#[derive(Default)]
struct Inner {
    next_conn: u64,
    next_sub: u64,
    conns: HashMap<u64, mpsc::Sender<Value>>,
    subs: HashMap<u64, Subscription>,
    turns: HashMap<String, RunningTurn>,
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

    fn notify_event(&self, session_id: &str, event: &Event) {
        let Ok(inner) = self.inner.lock() else { return };
        for sub in inner.subs.values() {
            if sub.session_id != session_id {
                continue;
            }
            if sub.exclude_kinds.iter().any(|k| k == &event.kind) {
                continue;
            }
            let payload = json!({
                "session_id": event.session_id,
                "seq": event.seq,
                "kind": event.kind,
                "payload": event.payload,
            });
            Self::push(
                &inner,
                sub.conn,
                json!({"jsonrpc": "2.0", "method": "event", "params": payload}),
            );
        }
    }

    // ---- events -----------------------------------------------------

    pub fn append(&self, session_id: &str, kind: &str, payload: &Value) -> Result<u64, StoreError> {
        let seq = self.store.append_event(session_id, kind, payload)?;
        if let Ok((events, _)) = self
            .store
            .events_since(session_id, seq.saturating_sub(1), 1)
        {
            if let Some(event) = events.first() {
                self.notify_event(session_id, event);
            }
        }
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
            effort: (!self.config.default_effort.is_empty())
                .then(|| self.config.default_effort.clone()),
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
        self.models()
            .first()
            .map(|model| model.id.clone())
            .unwrap_or_default()
    }

    /// the context window for a model: an explicit override wins, then what the endpoint
    /// advertised. `None` means unknown, and nothing downstream pretends otherwise.
    #[must_use]
    pub fn context_limit(&self, model: &str) -> Option<u64> {
        if let Some(limit) = self.config.context_limit {
            return Some(limit);
        }
        self.models()
            .iter()
            .find(|known| known.id == model)
            .and_then(|known| known.context_limit)
    }

    /// the reasoning effort levels this build offers.
    #[must_use]
    pub fn effort_levels() -> Vec<&'static str> {
        vec!["low", "medium", "high"]
    }

    /// changes a session's reasoning effort, recording it in the log.
    pub fn set_effort(&self, id: &str, effort: &str) -> Result<(), ManagerError> {
        self.store.set_session_effort(id, effort)?;
        self.append(
            id,
            kinds::MODEL_CHANGED,
            &json!({"field": "effort", "to": effort}),
        )?;
        Ok(())
    }

    pub fn list_sessions(
        &self,
        include_deleted: bool,
    ) -> Result<Vec<SessionSummary>, ManagerError> {
        Ok(self.store.list_sessions(include_deleted)?)
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
        Ok(path)
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

    pub fn usage(&self, id: &str) -> Result<Usage, ManagerError> {
        let (events, _) = self.store.events_since(id, 0, 100_000)?;
        let mut usage = Usage {
            prompt_tokens: 0,
            completion_tokens: 0,
            cost_micros: Some(0),
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
        limit: usize,
    ) -> Result<ReplayResult, ManagerError> {
        let (events, more) = self.store.events_since(session_id, from_seq, limit)?;
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
        session_id: &str,
        prompt: &str,
        images: &[sa_proto::session::ImageAttachment],
        detach: bool,
        attached_client: Option<&str>,
    ) -> Result<TurnStartResult, ManagerError> {
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
        {
            let inner = self.lock()?;
            if inner.turns.contains_key(session_id) {
                return Err(ManagerError::Rpc(RpcError::new(
                    code::SESSION_BUSY,
                    format!("session {session_id} already has a turn in flight"),
                )));
            }
        }

        let turn_id = uuid::Uuid::new_v4().to_string();
        let cancel = Arc::new(AtomicBool::new(false));
        let seq = self.append(
            session_id,
            kinds::TURN_STARTED,
            &json!({
                "turn_id": turn_id,
                "prompt": prompt,
                // kept in the log so the transcript can show the picture inline, and so a
                // replayed session reads the same as the live one.
                "images": images,
                "model": self.default_model(),
                "detach": detach,
                "client": attached_client,
            }),
        )?;

        let steering = Arc::new(std::sync::Mutex::new(Vec::new()));
        self.lock()?.turns.insert(
            session_id.to_owned(),
            RunningTurn {
                turn_id: turn_id.clone(),
                cancel: Arc::clone(&cancel),
                steering: Arc::clone(&steering),
            },
        );

        let manager = Arc::clone(self);
        let session = session_id.to_owned();
        let prompt = prompt.to_owned();
        let images = images.to_vec();
        let turn = turn_id.clone();
        tokio::spawn(async move {
            manager
                .run_turn(session, turn, prompt, images, row, cancel, steering, detach)
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
        cancel: Arc<AtomicBool>,
        steering: Arc<std::sync::Mutex<Vec<String>>>,
        detach: bool,
    ) {
        // highest priority: name an untitled session before any work starts, but never let
        // it hold the turn: a stalled provider would otherwise block everything.
        let title_model = row.model.clone().unwrap_or_else(|| self.default_model());
        let budget = std::time::Duration::from_secs(self.config.aux_budget_secs.max(1));
        if tokio::time::timeout(
            budget,
            self.name_session(&session_id, &prompt, &title_model),
        )
        .await
        .is_err()
        {
            tracing::warn!(%session_id, "title generation timed out");
        }

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
            match crate::workspace::checkpoint(&workspace, &session_id, seq) {
                Ok(checkpoint) => {
                    let _ = self.append(
                        &session_id,
                        kinds::CHECKPOINT,
                        &crate::workspace::checkpoint_payload(&checkpoint),
                    );
                    let _ = crate::workspace::prune_checkpoints(&workspace, &session_id, 10);
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
                    return;
                }
            }
        }

        let sink = Arc::new(TurnSink {
            manager: Arc::clone(&self),
            session_id: session_id.clone(),
            cancel: Arc::clone(&cancel),
            steering,
        });
        let tools: Arc<dyn ToolExecutor> = Arc::new(ToolRegistry::new(PathBuf::from(&row.cwd)));

        let model = row.model.clone().unwrap_or_else(|| self.default_model());

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
            reasoning_effort: row
                .effort
                .clone()
                .or_else(|| self.config.reasoning_effort.clone()),
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
            if let Ok(mut inner) = self.inner.lock() {
                inner.turns.remove(&session_id);
            }
            return;
        };

        match outcome {
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
                        },
                        // what the model's view costs now, against the window it has. the
                        // figure is the prompt the endpoint counted, not an estimate.
                        "context": {
                            "tokens": self.context_used(&session_id),
                            "limit": self.context_limit(&model),
                        },
                    }),
                );
            }
            Err(err) => {
                let _ = self.append(
                    &session_id,
                    kinds::TURN_INTERRUPTED,
                    &json!({"turn_id": turn_id, "cause": err.to_string()}),
                );
            }
        }

        if let Ok(mut inner) = self.inner.lock() {
            inner.turns.remove(&session_id);
        }

        // retitle inline so the operator watches it happen and sees the result.
        let model = self.default_model();
        self.maybe_retitle(&session_id, model).await;
    }

    /// hands a message to the turn that is already running, rather than refusing it.
    pub fn steer(&self, session_id: &str, prompt: &str) -> Result<(), ManagerError> {
        let inner = self.lock()?;
        let Some(turn) = inner.turns.get(session_id) else {
            return Err(ManagerError::Rpc(RpcError::new(
                code::SESSION_NOT_FOUND,
                format!("session {session_id} has no turn to steer"),
            )));
        };
        if let Ok(mut queue) = turn.steering.lock() {
            queue.push(prompt.to_owned());
        }
        drop(inner);
        self.append(session_id, kinds::TURN_STEERED, &json!({"prompt": prompt}))?;
        Ok(())
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
        let turn_id = self
            .lock()
            .ok()
            .and_then(|inner| inner.turns.get(session_id).map(|t| t.turn_id.clone()));
        TurnStatus {
            state: if turn_id.is_some() { "running" } else { "idle" }.to_owned(),
            since: crate::store::now_seconds(),
            turn_id,
        }
    }

    /// the first thing a turn does: name an untitled session from the user's prompt, so
    /// the tab reads like the work immediately rather than a session id.
    async fn name_session(&self, session_id: &str, prompt: &str, model: &str) {
        let Ok((events, _)) = self.store.events_since(session_id, 0, 100_000) else {
            return;
        };
        let already_named = events.iter().any(|e| {
            e.kind == kinds::SESSION_RENAMED
                || (e.kind == kinds::SESSION_CREATED
                    && e.payload.get("titled").and_then(Value::as_bool) == Some(true))
        });
        if already_named {
            return;
        }
        self.request_title(session_id, prompt, model, prompt).await;
    }

    /// refreshes the title from the conversation every fourth turn.
    async fn maybe_retitle(&self, session_id: &str, model: String) {
        let Ok((events, _)) = self.store.events_since(session_id, 0, 100_000) else {
            return;
        };
        let turns = events
            .iter()
            .filter(|e| e.kind == kinds::TURN_COMPLETED)
            .count();
        if turns == 0 || turns % 4 != 0 {
            return;
        }

        let mut digest = String::new();
        for event in &events {
            match event.kind.as_str() {
                kinds::TURN_STARTED => {
                    if let Some(prompt) = event.payload.get("prompt").and_then(Value::as_str) {
                        let _ = writeln!(
                            &mut digest,
                            "user: {}",
                            prompt.chars().take(200).collect::<String>()
                        );
                    }
                }
                kinds::LLM_COMPLETED => {
                    if let Some(text) = event.payload.get("text").and_then(Value::as_str) {
                        if !text.is_empty() {
                            let _ = writeln!(
                                &mut digest,
                                "assistant: {}",
                                text.chars().take(200).collect::<String>()
                            );
                        }
                    }
                }
                _ => {}
            }
        }
        if digest.is_empty() {
            return;
        }
        self.request_title(session_id, &digest, &model, &digest)
            .await;
    }

    /// asks the model for a short title and applies it. falls back to the prompt when the
    /// model returns nothing usable, so a session is never left unnamed.
    async fn request_title(
        &self,
        session_id: &str,
        source: &str,
        model: &str,
        fallback_source: &str,
    ) {
        let _ = self.append(
            session_id,
            kinds::TITLE_GENERATING,
            &json!({"model": model, "effort": "low"}),
        );

        let title = if let Ok(mut rx) = self
            .config
            .provider
            .stream(crate::llm::ChatRequest {
                model: model.to_owned(),
                messages: vec![crate::llm::ChatMessage::user(format!(
                    "Give this coding session a title of at most six words. \
                     No quotes, no trailing punctuation. Reply with the title only.\n\n{source}"
                ))],
                tools: Vec::new(),
                // generous on purpose: a reasoning model spends budget on its trace before
                // it emits any text, and a tight cap yields an empty title.
                max_tokens: Some(128),
                reasoning_effort: Some("low".to_owned()),
            })
            .await
        {
            let mut raw = String::new();
            while let Some(delta) = rx.recv().await {
                if let crate::llm::Delta::Text(chunk) = delta {
                    raw.push_str(&chunk);
                }
            }
            clean_title(&raw)
        } else {
            None
        };

        let Some(title) = title.or_else(|| fallback_title(fallback_source)) else {
            return;
        };
        if let Err(err) = self.rename_session(session_id, &title) {
            tracing::debug!(%session_id, %err, "retitle failed");
        }
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
    /// exist yet. four characters per token, no tokeniser dependency.
    #[must_use]
    pub fn estimate_tokens(&self, session_id: &str) -> u64 {
        let Ok((events, _)) = self.store.events_since(session_id, 0, 100_000) else {
            return 0;
        };
        let chars: u64 = events
            .iter()
            .map(|e| {
                let mut len = e.payload.to_string().len() as u64;
                // attached pictures live in the log so the transcript can draw them, but
                // the model's view carries only the prompt: counting base64 would show a
                // few screenshots as most of the window.
                if let Some(images) = e.payload.get("images").and_then(Value::as_array) {
                    for image in images {
                        len = len.saturating_sub(image.to_string().len() as u64);
                    }
                }
                len
            })
            .sum();
        chars / CHARS_PER_TOKEN
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
                messages: vec![crate::llm::ChatMessage::user(format!(
                    "Summarise the following work log in at most 400 words. Keep decisions, \
                     file paths and open questions. Do not add commentary.\n{transcript}"
                ))],
                tools: Vec::new(),
                max_tokens: Some(1024),
                reasoning_effort: Some("low".to_owned()),
            })
            .await?;

        let mut summary = String::new();
        let mut usage = Usage {
            prompt_tokens: 0,
            completion_tokens: 0,
            cost_micros: None,
        };
        while let Some(delta) = rx.recv().await {
            match delta {
                crate::llm::Delta::Text(chunk) => summary.push_str(&chunk),
                crate::llm::Delta::Done {
                    prompt_tokens,
                    completion_tokens,
                    ..
                } => {
                    usage.prompt_tokens = prompt_tokens;
                    usage.completion_tokens = completion_tokens;
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
        self.start_turn(session_id, &prompt, &[], true, Some(&resumed_from))
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
    steering: Arc<std::sync::Mutex<Vec<String>>>,
}

impl EventSink for TurnSink {
    fn append(&self, kind: &str, payload: &Value) -> Result<u64, StoreError> {
        self.manager.append(&self.session_id, kind, payload)
    }

    fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }

    fn drain_steering(&self) -> Vec<String> {
        self.steering
            .lock()
            .map(|mut queue| std::mem::take(&mut *queue))
            .unwrap_or_default()
    }
}

/// tidies whatever the model returned into a usable title, or rejects it.
fn clean_title(raw: &str) -> Option<String> {
    let line = raw.lines().map(str::trim).find(|line| !line.is_empty())?;
    let mut title = line
        .trim_start_matches("Title:")
        .trim_start_matches("title:")
        .trim()
        .trim_matches(|c: char| matches!(c, '"' | '\'' | '`' | '*' | '#'))
        .trim()
        .trim_end_matches(['.', ':', ';', ','])
        .trim()
        .to_owned();
    if title.is_empty() {
        return None;
    }
    if title.chars().count() > 60 {
        title = format!("{}\u{2026}", title.chars().take(57).collect::<String>());
    }
    Some(title)
}

/// a title derived from the prompt, used when the model gives nothing back.
fn fallback_title(source: &str) -> Option<String> {
    let line = source
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())?;
    if line.is_empty() {
        return None;
    }
    let mut title: String = line.chars().take(57).collect();
    if line.chars().count() > 57 {
        title.push('\u{2026}');
    }
    Some(title)
}

fn default_title(cwd: &std::path::Path) -> String {
    cwd.file_name().map_or_else(
        || "session".to_owned(),
        |n| n.to_string_lossy().into_owned(),
    )
}
