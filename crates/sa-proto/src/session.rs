//! session, turn and event wire types (spec 3.2, 5.2).

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// session lifecycle state (spec 5.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SessionState {
    Active,
    Quiescing,
    Relocating,
    Relocated,
    Interrupted,
    Deleted,
}

impl SessionState {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Quiescing => "quiescing",
            Self::Relocating => "relocating",
            Self::Relocated => "relocated",
            Self::Interrupted => "interrupted",
            Self::Deleted => "deleted",
        }
    }

    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        Some(match raw {
            "active" => Self::Active,
            "quiescing" => Self::Quiescing,
            "relocating" => Self::Relocating,
            "relocated" => Self::Relocated,
            "interrupted" => Self::Interrupted,
            "deleted" => Self::Deleted,
            _ => return None,
        })
    }

    /// whether a turn may start.
    #[must_use]
    pub const fn is_writable(self) -> bool {
        matches!(self, Self::Active)
    }
}

/// an append-only log record (spec 5.2).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub session_id: String,
    pub seq: u64,
    pub kind: String,
    pub at: i64,
    pub payload: Value,
}

/// a session as listed to a client.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSummary {
    pub id: String,
    pub title: String,
    pub state: SessionState,
    pub owner_epoch: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Usage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_micros: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateSessionParams {
    pub cwd: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateSessionResult {
    pub session_id: String,
    pub seq: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListSessionsParams {
    #[serde(default)]
    pub include_deleted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachParams {
    pub session_id: String,
    #[serde(default)]
    pub from_seq: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
    #[serde(default)]
    pub takeover: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachResult {
    pub session: SessionSummary,
    pub events: Vec<Event>,
    pub more: bool,
    pub next_seq: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplayParams {
    pub session_id: String,
    pub from_seq: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to_seq: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplayResult {
    pub events: Vec<Event>,
    pub more: bool,
    pub next_seq: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnStartParams {
    pub session_id: String,
    pub prompt: String,
    #[serde(default)]
    pub detach: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnStartResult {
    pub turn_id: String,
    pub seq: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnStatus {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    pub state: String,
    pub since: i64,
}

/// event kind names, so the daemon and clients cannot drift on spelling.
pub mod kinds {
    pub const SESSION_CREATED: &str = "session.created";
    pub const SESSION_RENAMED: &str = "session.renamed";
    pub const SESSION_STATE: &str = "session.state";
    pub const TURN_STARTED: &str = "turn.started";
    pub const TURN_COMPLETED: &str = "turn.completed";
    pub const TURN_INTERRUPTED: &str = "turn.interrupted";
    pub const LLM_DELTA: &str = "llm.delta";
    pub const LLM_REASONING: &str = "llm.reasoning";
    pub const LLM_COMPLETED: &str = "llm.completed";
    pub const TOOL_REQUESTED: &str = "tool.requested";
    pub const TOOL_COMPLETED: &str = "tool.completed";
    pub const TOOL_FAILED: &str = "tool.failed";
    pub const MODEL_CHANGED: &str = "model.changed";
    pub const TITLE_GENERATING: &str = "title.generating";
    pub const CHECKPOINT: &str = "checkpoint";
    pub const COMPACTION_APPLIED: &str = "compaction.applied";
    pub const REVIEW_COMPLETED: &str = "review.completed";
    pub const ERROR: &str = "error";

    /// kinds that terminate a turn.
    pub const TURN_TERMINAL: &[&str] = &[TURN_COMPLETED, TURN_INTERRUPTED];
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn session_state_parses_and_only_active_is_writable() {
        for state in [
            SessionState::Active,
            SessionState::Quiescing,
            SessionState::Relocating,
            SessionState::Relocated,
            SessionState::Interrupted,
            SessionState::Deleted,
        ] {
            assert_eq!(SessionState::parse(state.as_str()), Some(state));
        }
        assert!(SessionState::Active.is_writable());
        assert!(!SessionState::Relocated.is_writable());
        assert!(!SessionState::Deleted.is_writable());
    }
}
