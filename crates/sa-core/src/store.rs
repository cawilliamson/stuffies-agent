//! sqlite event log, the sole source of truth for live sessions (spec 6.1).
//!
//! ponytail: one connection behind a mutex. every call is a local write measured in
//! microseconds, and turns are sequential per session. if contention ever shows up, move to
//! a pool keyed by session.

use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection, OptionalExtension};
use sa_proto::session::{Autonomy, Event, SessionState, SessionSummary};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

/// payload fields larger than this are spilled to the blob table (spec 6.3).
pub const BLOB_THRESHOLD: usize = 64 * 1024;

const SCHEMA_VERSION: i64 = 1;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("store lock poisoned")]
    Lock,
    #[error("session not found: {0}")]
    SessionNotFound(String),
    #[error("unknown session state: {0}")]
    UnknownState(String),
    #[error("blob missing: {0}")]
    MissingBlob(String),
}

#[derive(Clone)]
pub struct Store {
    conn: Arc<Mutex<Connection>>,
}

/// the fields needed to create a session row.
#[derive(Debug, Clone)]
pub struct NewSession {
    pub id: String,
    pub title: String,
    pub cwd: String,
    pub repo_id: Option<String>,
    pub autonomy: Autonomy,
    pub owner_host: String,
    pub owner_epoch: u64,
}

/// a full session row, as the daemon sees it.
#[derive(Debug, Clone)]
pub struct SessionRow {
    pub id: String,
    pub title: String,
    pub parent_id: Option<String>,
    pub state: SessionState,
    pub owner_host: String,
    pub owner_epoch: u64,
    pub cwd: String,
    pub repo_id: Option<String>,
    pub autonomy: Autonomy,
    pub model: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

impl SessionRow {
    #[must_use]
    pub fn summary(&self) -> SessionSummary {
        SessionSummary {
            id: self.id.clone(),
            title: self.title.clone(),
            state: self.state,
            owner_epoch: self.owner_epoch,
            autonomy: self.autonomy,
            updated_at: self.updated_at,
        }
    }
}

/// a turn that never reached a terminal event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenTurn {
    pub session_id: String,
    pub turn_id: String,
    pub pending_calls: Vec<String>,
}

#[must_use]
pub fn now_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

fn autonomy_str(autonomy: Autonomy) -> &'static str {
    match autonomy {
        Autonomy::ReadOnly => "read-only",
        Autonomy::Allowlisted => "allowlisted",
        Autonomy::Full => "full",
    }
}

fn parse_autonomy(raw: &str) -> Autonomy {
    match raw {
        "full" => Autonomy::Full,
        "allowlisted" => Autonomy::Allowlisted,
        _ => Autonomy::ReadOnly,
    }
}

