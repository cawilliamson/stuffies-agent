//! the goal loop (spec 9): a session is nudged until `goal_done`, and stops when it should.
#![allow(clippy::expect_used, clippy::panic)]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use sa_core::agent::{OutputSink, ToolExecutor};
use sa_core::llm::{ChatRequest, Delta, ModelsFuture, Provider, StreamFuture, ToolCall};
use sa_core::manager::{Manager, ManagerConfig, NewTurn};
use sa_core::store::Store;
use sa_core::tools::ToolRegistry;
use sa_proto::session::kinds;
use serde_json::{json, Value};
use tokio::sync::{mpsc, Notify};

/// a provider that holds each stream open until the test releases it, so a turn is genuinely
/// running while the test looks at it. `fail` makes the next release an error, which is what
/// an interrupted turn looks like from the manager's side.
struct GatedProvider {
    entered: Arc<Notify>,
    release: Arc<Notify>,
    fail: AtomicBool,
}

impl GatedProvider {
    fn new() -> Self {
        Self {
            entered: Arc::new(Notify::new()),
            release: Arc::new(Notify::new()),
            fail: AtomicBool::new(false),
        }
    }
}

impl Provider for GatedProvider {
    fn list_models(&self) -> ModelsFuture<'_> {
        Box::pin(async { Ok(Vec::new()) })
    }

    fn stream(&self, _request: ChatRequest) -> StreamFuture<'_> {
        let entered = Arc::clone(&self.entered);
        let release = Arc::clone(&self.release);
        let fail = self.fail.load(Ordering::SeqCst);
        Box::pin(async move {
            let (tx, rx) = mpsc::channel(8);
            tokio::spawn(async move {
                entered.notify_one();
                release.notified().await;
                let last = if fail {
                    Delta::Failed("endpoint went away".to_owned())
                } else {
                    Delta::Done {
                        stop_reason: "stop".to_owned(),
                        prompt_tokens: 1,
                        completion_tokens: 1,
                        cached_tokens: 0,
                    }
                };
                let _ = tx.send(last).await;
            });
            Ok(rx)
        })
    }
}

fn manager(dir: &std::path::Path, provider: &Arc<GatedProvider>) -> Arc<Manager> {
    let store = Store::open(&dir.join("sa.db")).expect("store");
    Arc::new(Manager::new(
        store,
        Arc::new(ManagerConfig {
            host_fingerprint: "test-host".to_owned(),
            provider: Arc::clone(provider) as Arc<dyn Provider>,
            max_tokens: None,
            lock_dir: dir.join("locks"),
            checkpoint_dir: dir.join("checkpoints"),
            turn_timeout_secs: 30,
            aux_budget_secs: 5,
            models: Arc::new(Mutex::new(Vec::new())),
            mcp: Arc::new(sa_core::mcp::Hub::empty()),
            lsp: Arc::new(sa_core::lsp::Hub::new()),
        }),
    ))
}

fn session(manager: &Arc<Manager>, dir: &std::path::Path) -> String {
    manager
        .create_session(&dir.display().to_string(), Some("t".to_owned()), "test")
        .expect("session")
        .session_id
}

fn payloads(manager: &Arc<Manager>, session_id: &str, kind: &str) -> Vec<Value> {
    manager
        .store()
        .events_since(session_id, 0, 10_000)
        .expect("events")
        .0
        .into_iter()
        .filter(|event| event.kind == kind)
        .map(|event| event.payload)
        .collect()
}

/// lets the turn that is already in flight finish, and waits for the manager to settle it.
async fn release_running_turn(provider: &Arc<GatedProvider>) {
    provider.entered.notified().await;
    provider.release.notify_one();
    // the turn's own task finishes shortly after the stream does; give it room to settle
    // before the next assertion reads the log.
    tokio::time::sleep(std::time::Duration::from_millis(120)).await;
}

