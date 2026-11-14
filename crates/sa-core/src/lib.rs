//! sessions, event log, llm client, tools, policy.
//!
//! must not depend on a terminal crate and must never write to stdout (spec 2.2).

pub mod agent;
pub mod dispatcher;
pub mod llm;
pub mod manager;
pub mod peer;
pub mod store;
pub mod tools;
pub mod workspace;
