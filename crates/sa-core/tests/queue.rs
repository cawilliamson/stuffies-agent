//! messages queued while a turn runs (spec 4.1).
#![allow(clippy::expect_used, clippy::panic)]

use std::sync::{Arc, Mutex};

use sa_core::llm::{ChatRequest, Delta, ModelsFuture, Provider, StreamFuture};
use sa_core::manager::{Manager, ManagerConfig};
use sa_core::store::Store;
use sa_proto::session::kinds;
use tokio::sync::{mpsc, Notify};

/// a provider that holds each stream open until the test releases it, so a turn is
/// genuinely running while the test queues messages into it.
struct GatedProvider {
    /// fired when a stream starts, so the test knows the turn is live.
    entered: Arc<Notify>,
    /// fired by the test to let the current stream finish.
    release: Arc<Notify>,
    seen: Mutex<Vec<ChatRequest>>,
}

impl GatedProvider {
    fn new() -> Self {
        Self {
            entered: Arc::new(Notify::new()),
            release: Arc::new(Notify::new()),
            seen: Mutex::new(Vec::new()),
        }
    }
}

impl Provider for GatedProvider {
    fn list_models(&self) -> ModelsFuture<'_> {
        Box::pin(async { Ok(Vec::new()) })
    }

    fn stream(&self, request: ChatRequest) -> StreamFuture<'_> {
        if let Ok(mut seen) = self.seen.lock() {
            seen.push(request);
        }
        let entered = Arc::clone(&self.entered);
        let release = Arc::clone(&self.release);
        Box::pin(async move {
            let (tx, rx) = mpsc::channel(8);
            tokio::spawn(async move {
                entered.notify_one();
                release.notified().await;
                let _ = tx
                    .send(Delta::Done {
                        stop_reason: "stop".to_owned(),
                        prompt_tokens: 1,
                        completion_tokens: 1,
                    })
                    .await;
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
            context_limit: None,
            reasoning_effort: None,
            lock_dir: dir.join("locks"),
            turn_timeout_secs: 30,
            aux_budget_secs: 5,
            models: Arc::new(Mutex::new(Vec::new())),
            mcp: Arc::new(sa_core::mcp::Hub::empty()),
        }),
    ))
}

fn session(manager: &Arc<Manager>, dir: &std::path::Path) -> String {
    manager
        .create_session(&dir.display().to_string(), Some("t".to_owned()), "test")
        .expect("session")
        .session_id
}

