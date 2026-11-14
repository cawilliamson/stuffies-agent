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
    pub default_model: String,
    pub provider: Arc<dyn Provider>,
    pub max_tokens: Option<u64>,
    pub reasoning_effort: Option<String>,
    pub context_limit: u64,
    pub lock_dir: PathBuf,
    pub turn_timeout_secs: u64,
}

struct Attached {
    conn: u64,
    client: String,
    since: i64,
}

struct Subscription {
    conn: u64,
    session_id: String,
    exclude_kinds: Vec<String>,
}

struct RunningTurn {
    turn_id: String,
    cancel: Arc<AtomicBool>,
}

#[derive(Default)]
struct Inner {
    next_conn: u64,
    next_sub: u64,
    conns: HashMap<u64, mpsc::Sender<Value>>,
    subs: HashMap<u64, Subscription>,
    attached: HashMap<String, Attached>,
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
    pub fn store(&self) -> &Store {
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
            inner.attached.retain(|_, a| a.conn != conn);
        }
    }

    fn push(&self, inner: &Inner, conn: u64, value: Value) {
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
            self.push(
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
        let canonical = std::fs::canonicalize(cwd).unwrap_or_else(|_| PathBuf::from(cwd));
        let row = self.store.create_session(&NewSession {
            id: id.clone(),
            title: title.unwrap_or_else(|| default_title(&canonical)),
            cwd: canonical.display().to_string(),
            repo_id: None,
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
                "owner": {
                    "host_fingerprint": self.config.host_fingerprint,
                    "epoch": 0,
                },
                "knowledge_namespaces": ["shared"],
            }),
        )?;

        Ok(CreateSessionResult {
            session_id: id,
            seq,
        })
    }

    pub fn list_sessions(
        &self,
        include_deleted: bool,
    ) -> Result<Vec<SessionSummary>, ManagerError> {
        Ok(self.store.list_sessions(include_deleted)?)
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

    pub fn attach(
        &self,
        conn: u64,
        client: &str,
        session_id: &str,
        from_seq: u64,
        limit: usize,
        takeover: bool,
    ) -> Result<AttachResult, ManagerError> {
        let Some(row) = self.store.get_session(session_id)? else {
            return Err(ManagerError::Rpc(not_found(session_id)));
        };

        {
            let mut inner = self.lock()?;
            if let Some(existing) = inner.attached.get(session_id) {
                if existing.conn != conn && !takeover {
                    return Err(ManagerError::Rpc(
                        RpcError::new(
                            code::SESSION_BUSY,
                            format!(
                                "session {session_id} is attached to {} since {}",
                                existing.client, existing.since
                            ),
                        )
                        .with_data(json!({"client": existing.client, "since": existing.since})),
                    ));
                }
                let evicted = existing.conn;
                inner.attached.insert(
                    session_id.to_owned(),
                    Attached {
                        conn,
                        client: client.to_owned(),
                        since: crate::store::now_seconds(),
                    },
                );
                if evicted != conn {
                    self.push(
                        &inner,
                        evicted,
                        json!({
                            "jsonrpc": "2.0",
                            "method": "evicted",
                            "params": {"session_id": session_id, "reason": "takeover"},
                        }),
                    );
                }
            } else {
                inner.attached.insert(
                    session_id.to_owned(),
                    Attached {
                        conn,
                        client: client.to_owned(),
                        since: crate::store::now_seconds(),
                    },
                );
            }
        }

        let (events, more) = self.store.events_since(session_id, from_seq, limit)?;
        let next_seq = events.last().map_or(from_seq, |e| e.seq);
        Ok(AttachResult {
            session: row.summary(),
            events,
            more,
            next_seq,
        })
    }

    pub fn detach(&self, conn: u64, session_id: &str) {
        if let Ok(mut inner) = self.inner.lock() {
            if inner
                .attached
                .get(session_id)
                .is_some_and(|a| a.conn == conn)
            {
                inner.attached.remove(session_id);
            }
        }
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
                "model": self.config.default_model,
                "detach": detach,
                "client": attached_client,
            }),
        )?;

        self.lock()?.turns.insert(
            session_id.to_owned(),
            RunningTurn {
                turn_id: turn_id.clone(),
                cancel: Arc::clone(&cancel),
            },
        );

        let manager = Arc::clone(self);
        let session = session_id.to_owned();
        let prompt = prompt.to_owned();
        let turn = turn_id.clone();
        tokio::spawn(async move {
            manager
                .run_turn(session, turn, prompt, row, cancel, detach)
                .await;
        });

        Ok(TurnStartResult { turn_id, seq })
    }

    async fn run_turn(
        self: Arc<Self>,
        session_id: String,
        turn_id: String,
        prompt: String,
        row: SessionRow,
        cancel: Arc<AtomicBool>,
        detach: bool,
    ) {
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
        });
        let tools: Arc<dyn ToolExecutor> = Arc::new(ToolRegistry::new(PathBuf::from(&row.cwd)));

        let model = row
            .model
            .clone()
            .unwrap_or_else(|| self.config.default_model.clone());

        // compaction is checked before the turn, and only ever appends (spec 10.4).
        if let Err(err) = self.maybe_compact(&session_id, &model).await {
            tracing::warn!(%err, "compaction skipped");
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
            model: row
                .model
                .unwrap_or_else(|| self.config.default_model.clone()),
            history,
            compaction_summary,
            context: crate::workspace::context(&workspace),
            max_tokens: self.config.max_tokens,
            reasoning_effort: self.config.reasoning_effort.clone(),
        };

        // a stalled model stream must not hang the session forever (spec 14, v1.1).
        let outcome = match tokio::time::timeout(
            std::time::Duration::from_secs(self.config.turn_timeout_secs.max(1)),
            agent.run_turn(request),
        )
        .await
        {
            Ok(outcome) => outcome,
            Err(_) => {
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
            }
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

    // ---- compaction --------------------------------------------------

    /// estimates the token cost of the model's view of this session.
    #[must_use]
    pub fn estimate_tokens(&self, session_id: &str) -> u64 {
        let Ok((events, _)) = self.store.events_since(session_id, 0, 100_000) else {
            return 0;
        };
        let chars: u64 = events
            .iter()
            .map(|e| e.payload.to_string().len() as u64)
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
        let limit = self.config.context_limit;
        let threshold = (limit as f64 * COMPACTION_TRIGGER) as u64;
        let before = self.estimate_tokens(session_id);
        if limit == 0 || before < threshold {
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
                        transcript.push_str(&format!("\nuser: {prompt}\n"));
                    }
                }
                kinds::LLM_COMPLETED => {
                    if let Some(text) = event.payload.get("text").and_then(Value::as_str) {
                        transcript.push_str(&format!("assistant: {text}\n"));
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
        self.start_turn(session_id, &prompt, true, Some(&resumed_from))
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
}

fn default_title(cwd: &std::path::Path) -> String {
    cwd.file_name().map_or_else(
        || "session".to_owned(),
        |n| n.to_string_lossy().into_owned(),
    )
}
