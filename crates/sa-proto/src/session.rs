//! session, turn, approval and event wire types (spec 3.2, 5.2).

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// per-session autonomy level (spec 8.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Autonomy {
    ReadOnly,
    Allowlisted,
    Full,
}

impl Autonomy {
    /// whether a write inside the workspace is permitted.
    #[must_use]
    pub fn allows_write(self) -> bool {
        !matches!(self, Self::ReadOnly)
    }

    /// whether an allowlisted command may run.
    #[must_use]
    pub fn allows_command(self) -> bool {
        !matches!(self, Self::ReadOnly)
    }

    /// whether arbitrary commands and network access are permitted.
    #[must_use]
    pub fn is_unrestricted(self) -> bool {
        matches!(self, Self::Full)
    }
}

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
    pub fn as_str(self) -> &'static str {
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
    pub fn is_writable(self) -> bool {
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
    pub autonomy: Autonomy,
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
    pub autonomy: Autonomy,
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

/// approval decision (spec 8.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ApprovalDecision {
    Allow,
    Deny,
}

/// how far an allowance reaches. ignored on deny.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ApprovalScope {
    #[default]
    Once,
    Turn,
    Session,
    Class,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalRespondParams {
    pub approval_id: String,
    pub decision: ApprovalDecision,
    #[serde(default)]
    pub scope: ApprovalScope,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalEntry {
    pub approval_id: String,
    pub session_id: String,
    pub class: String,
    pub detail: String,
    pub requested_at: i64,
    pub expires_at: i64,
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
    pub const APPROVAL_REQUESTED: &str = "approval.requested";
    pub const APPROVAL_RESOLVED: &str = "approval.resolved";
    pub const MODEL_CHANGED: &str = "model.changed";
    pub const CHECKPOINT: &str = "checkpoint";
    pub const COMPACTION_APPLIED: &str = "compaction.applied";
    pub const REVIEW_COMPLETED: &str = "review.completed";
    pub const ERROR: &str = "error";

    /// kinds that terminate a turn.
    pub const TURN_TERMINAL: &[&str] = &[TURN_COMPLETED, TURN_INTERRUPTED];
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn autonomy_round_trips_as_kebab_case() {
        for (value, raw) in [
            (Autonomy::ReadOnly, "\"read-only\""),
            (Autonomy::Allowlisted, "\"allowlisted\""),
            (Autonomy::Full, "\"full\""),
        ] {
            assert_eq!(serde_json::to_string(&value).expect("ser"), raw);
            let back: Autonomy = serde_json::from_str(raw).expect("de");
            assert_eq!(back, value);
        }
    }

    #[test]
    fn read_only_permits_no_writes_or_commands() {
        assert!(!Autonomy::ReadOnly.allows_write());
        assert!(!Autonomy::ReadOnly.allows_command());
        assert!(Autonomy::Allowlisted.allows_write());
        assert!(!Autonomy::Allowlisted.is_unrestricted());
        assert!(Autonomy::Full.is_unrestricted());
    }

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

    #[test]
    fn approval_scope_defaults_to_once() {
        assert_eq!(ApprovalScope::default(), ApprovalScope::Once);
        let parsed: ApprovalRespondParams =
            serde_json::from_str(r#"{"approval_id":"a","decision":"allow"}"#).expect("de");
        assert_eq!(parsed.scope, ApprovalScope::Once);
    }
}