/// every payload of one event kind, in order.
fn payloads(manager: &Arc<Manager>, session_id: &str, kind: &str) -> Vec<serde_json::Value> {
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

async fn start(manager: &Arc<Manager>, provider: &Arc<GatedProvider>, session_id: &str) {
    manager
        .start_turn(session_id, "do the thing", &[], false, None)
        .expect("turn");
    // the turn is only live once the provider has been entered.
    provider.entered.notified().await;
}

#[tokio::test]
async fn steering_is_refused_when_nothing_is_running() {
    let dir = tempfile::tempdir().expect("tempdir");
    let provider = Arc::new(GatedProvider::new());
    let manager = manager(dir.path(), &provider);
    let session_id = session(&manager, dir.path());

    assert!(manager.steer(&session_id, "too early").is_err());
    assert!(manager.follow_up(&session_id, "too early").is_err());
}

#[tokio::test]
async fn a_steering_message_is_queued_until_the_turn_takes_it() {
    let dir = tempfile::tempdir().expect("tempdir");
    let provider = Arc::new(GatedProvider::new());
    let manager = manager(dir.path(), &provider);
    let session_id = session(&manager, dir.path());
    start(&manager, &provider, &session_id).await;

    manager
        .steer(&session_id, "stop and run the tests")
        .expect("steer");

    let queued = payloads(&manager, &session_id, kinds::TURN_QUEUED);
    let last = queued.last().expect("a queue event");
    assert_eq!(
        last["steering"],
        serde_json::json!(["stop and run the tests"]),
        "the queue event carries the complete queue"
    );
    assert_eq!(last["follow_up"], serde_json::json!([]));

    provider.release.notify_one();
}

#[tokio::test]
async fn dequeue_hands_everything_back_and_empties_the_queue() {
    let dir = tempfile::tempdir().expect("tempdir");
    let provider = Arc::new(GatedProvider::new());
    let manager = manager(dir.path(), &provider);
    let session_id = session(&manager, dir.path());
    start(&manager, &provider, &session_id).await;

    manager.steer(&session_id, "one").expect("steer");
    manager.follow_up(&session_id, "two").expect("follow up");

    let taken = manager.dequeue(&session_id).expect("dequeue");
    assert_eq!(taken.steering, vec!["one".to_owned()]);
    assert_eq!(taken.follow_up, vec!["two".to_owned()]);

    let again = manager.dequeue(&session_id).expect("dequeue");
    assert_eq!(again.steering.len(), 0);
    assert_eq!(again.follow_up.len(), 0);

    let queued = payloads(&manager, &session_id, kinds::TURN_QUEUED);
    let last = queued.last().expect("a queue event");
    assert_eq!(
        last["steering"],
        serde_json::json!([]),
        "taking the queue back is recorded, so a reattached client sees it empty"
    );

    provider.release.notify_one();
}

#[tokio::test]
async fn a_follow_up_runs_as_the_next_turn_after_this_one() {
    let dir = tempfile::tempdir().expect("tempdir");
    let provider = Arc::new(GatedProvider::new());
    let manager = manager(dir.path(), &provider);
    let session_id = session(&manager, dir.path());
    start(&manager, &provider, &session_id).await;

    manager
        .follow_up(&session_id, "and then summarise")
        .expect("follow up");

    // release the first turn; the queued follow-up must start a second one.
    provider.release.notify_one();
    provider.entered.notified().await;

    let started = payloads(&manager, &session_id, kinds::TURN_STARTED);
    assert_eq!(started.len(), 2, "the follow-up starts its own turn");
    assert_eq!(
        started[1]["prompt"],
        serde_json::json!("and then summarise")
    );

    provider.release.notify_one();
}

#[tokio::test]
async fn a_cancelled_turn_still_runs_what_was_queued_for_after_it() {
    let dir = tempfile::tempdir().expect("tempdir");
    let provider = Arc::new(GatedProvider::new());
    let manager = manager(dir.path(), &provider);
    let session_id = session(&manager, dir.path());
    start(&manager, &provider, &session_id).await;

    manager
        .follow_up(&session_id, "carry on")
        .expect("follow up");
    manager.cancel_turn(&session_id).expect("cancel");
    provider.release.notify_one();
    provider.entered.notified().await;

    let started = payloads(&manager, &session_id, kinds::TURN_STARTED);
    assert_eq!(
        started.len(),
        2,
        "the queue outlives the turn it was typed in"
    );

    provider.release.notify_one();
}

#[tokio::test]
async fn a_steered_message_reaches_the_model_at_the_point_it_was_taken() {
    let dir = tempfile::tempdir().expect("tempdir");
    let provider = Arc::new(GatedProvider::new());
    let manager = manager(dir.path(), &provider);
    let session_id = session(&manager, dir.path());
    start(&manager, &provider, &session_id).await;

    manager
        .steer(&session_id, "actually, read main.rs")
        .expect("steer");
    let taken = manager.dequeue(&session_id).expect("dequeue");
    assert_eq!(taken.steering, vec!["actually, read main.rs".to_owned()]);
    assert_eq!(
        payloads(&manager, &session_id, kinds::TURN_STEERED).len(),
        0,
        "taking a message back means it was never delivered"
    );

    provider.release.notify_one();
}

#[tokio::test]
async fn one_queued_message_can_be_dropped_by_index() {
    let dir = tempfile::tempdir().expect("tempdir");
    let provider = Arc::new(GatedProvider::new());
    let manager = manager(dir.path(), &provider);
    let session_id = session(&manager, dir.path());
    start(&manager, &provider, &session_id).await;

    manager.steer(&session_id, "keep").expect("steer");
    manager.steer(&session_id, "drop me").expect("steer");

    let state = manager
        .drop_queued(&session_id, "steering", 1)
        .expect("drop");
    assert_eq!(state.steering, vec!["keep".to_owned()]);
    assert_eq!(state.follow_up.len(), 0);

    let queued = payloads(&manager, &session_id, kinds::TURN_QUEUED);
    let last = queued.last().expect("a queue event");
    assert_eq!(
        last["steering"],
        serde_json::json!(["keep"]),
        "dropping one is recorded, so every client sees the same queue"
    );

    provider.release.notify_one();
}

#[tokio::test]
async fn dropping_past_the_end_is_not_an_error() {
    // the message may have been delivered between the client drawing the row and the key
    // landing; the caller only wanted it gone.
    let dir = tempfile::tempdir().expect("tempdir");
    let provider = Arc::new(GatedProvider::new());
    let manager = manager(dir.path(), &provider);
    let session_id = session(&manager, dir.path());
    start(&manager, &provider, &session_id).await;

    manager.steer(&session_id, "only").expect("steer");
    let state = manager
        .drop_queued(&session_id, "steering", 7)
        .expect("drop");
    assert_eq!(state.steering, vec!["only".to_owned()]);

    provider.release.notify_one();
}

#[tokio::test]
async fn an_unknown_queue_name_is_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let provider = Arc::new(GatedProvider::new());
    let manager = manager(dir.path(), &provider);
    let session_id = session(&manager, dir.path());
    start(&manager, &provider, &session_id).await;

    assert!(manager.drop_queued(&session_id, "later", 0).is_err());

    provider.release.notify_one();
}

