//! the single implementation of every protocol method (spec 2.3).
//!
//! transports are carriers around this type. adding a transport must not add a code path
//! here, and adding a method must not require touching a transport.

use std::sync::Arc;
use std::time::Instant;

use sa_proto::error::{code, RpcError};
use sa_proto::rpc::{
    HelloParams, HelloResult, PingResult, Request, Response, DAEMON_VERSION, PROTOCOL_VERSION,
};
use sa_proto::session::{
    AttachParams, CreateSessionParams, ListSessionsParams, ReplayParams, SteerParams,
    TurnStartParams,
};
use serde_json::{json, Value};

use crate::manager::Manager;

/// per-connection protocol state.
#[derive(Debug)]
pub struct ConnState {
    conn: u64,
    client: Option<String>,
    handshaken: bool,
}

impl ConnState {
    #[must_use]
    pub const fn new(conn: u64) -> Self {
        Self {
            conn,
            client: None,
            handshaken: false,
        }
    }

    #[must_use]
    pub const fn is_handshaken(&self) -> bool {
        self.handshaken
    }

    #[must_use]
    pub const fn conn(&self) -> u64 {
        self.conn
    }

    #[must_use]
    pub fn client(&self) -> &str {
        self.client.as_deref().unwrap_or("unknown")
    }
}

pub struct Dispatcher {
    started: Instant,
    manager: Arc<Manager>,
}

impl Dispatcher {
    #[must_use]
    pub fn new(manager: Arc<Manager>) -> Self {
        Self {
            started: Instant::now(),
            manager,
        }
    }

    #[must_use]
    pub fn uptime_seconds(&self) -> u64 {
        self.started.elapsed().as_secs()
    }

    /// handles one request. returns `None` for notifications, which are never answered.
    pub fn handle(&self, conn: &mut ConnState, req: Request) -> Option<Response> {
        if req.is_notification() {
            return None;
        }
        Some(self.dispatch(conn, req))
    }

    fn dispatch(&self, conn: &mut ConnState, req: Request) -> Response {
        let id = req.id.clone();
        match self.call(conn, req) {
            Ok(result) => Response::ok(id, result),
            Err(error) => Response::err(id, error),
        }
    }

    fn call(&self, conn: &mut ConnState, req: Request) -> Result<Value, RpcError> {
        if !conn.handshaken {
            if req.method != "hello" {
                return Err(RpcError::new(
                    code::HANDSHAKE_REQUIRED,
                    format!("first message must be hello, got {}", req.method),
                ));
            }
            return Self::hello(conn, req.params);
        }

        let params = req.params.unwrap_or(Value::Null);
        // alphabetical: this is the protocol surface, and the only way to see whether a
        // method is handled is to read it as a list.
        match req.method.as_str() {
            "events.replay" => self.events_replay(&params),
            "events.subscribe" => self.events_subscribe(conn, &params),
            "events.unsubscribe" => self.events_unsubscribe(&params),
            "handoff.accept" | "handoff.export" | "handoff.import" => Err(RpcError::new(
                code::NOT_IMPLEMENTED,
                "handoff arrives at v4.0",
            )),
            "hello" => Self::hello(conn, Some(params)),
            "lsp.list" => self.lsp_list(),
            "mcp.list" => self.mcp_list(),
            "model.effort" => self.model_effort(&params),
            "model.list" => self.model_list(),
            "model.select" => self.model_select(&params),
            "ping" => self.ping(),
            "session.attach" => self.session_attach(&params),
            "session.create" => self.session_create(conn, &params),
            "session.cwd" => self.session_cwd(&params),
            "session.delete" => self.session_delete(&params),
            "session.fork" => Self::session_fork(&params),
            "session.list" => self.session_list(&params),
            "session.rename" => self.session_rename(&params),
            "session.restore" => self.session_restore(&params),
            "session.usage" => self.session_usage(&params),
            "turn.cancel" => self.turn_cancel(&params),
            "turn.dequeue" => self.turn_dequeue(&params),
            "turn.follow_up" => self.turn_follow_up(&params),
            "turn.queue_drop" => self.turn_queue_drop(&params),
            "turn.resume" => self.turn_resume(&params),
            "turn.start" => self.turn_start(conn, &params),
            "turn.status" => self.turn_status(&params),
            "turn.steer" => self.turn_steer(&params),
            other => Err(RpcError::method_not_found(other)),
        }
    }

