//! the spec tree, executed (spec 9).
//!
//! every scenario under `specs/` is a claim the build makes about the daemon. a step with no
//! definition is a skipped step, and `fail_on_skipped` makes a skipped step a failure, so a
//! scenario that nothing implements cannot pass. `@allow.skipped` marks the scenarios whose
//! steps are not written yet: the gap is visible and countable rather than silent.
#![allow(
    clippy::expect_used,
    clippy::needless_pass_by_ref_mut,
    clippy::needless_pass_by_value,
    clippy::assert_is_empty,
    clippy::panic,
    clippy::print_stderr,
    clippy::too_many_lines
)]

mod cli;
mod client_library;
mod common;
mod compaction;
mod daemon_lifecycle;
mod engineering_constraints;
mod goal_mode;
mod hashline;
mod lsp;
mod mcp;
mod model_provider;
mod protocol;
mod session_lifecycle;
mod session_store;
mod terminal_front_end;
mod tools;
mod turns;
mod workspace;
mod world;

use cucumber::World as _;

#[tokio::main]
async fn main() {
    world::Spec::cucumber()
        .fail_on_skipped()
        .run_and_exit(concat!(env!("CARGO_MANIFEST_DIR"), "/../../specs"))
        .await;
}