#[tokio::test]
async fn a_message_queued_as_the_turn_ends_still_runs() {
    // the model had already finished its answer when the operator typed: the message must
    // become the next turn rather than sitting in the queue forever.
    let dir = tempfile::tempdir().expect("tempdir");
    let provider = Arc::new(GatedProvider::new());
    let manager = manager(dir.path(), &provider);
    let session_id = session(&manager, dir.path());
    start(&manager, &provider, &session_id).await;

    manager
        .steer(&session_id, "actually, answer this too")
        .expect("steer");

    provider.release.notify_one();
    provider.entered.notified().await;

    let started = payloads(&manager, &session_id, kinds::TURN_STARTED);
    assert_eq!(
        started.len(),
        2,
        "the stranded message starts a turn of its own"
    );
    assert_eq!(
        started[1]["prompt"],
        serde_json::json!("actually, answer this too")
    );

    provider.release.notify_one();
}

#[tokio::test]
async fn delivering_a_queued_message_takes_it_off_the_queue() {
    // the bug: a message queued after the model's last round-trip was delivered as a new
    // turn, but the queue event was never appended, so it stayed on screen as though it
    // were still waiting.
    let dir = tempfile::tempdir().expect("tempdir");
    let provider = Arc::new(GatedProvider::new());
    let manager = manager(dir.path(), &provider);
    let session_id = session(&manager, dir.path());
    start(&manager, &provider, &session_id).await;

    manager.steer(&session_id, "count to 5").expect("steer");
    provider.release.notify_one();
    provider.entered.notified().await;

    let queued = payloads(&manager, &session_id, kinds::TURN_QUEUED);
    let last = queued.last().expect("a queue event");
    assert_eq!(
        last["steering"],
        serde_json::json!([]),
        "the delivered message leaves the queue"
    );
    assert_eq!(last["follow_up"], serde_json::json!([]));

    provider.release.notify_one();
}