    fn hello(conn: &mut ConnState, params: Option<Value>) -> Result<Value, RpcError> {
        let params: HelloParams = params
            .map(serde_json::from_value)
            .transpose()
            .map_err(|e| RpcError::new(code::INVALID_PARAMS, format!("bad hello params: {e}")))?
            .ok_or_else(|| RpcError::new(code::INVALID_PARAMS, "hello requires params"))?;

        if params.protocol != PROTOCOL_VERSION {
            return Err(RpcError::new(
                code::VERSION_MISMATCH,
                format!(
                    "protocol version mismatch: daemon speaks {}, client speaks {}",
                    PROTOCOL_VERSION, params.protocol
                ),
            )
            .with_data(json!({"daemon": PROTOCOL_VERSION, "client": params.protocol})));
        }

        conn.handshaken = true;
        conn.client = Some(params.client);
        Ok(json!(HelloResult {
            protocol: PROTOCOL_VERSION,
            version: DAEMON_VERSION.to_owned(),
            build_rev: sa_proto::BUILD_REV.to_owned(),
        }))
    }

    // uniform with every other dispatch arm: the caller expects a Result.
    #[allow(clippy::unnecessary_wraps)]
    fn ping(&self) -> Result<Value, RpcError> {
        Ok(json!(PingResult {
            version: DAEMON_VERSION.to_owned(),
            build_rev: sa_proto::BUILD_REV.to_owned(),
            uptime_seconds: self.uptime_seconds(),
            sessions: self
                .manager
                .list_sessions(false)
                .map_or(0, |s| s.len() as u64),
        }))
    }

    fn session_create(&self, conn: &ConnState, params: &Value) -> Result<Value, RpcError> {
        let params: CreateSessionParams = parse(params)?;
        let result = self
            .manager
            .create_session(&params.cwd, params.title, conn.client())
            .map_err(crate::manager::ManagerError::into_rpc)?;
        Ok(json!(result))
    }

    fn session_list(&self, params: &Value) -> Result<Value, RpcError> {
        let params: ListSessionsParams = if params.is_null() {
            ListSessionsParams {
                include_deleted: false,
            }
        } else {
            parse(params)?
        };
        let sessions = self
            .manager
            .list_sessions(params.include_deleted)
            .map_err(crate::manager::ManagerError::into_rpc)?;
        Ok(json!(sessions))
    }

    /// history for one session. sessions are not owned by a connection: any number of
    /// clients may hold the same one, which is what lets several windows show it at once.
    fn session_attach(&self, params: &Value) -> Result<Value, RpcError> {
        let params: AttachParams = parse(params)?;
        let limit = params.limit.unwrap_or(500).clamp(1, 5000);
        let result = self
            .manager
            .attach(&params.session_id, params.from_seq, limit)
            .map_err(crate::manager::ManagerError::into_rpc)?;
        Ok(json!(result))
    }

    fn session_rename(&self, params: &Value) -> Result<Value, RpcError> {
        let session_id = string_field(params, "session_id")?;
        let title = string_field(params, "title")?;
        self.manager
            .rename_session(&session_id, &title)
            .map_err(crate::manager::ManagerError::into_rpc)?;
        Ok(json!({}))
    }

    fn session_cwd(&self, params: &Value) -> Result<Value, RpcError> {
        let session_id = string_field(params, "session_id")?;
        let cwd = string_field(params, "cwd")?;
        let cwd = self
            .manager
            .set_cwd(&session_id, &cwd)
            .map_err(crate::manager::ManagerError::into_rpc)?;
        Ok(json!({"cwd": cwd}))
    }

    fn session_delete(&self, params: &Value) -> Result<Value, RpcError> {
        let session_id = string_field(params, "session_id")?;
        self.manager
            .delete_session(&session_id)
            .map_err(crate::manager::ManagerError::into_rpc)?;
        Ok(json!({}))
    }

    fn session_usage(&self, params: &Value) -> Result<Value, RpcError> {
        let session_id = string_field(params, "session_id")?;
        let usage = self
            .manager
            .usage(&session_id)
            .map_err(crate::manager::ManagerError::into_rpc)?;
        Ok(json!(usage))
    }