impl Store {
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        Self::from_connection(conn)
    }

    pub fn open_in_memory() -> Result<Self, StoreError> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    fn from_connection(conn: Connection) -> Result<Self, StoreError> {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "FULL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        migrate(&conn)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    fn conn(&self) -> Result<MutexGuard<'_, Connection>, StoreError> {
        self.conn.lock().map_err(|_| StoreError::Lock)
    }

    // ---- sessions ----------------------------------------------------

    pub fn create_session(&self, new: &NewSession) -> Result<SessionRow, StoreError> {
        let now = now_seconds();
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO sessions (id, title, parent_id, state, owner_host, owner_epoch, cwd,
                                   repo_id, autonomy, model, created_at, updated_at)
             VALUES (?1, ?2, NULL, 'active', ?3, ?4, ?5, ?6, ?7, NULL, ?8, ?8)",
            params![
                new.id,
                new.title,
                new.owner_host,
                i64::try_from(new.owner_epoch).unwrap_or(i64::MAX),
                new.cwd,
                new.repo_id,
                autonomy_str(new.autonomy),
                now,
            ],
        )?;
        drop(conn);
        self.get_session(&new.id)?
            .ok_or_else(|| StoreError::SessionNotFound(new.id.clone()))
    }

    pub fn get_session(&self, id: &str) -> Result<Option<SessionRow>, StoreError> {
        let conn = self.conn()?;
        let row = conn
            .query_row(
                "SELECT id, title, parent_id, state, owner_host, owner_epoch, cwd, repo_id,
                        autonomy, model, created_at, updated_at
                 FROM sessions WHERE id = ?1",
                params![id],
                map_session,
            )
            .optional()?;
        row.transpose()
    }

    pub fn list_sessions(&self, include_deleted: bool) -> Result<Vec<SessionSummary>, StoreError> {
        let conn = self.conn()?;
        let sql = if include_deleted {
            "SELECT id, title, parent_id, state, owner_host, owner_epoch, cwd, repo_id,
                    autonomy, model, created_at, updated_at
             FROM sessions ORDER BY updated_at DESC"
        } else {
            "SELECT id, title, parent_id, state, owner_host, owner_epoch, cwd, repo_id,
                    autonomy, model, created_at, updated_at
             FROM sessions WHERE state != 'deleted' ORDER BY updated_at DESC"
        };
        let mut stmt = conn.prepare(sql)?;
        let rows = stmt.query_map([], map_session)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row??.summary());
        }
        Ok(out)
    }

    pub fn set_session_state(&self, id: &str, state: SessionState) -> Result<(), StoreError> {
        let conn = self.conn()?;
        let changed = conn.execute(
            "UPDATE sessions SET state = ?2, updated_at = ?3 WHERE id = ?1",
            params![id, state.as_str(), now_seconds()],
        )?;
        if changed == 0 {
            return Err(StoreError::SessionNotFound(id.to_owned()));
        }
        Ok(())
    }

    pub fn rename_session(&self, id: &str, title: &str) -> Result<(), StoreError> {
        let conn = self.conn()?;
        let changed = conn.execute(
            "UPDATE sessions SET title = ?2, updated_at = ?3 WHERE id = ?1",
            params![id, title, now_seconds()],
        )?;
        if changed == 0 {
            return Err(StoreError::SessionNotFound(id.to_owned()));
        }
        Ok(())
    }

    pub fn set_session_model(&self, id: &str, model: &str) -> Result<(), StoreError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE sessions SET model = ?2, updated_at = ?3 WHERE id = ?1",
            params![id, model, now_seconds()],
        )?;
        Ok(())
    }

    // ---- events ------------------------------------------------------

    /// appends one event, allocating the next `seq` inside a single immediate transaction.
    pub fn append_event(
        &self,
        session_id: &str,
        kind: &str,
        payload: &Value,
    ) -> Result<u64, StoreError> {
        let mut conn = self.conn()?;
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;

        let seq: i64 = tx.query_row(
            "SELECT COALESCE(MAX(seq), 0) + 1 FROM events WHERE session_id = ?1",
            params![session_id],
            |row| row.get(0),
        )?;

        let mut stored = payload.clone();
        spill(&tx, &mut stored)?;

        tx.execute(
            "INSERT INTO events (session_id, seq, kind, payload, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                session_id,
                seq,
                kind,
                serde_json::to_string(&stored)?,
                now_seconds()
            ],
        )?;
        tx.execute(
            "UPDATE sessions SET updated_at = ?2 WHERE id = ?1",
            params![session_id, now_seconds()],
        )?;
        tx.commit()?;
        Ok(u64::try_from(seq).unwrap_or(u64::MAX))
    }

    /// events with `seq` greater than `from_seq`, in order.
    pub fn events_since(
        &self,
        session_id: &str,
        from_seq: u64,
        limit: usize,
    ) -> Result<(Vec<Event>, bool), StoreError> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT session_id, seq, kind, payload, created_at
             FROM events WHERE session_id = ?1 AND seq > ?2
             ORDER BY seq ASC LIMIT ?3",
        )?;
        let fetch = i64::try_from(limit.saturating_add(1)).unwrap_or(i64::MAX);
        let rows = stmt.query_map(
            params![
                session_id,
                i64::try_from(from_seq).unwrap_or(i64::MAX),
                fetch
            ],
            |row| {
                let raw: String = row.get(3)?;
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    raw,
                    row.get::<_, i64>(4)?,
                ))
            },
        )?;

        let mut events = Vec::new();
        for row in rows {
            let (sid, seq, kind, raw, at) = row?;
            let mut payload: Value = serde_json::from_str(&raw)?;
            expand(&conn, &mut payload)?;
            events.push(Event {
                session_id: sid,
                seq: u64::try_from(seq).unwrap_or(u64::MAX),
                kind,
                at,
                payload,
            });
        }

        let more = events.len() > limit;
        events.truncate(limit);
        Ok((events, more))
    }

    pub fn last_seq(&self, session_id: &str) -> Result<u64, StoreError> {
        let conn = self.conn()?;
        let seq: i64 = conn.query_row(
            "SELECT COALESCE(MAX(seq), 0) FROM events WHERE session_id = ?1",
            params![session_id],
            |row| row.get(0),
        )?;
        Ok(u64::try_from(seq).unwrap_or(u64::MAX))
    }

    /// the last event of any of `kinds` for a session.
    pub fn last_event_of_kind(
        &self,
        session_id: &str,
        kinds: &[&str],
    ) -> Result<Option<Event>, StoreError> {
        let conn = self.conn()?;
        let placeholders = (0..kinds.len())
            .map(|i| format!("?{}", i + 2))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT session_id, seq, kind, payload, created_at FROM events
             WHERE session_id = ?1 AND kind IN ({placeholders})
             ORDER BY seq DESC LIMIT 1"
        );
        let mut stmt = conn.prepare(&sql)?;
        let mut bound: Vec<&dyn rusqlite::ToSql> = Vec::new();
        bound.push(&session_id);
        for kind in kinds {
            bound.push(kind);
        }
        let row = stmt
            .query_row(bound.as_slice(), |row| {
                let raw: String = row.get(3)?;
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    raw,
                    row.get::<_, i64>(4)?,
                ))
            })
            .optional()?;
        match row {
            None => Ok(None),
            Some((sid, seq, kind, raw, at)) => {
                let mut payload: Value = serde_json::from_str(&raw)?;
                expand(&conn, &mut payload)?;
                Ok(Some(Event {
                    session_id: sid,
                    seq: u64::try_from(seq).unwrap_or(u64::MAX),
                    kind,
                    at,
                    payload,
                }))
            }
        }
    }

    /// tool calls with no terminal event, in the given seq range.
    pub fn unmatched_tool_calls(
        &self,
        session_id: &str,
        after_seq: u64,
    ) -> Result<Vec<String>, StoreError> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT kind, payload FROM events
             WHERE session_id = ?1 AND seq > ?2
               AND kind IN ('tool.requested', 'tool.completed', 'tool.failed')
             ORDER BY seq ASC",
        )?;
        let rows = stmt.query_map(
            params![session_id, i64::try_from(after_seq).unwrap_or(i64::MAX)],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )?;

        let mut pending: Vec<String> = Vec::new();
        for row in rows {
            let (kind, raw) = row?;
            let payload: Value = serde_json::from_str(&raw)?;
            let call_id = payload
                .get("call_id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            if kind == "tool.requested" {
                pending.push(call_id);
            } else if let Some(pos) = pending.iter().position(|id| id == &call_id) {
                pending.remove(pos);
            }
        }
        Ok(pending)
    }

    /// sessions whose last turn event is a start, not a terminal (spec 5.6).
    pub fn open_turns(&self) -> Result<Vec<OpenTurn>, StoreError> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare("SELECT id FROM sessions WHERE state = 'active'")?;
        let ids: Vec<String> = stmt
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<Result<_, _>>()?;
        drop(stmt);
        drop(conn);

        let mut out = Vec::new();
        for id in ids {
            let Some(last) = self
                .last_event_of_kind(&id, &["turn.started", "turn.completed", "turn.interrupted"])?
            else {
                continue;
            };
            if last.kind != "turn.started" {
                continue;
            }
            let turn_id = last
                .payload
                .get("turn_id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let pending = self.unmatched_tool_calls(&id, last.seq)?;
            out.push(OpenTurn {
                session_id: id,
                turn_id,
                pending_calls: pending,
            });
        }
        Ok(out)
    }

    // ---- blobs -------------------------------------------------------

    pub fn get_blob(&self, hash: &str) -> Result<Vec<u8>, StoreError> {
        let conn = self.conn()?;
        let data: Option<Vec<u8>> = conn
            .query_row(
                "SELECT data FROM blobs WHERE hash = ?1",
                params![hash],
                |row| row.get(0),
            )
            .optional()?;
        data.ok_or_else(|| StoreError::MissingBlob(hash.to_owned()))
    }

    #[must_use]
    pub fn blob_count(&self) -> usize {
        self.conn()
            .ok()
            .and_then(|conn| {
                conn.query_row("SELECT COUNT(*) FROM blobs", [], |row| row.get::<_, i64>(0))
                    .ok()
            })
            .and_then(|n| usize::try_from(n).ok())
            .unwrap_or(0)
    }
}