/// starts a turn and runs it to its end.
async fn run_turn(
    manager: &Arc<Manager>,
    provider: &Arc<GatedProvider>,
    session_id: &str,
    prompt: &str,
) {
    manager
        .start_turn(&NewTurn::new(session_id, prompt))
        .expect("turn");
    release_running_turn(provider).await;
}

#[tokio::test]
async fn setting_a_goal_reports_it_active() {
    let dir = tempfile::tempdir().expect("tempdir");
    let provider = Arc::new(GatedProvider::new());
    let manager = manager(dir.path(), &provider);
    let id = session(&manager, dir.path());

    let status = manager.set_goal(&id, "ship the thing", 5).expect("set");
    assert!(status.active);
    assert_eq!(status.text, "ship the thing");
    assert_eq!(status.iterations, 0);
    assert_eq!(status.max_iterations, 5);
}

#[tokio::test]
async fn a_completed_turn_that_has_not_finished_the_goal_is_nudged() {
    let dir = tempfile::tempdir().expect("tempdir");
    let provider = Arc::new(GatedProvider::new());
    let manager = manager(dir.path(), &provider);
    let id = session(&manager, dir.path());
    manager.set_goal(&id, "ship the thing", 5).expect("set");

    run_turn(&manager, &provider, &id, "start").await;

    let nudges = payloads(&manager, &id, kinds::GOAL_NUDGED);
    assert_eq!(nudges.len(), 1, "expected one nudge: {nudges:?}");
    assert_eq!(nudges[0]["iteration"], 1);

    // the nudge became the session's next turn, carrying the goal text.
    let started = payloads(&manager, &id, kinds::TURN_STARTED);
    assert_eq!(
        started.len(),
        2,
        "the nudge did not open a turn: {started:?}"
    );
    let prompt = started[1]["prompt"].as_str().unwrap_or_default();
    assert!(
        prompt.contains("Keep working on the goal: ship the thing"),
        "{prompt}"
    );

    let status = manager.goal_status(&id).expect("status");
    assert!(status.active);
    assert_eq!(status.iterations, 1);
}

#[tokio::test]
async fn calling_goal_done_ends_the_loop() {
    let dir = tempfile::tempdir().expect("tempdir");
    let provider = Arc::new(GatedProvider::new());
    let manager = manager(dir.path(), &provider);
    let id = session(&manager, dir.path());
    manager.set_goal(&id, "ship the thing", 5).expect("set");

    // what the goal_done tool records; the manager reads its own log to decide.
    manager
        .append(&id, kinds::GOAL_DONE, &json!({"summary": "shipped"}))
        .expect("append");
    run_turn(&manager, &provider, &id, "start").await;

    assert!(
        payloads(&manager, &id, kinds::GOAL_NUDGED).is_empty(),
        "a finished goal was nudged anyway"
    );
    assert_eq!(payloads(&manager, &id, kinds::TURN_STARTED).len(), 1);

    let status = manager.goal_status(&id).expect("status");
    assert!(!status.active);
    assert_eq!(status.summary.as_deref(), Some("shipped"));
}

#[tokio::test]
async fn the_loop_stops_at_its_cap_rather_than_circling() {
    let dir = tempfile::tempdir().expect("tempdir");
    let provider = Arc::new(GatedProvider::new());
    let manager = manager(dir.path(), &provider);
    let id = session(&manager, dir.path());
    manager.set_goal(&id, "ship the thing", 1).expect("set");

    run_turn(&manager, &provider, &id, "start").await;
    // the nudge already opened the second turn; letting it end spends the last nudge.
    release_running_turn(&provider).await;

    let nudges = payloads(&manager, &id, kinds::GOAL_NUDGED);
    assert_eq!(nudges.len(), 1, "the cap did not hold: {nudges:?}");
    let stopped = payloads(&manager, &id, kinds::GOAL_STOPPED);
    assert_eq!(stopped.len(), 1, "{stopped:?}");
    assert_eq!(stopped[0]["reason"], "exhausted");
    assert!(!manager.goal_status(&id).expect("status").active);
}