    fn session_restore(&self, params: &Value) -> Result<Value, RpcError> {
        let session_id = string_field(params, "session_id")?;
        self.manager
            .restore_session(&session_id)
            .map_err(crate::manager::ManagerError::into_rpc)?;
        Ok(json!({}))
    }

    fn session_fork(_params: &Value) -> Result<Value, RpcError> {
        Err(RpcError::new(
            code::NOT_IMPLEMENTED,
            "session.fork arrives at v4.0",
        ))
    }

    fn turn_start(&self, conn: &ConnState, params: &Value) -> Result<Value, RpcError> {
        let params: TurnStartParams = parse(params)?;
        let result = self
            .manager
            .start_turn(&crate::manager::NewTurn {
                session_id: &params.session_id,
                prompt: &params.prompt,
                images: &params.images,
                detach: params.detach,
                client: Some(conn.client()),
                resumed_from: None,
                continue_final: params.continue_final,
            })
            .map_err(crate::manager::ManagerError::into_rpc)?;
        Ok(json!(result))
    }

    fn turn_steer(&self, params: &Value) -> Result<Value, RpcError> {
        let params: SteerParams = parse(params)?;
        self.manager
            .steer(&params.session_id, &params.prompt)
            .map_err(crate::manager::ManagerError::into_rpc)?;
        Ok(json!({"disposition": "queued"}))
    }

    /// a message for after the running turn, rather than inside it.
    fn turn_follow_up(&self, params: &Value) -> Result<Value, RpcError> {
        let params: SteerParams = parse(params)?;
        self.manager
            .follow_up(&params.session_id, &params.prompt)
            .map_err(crate::manager::ManagerError::into_rpc)?;
        Ok(json!({"disposition": "queued"}))
    }

    /// takes one queued message back out, for editing or discarding.
    fn turn_queue_drop(&self, params: &Value) -> Result<Value, RpcError> {
        let session_id = string_field(params, "session_id")?;
        let queue = string_field(params, "queue")?;
        let index = params
            .get("index")
            .and_then(Value::as_u64)
            .map_or(0, |index| usize::try_from(index).unwrap_or(0));
        let state = self
            .manager
            .drop_queued(&session_id, &queue, index)
            .map_err(crate::manager::ManagerError::into_rpc)?;
        Ok(json!(state))
    }

    /// takes back everything queued so a client can restore it to the editor.
    fn turn_dequeue(&self, params: &Value) -> Result<Value, RpcError> {
        let session_id = string_field(params, "session_id")?;
        let queue = self
            .manager
            .dequeue(&session_id)
            .map_err(crate::manager::ManagerError::into_rpc)?;
        Ok(json!(queue))
    }

    fn turn_cancel(&self, params: &Value) -> Result<Value, RpcError> {
        let session_id = string_field(params, "session_id")?;
        self.manager
            .cancel_turn(&session_id)
            .map_err(crate::manager::ManagerError::into_rpc)?;
        Ok(json!({}))
    }

    fn turn_status(&self, params: &Value) -> Result<Value, RpcError> {
        let session_id = string_field(params, "session_id")?;
        Ok(json!(self.manager.turn_status(&session_id)))
    }

    fn turn_resume(&self, params: &Value) -> Result<Value, RpcError> {
        let session_id = string_field(params, "session_id")?;
        let result = self
            .manager
            .resume_turn(&session_id)
            .map_err(crate::manager::ManagerError::into_rpc)?;
        Ok(json!(result))
    }

    fn events_replay(&self, params: &Value) -> Result<Value, RpcError> {
        let params: ReplayParams = parse(params)?;
        let limit = params.limit.unwrap_or(500).clamp(1, 5000);
        let result = self
            .manager
            .replay(&params.session_id, params.from_seq, params.to_seq, limit)
            .map_err(crate::manager::ManagerError::into_rpc)?;
        Ok(json!(result))
    }