fn map_session(row: &rusqlite::Row<'_>) -> rusqlite::Result<Result<SessionRow, StoreError>> {
    let state_raw: String = row.get(3)?;
    let Some(state) = SessionState::parse(&state_raw) else {
        return Ok(Err(StoreError::UnknownState(state_raw)));
    };
    let autonomy_raw: String = row.get(8)?;
    Ok(Ok(SessionRow {
        id: row.get(0)?,
        title: row.get(1)?,
        parent_id: row.get(2)?,
        state,
        owner_host: row.get(4)?,
        owner_epoch: u64::try_from(row.get::<_, i64>(5)?).unwrap_or(0),
        cwd: row.get(6)?,
        repo_id: row.get(7)?,
        autonomy: parse_autonomy(&autonomy_raw),
        model: row.get(9)?,
        created_at: row.get(10)?,
        updated_at: row.get(11)?,
    }))
}

/// replaces oversized strings with blob references (spec 6.3).
fn spill(conn: &Connection, value: &mut Value) -> Result<(), StoreError> {
    match value {
        Value::String(text) if text.len() > BLOB_THRESHOLD => {
            let mut hasher = Sha256::new();
            hasher.update(text.as_bytes());
            let hash = hex(&hasher.finalize());
            conn.execute(
                "INSERT OR IGNORE INTO blobs (hash, bytes, data) VALUES (?1, ?2, ?3)",
                params![
                    hash,
                    i64::try_from(text.len()).unwrap_or(i64::MAX),
                    text.as_bytes()
                ],
            )?;
            *value = json!({"$blob": hash, "bytes": text.len()});
            Ok(())
        }
        Value::Array(items) => {
            for item in items {
                spill(conn, item)?;
            }
            Ok(())
        }
        Value::Object(map) => {
            for (_, item) in map.iter_mut() {
                spill(conn, item)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

/// re-inlines blob references on read.
fn expand(conn: &Connection, value: &mut Value) -> Result<(), StoreError> {
    match value {
        Value::Object(map) => {
            if map.len() == 2 {
                if let (Some(Value::String(hash)), Some(_)) =
                    (map.get("$blob").cloned().as_ref(), map.get("bytes"))
                {
                    let hash = hash.clone();
                    let data: Option<Vec<u8>> = conn
                        .query_row(
                            "SELECT data FROM blobs WHERE hash = ?1",
                            params![hash],
                            |row| row.get(0),
                        )
                        .optional()?;
                    let bytes = data.ok_or(StoreError::MissingBlob(hash))?;
                    *value = Value::String(String::from_utf8_lossy(&bytes).into_owned());
                    return Ok(());
                }
            }
            for (_, item) in map.iter_mut() {
                expand(conn, item)?;
            }
            Ok(())
        }
        Value::Array(items) => {
            for item in items {
                expand(conn, item)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

fn migrate(conn: &Connection) -> Result<(), StoreError> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS events (
             id INTEGER PRIMARY KEY,
             session_id TEXT NOT NULL,
             seq INTEGER NOT NULL,
             kind TEXT NOT NULL,
             payload TEXT NOT NULL,
             created_at INTEGER NOT NULL,
             UNIQUE(session_id, seq)
         );
         CREATE TABLE IF NOT EXISTS sessions (
             id TEXT PRIMARY KEY,
             title TEXT NOT NULL,
             parent_id TEXT,
             state TEXT NOT NULL,
             owner_host TEXT NOT NULL,
             owner_epoch INTEGER NOT NULL,
             cwd TEXT NOT NULL,
             repo_id TEXT,
             autonomy TEXT NOT NULL,
             model TEXT,
             created_at INTEGER NOT NULL,
             updated_at INTEGER NOT NULL
         );
         CREATE TABLE IF NOT EXISTS approvals (
             id TEXT PRIMARY KEY,
             session_id TEXT NOT NULL,
             seq INTEGER NOT NULL,
             state TEXT NOT NULL,
             request TEXT NOT NULL,
             resolution TEXT,
             requested_at INTEGER NOT NULL,
             resolved_at INTEGER
         );
         CREATE TABLE IF NOT EXISTS blobs (
             hash TEXT PRIMARY KEY,
             bytes INTEGER NOT NULL,
             data BLOB NOT NULL
         );
         CREATE TABLE IF NOT EXISTS web_sessions (
             id TEXT PRIMARY KEY,
             created_at INTEGER NOT NULL,
             expires_at INTEGER NOT NULL,
             peer TEXT
         );
         CREATE TABLE IF NOT EXISTS schema_version (version INTEGER NOT NULL);
         INSERT INTO schema_version (version)
             SELECT 1 WHERE NOT EXISTS (SELECT 1 FROM schema_version);",
    )?;
    let version: i64 = conn.query_row("SELECT version FROM schema_version LIMIT 1", [], |row| {
        row.get(0)
    })?;
    if version != SCHEMA_VERSION {
        return Err(StoreError::UnknownState(format!(
            "schema version {version}, expected {SCHEMA_VERSION}"
        )));
    }
    Ok(())
}
