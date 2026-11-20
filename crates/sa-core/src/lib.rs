//! sessions, event log, llm client, tools, policy.
//!
//! must not depend on a terminal crate and must never write to stdout (spec 2.2).

/// the message model the transcript is built in, re-exported so nothing has to depend on
/// rig directly to read one.
pub use rig_core;

pub mod agent;
pub mod dispatcher;
pub mod llm;
pub mod manager;
pub mod mcp;
pub mod peer;
pub mod store;
pub use sa_proto::tokens;
pub mod tools;
pub mod web;
pub mod workspace;