#[tokio::test]
async fn a_failed_turn_does_not_nudge() {
    let dir = tempfile::tempdir().expect("tempdir");
    let provider = Arc::new(GatedProvider::new());
    let manager = manager(dir.path(), &provider);
    let id = session(&manager, dir.path());
    manager.set_goal(&id, "ship the thing", 5).expect("set");

    provider.fail.store(true, Ordering::SeqCst);
    run_turn(&manager, &provider, &id, "start").await;

    assert!(
        payloads(&manager, &id, kinds::GOAL_NUDGED).is_empty(),
        "a failed turn was treated as progress"
    );
    // the goal stays open, so the next turn that does finish resumes the loop.
    assert!(manager.goal_status(&id).expect("status").active);
}

#[tokio::test]
async fn stopping_a_goal_records_who_ended_it() {
    let dir = tempfile::tempdir().expect("tempdir");
    let provider = Arc::new(GatedProvider::new());
    let manager = manager(dir.path(), &provider);
    let id = session(&manager, dir.path());
    manager.set_goal(&id, "ship the thing", 5).expect("set");

    let status = manager.stop_goal(&id, "user").expect("stop");
    assert!(!status.active);
    let stopped = payloads(&manager, &id, kinds::GOAL_STOPPED);
    assert_eq!(stopped[0]["reason"], "user");
}

#[tokio::test]
async fn a_second_goal_does_not_inherit_the_first_ones_nudges() {
    let dir = tempfile::tempdir().expect("tempdir");
    let provider = Arc::new(GatedProvider::new());
    let manager = manager(dir.path(), &provider);
    let id = session(&manager, dir.path());

    manager.set_goal(&id, "first", 5).expect("set");
    run_turn(&manager, &provider, &id, "start").await;
    assert_eq!(manager.goal_status(&id).expect("status").iterations, 1);

    manager.set_goal(&id, "second", 5).expect("set");
    let status = manager.goal_status(&id).expect("status");
    assert!(status.active);
    assert_eq!(status.text, "second");
    assert_eq!(status.iterations, 0, "the new goal inherited a nudge count");
}

/// what the `goal_done` tool writes when a turn's sink is the session log.
#[derive(Default)]
struct Capture {
    events: Mutex<Vec<(String, Value)>>,
}

impl OutputSink for Capture {
    fn output(&self, _call_id: &str, _chunk: &str) {}

    fn event(&self, kind: &str, payload: &Value) {
        self.events
            .lock()
            .expect("lock")
            .push((kind.to_owned(), payload.clone()));
    }
}

#[tokio::test]
async fn the_goal_done_tool_records_the_event_the_loop_reads() {
    let dir = tempfile::tempdir().expect("tempdir");
    let registry = ToolRegistry::new(dir.path().to_path_buf());
    let capture = Capture::default();
    let call = ToolCall {
        id: "c1".to_owned(),
        name: "goal_done".to_owned(),
        arguments: json!({"summary": "shipped it"}).to_string(),
    };
    let outcome = registry
        .execute_streaming(call, &capture)
        .await
        .expect("goal_done");
    assert!(outcome.output.contains("shipped it"), "{}", outcome.output);
    let events = capture.events.lock().expect("lock");
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0].0, kinds::GOAL_DONE);
    assert_eq!(events[0].1["summary"], "shipped it");
}

#[tokio::test]
async fn the_goal_done_tool_refuses_an_empty_summary() {
    let dir = tempfile::tempdir().expect("tempdir");
    let registry = ToolRegistry::new(dir.path().to_path_buf());
    let capture = Capture::default();
    let call = ToolCall {
        id: "c1".to_owned(),
        name: "goal_done".to_owned(),
        arguments: json!({"summary": "   "}).to_string(),
    };
    let err = registry
        .execute_streaming(call, &capture)
        .await
        .expect_err("empty summary");
    assert!(err.contains("empty"), "{err}");
    assert!(capture.events.lock().expect("lock").is_empty());
}