    fn events_subscribe(&self, conn: &ConnState, params: &Value) -> Result<Value, RpcError> {
        let session_id = string_field(params, "session_id")?;
        let from_seq = params.get("from_seq").and_then(Value::as_u64).unwrap_or(0);
        let exclude = params
            .get("filter")
            .and_then(|f| f.get("exclude_kinds"))
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();

        let subscription_id = self
            .manager
            .subscribe(conn.conn(), &session_id, exclude)
            .map_err(crate::manager::ManagerError::into_rpc)?;
        Ok(json!({"subscription_id": subscription_id, "from_seq": from_seq}))
    }

    fn events_unsubscribe(&self, params: &Value) -> Result<Value, RpcError> {
        let id = params
            .get("subscription_id")
            .and_then(Value::as_u64)
            .ok_or_else(|| RpcError::new(code::INVALID_PARAMS, "subscription_id required"))?;
        self.manager.unsubscribe(id);
        Ok(json!({}))
    }

    #[allow(clippy::unnecessary_wraps)]
    fn model_list(&self) -> Result<Value, RpcError> {
        let models = self.manager.models();
        Ok(json!(models
            .into_iter()
            .map(|model| {
                let id = model.id;
                let context_limit = model.context_limit;
                let effort_levels = model.effort_levels;
                json!({
                    "id": id,
                    // absent when the endpoint did not say, so a client shows no guess.
                    "context_limit": context_limit,
                    "capabilities": ["tools", "streaming"],
                    // the endpoint's own values, in its own order. empty means none.
                    "effort_levels": effort_levels,
                })
            })
            .collect::<Vec<_>>()))
    }

    #[allow(clippy::unnecessary_wraps)]
    fn lsp_list(&self) -> Result<Value, RpcError> {
        Ok(json!(self
            .manager
            .lsp_status()
            .into_iter()
            .map(|(name, installed, running)| json!({
                "name": name,
                "installed": installed,
                "running": running,
            }))
            .collect::<Vec<_>>()))
    }

    #[allow(clippy::unnecessary_wraps)]
    fn mcp_list(&self) -> Result<Value, RpcError> {
        Ok(json!(self
            .manager
            .mcp_status()
            .into_iter()
            .map(|server| json!({
                "name": server.name,
                "transport": server.transport,
                "tools": server.tools,
                "error": server.error,
            }))
            .collect::<Vec<_>>()))
    }

    fn model_effort(&self, params: &Value) -> Result<Value, RpcError> {
        let session_id = string_field(params, "session_id")?;
        let effort = string_field(params, "effort")?;
        let model = self
            .manager
            .session_model(&session_id)
            .map_err(crate::manager::ManagerError::into_rpc)?;
        let levels = self.manager.effort_levels(&model);
        // an endpoint that advertises nothing cannot be second-guessed, so anything goes;
        // one that does advertises is obeyed.
        if !levels.is_empty() && !levels.contains(&effort) {
            return Err(RpcError::new(
                code::INVALID_PARAMS,
                format!("{model} accepts {}: not {effort}", levels.join(", ")),
            ));
        }
        self.manager
            .set_effort(&session_id, &effort)
            .map_err(crate::manager::ManagerError::into_rpc)?;
        Ok(json!({"effort": effort}))
    }

    fn model_select(&self, params: &Value) -> Result<Value, RpcError> {
        let session_id = string_field(params, "session_id")?;
        let model = string_field(params, "model")?;
        // a model the endpoint does not serve is a typo, not a wish: say so rather than
        // storing a name no turn can use.
        let served = self.manager.models();
        if !served.is_empty() && !served.iter().any(|known| known.id == model) {
            return Err(RpcError::new(
                code::INVALID_PARAMS,
                format!(
                    "{model} is not served; the endpoint offers {}",
                    served
                        .iter()
                        .map(|known| known.id.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            ));
        }
        self.manager
            .set_model(&session_id, &model)
            .map_err(crate::manager::ManagerError::into_rpc)?;
        Ok(json!({"model": model}))
    }
}

fn parse<T: serde::de::DeserializeOwned>(params: &Value) -> Result<T, RpcError> {
    serde_json::from_value(params.clone())
        .map_err(|e| RpcError::new(code::INVALID_PARAMS, format!("bad params: {e}")))
}

fn string_field(params: &Value, key: &str) -> Result<String, RpcError> {
    params
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| RpcError::new(code::INVALID_PARAMS, format!("{key} required")))
}
