//! behaviour tests for the event log (spec 14, v1.0 task 2).
#![allow(clippy::expect_used, clippy::panic)]

use sa_core::store::{NewSession, Store, BLOB_THRESHOLD};
use sa_proto::session::SessionState;
use serde_json::json;

fn session(store: &Store, id: &str) -> sa_core::store::SessionRow {
    store
        .create_session(&NewSession {
            id: id.to_owned(),
            title: "test".to_owned(),
            cwd: "/tmp".to_owned(),
            repo_id: None,
            model: None,
            effort: None,
            owner_host: "host".to_owned(),
            owner_epoch: 0,
        })
        .expect("create session")
}

#[test]
fn appends_are_monotonic_and_unique() {
    let store = Store::open_in_memory().expect("store");
    session(&store, "s1");

    let a = store
        .append_event("s1", "turn.started", &json!({"turn_id": "t1"}))
        .expect("append");
    let b = store
        .append_event("s1", "llm.delta", &json!({"text": "hi"}))
        .expect("append");
    assert_eq!(a, 1);
    assert_eq!(b, 2);
    assert_eq!(store.last_seq("s1").expect("last"), 2);
}

#[test]
fn replay_from_a_cursor_returns_exactly_later_events() {
    let store = Store::open_in_memory().expect("store");
    session(&store, "s1");
    for n in 0..5 {
        store
            .append_event("s1", "llm.delta", &json!({"text": format!("d{n}")}))
            .expect("append");
    }

    let (events, more) = store.events_since("s1", 2, 100).expect("replay");
    assert!(!more);
    assert_eq!(events.len(), 3);
    assert_eq!(events.first().expect("first").seq, 3);
    assert_eq!(events.last().expect("last").seq, 5);
    assert_eq!(events[0].payload["text"], "d2");
}

#[test]
fn replay_reports_more_when_the_window_is_truncated() {
    let store = Store::open_in_memory().expect("store");
    session(&store, "s1");
    for n in 0..10 {
        store
            .append_event("s1", "llm.delta", &json!({"text": format!("d{n}")}))
            .expect("append");
    }
    let (events, more) = store.events_since("s1", 0, 4).expect("replay");
    assert!(more, "must signal a truncated window");
    assert_eq!(events.len(), 4);
}

#[test]
fn replay_is_scoped_to_one_session() {
    let store = Store::open_in_memory().expect("store");
    session(&store, "s1");
    session(&store, "s2");
    store
        .append_event("s1", "llm.delta", &json!({"text": "one"}))
        .expect("append");
    store
        .append_event("s2", "llm.delta", &json!({"text": "two"}))
        .expect("append");

    let (events, _) = store.events_since("s1", 0, 100).expect("replay");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].payload["text"], "one");
}

#[test]
fn oversized_payload_spills_to_a_blob_and_reads_back_intact() {
    let store = Store::open_in_memory().expect("store");
    session(&store, "s1");

    let big = "x".repeat(BLOB_THRESHOLD + 1024);
    store
        .append_event(
            "s1",
            "tool.completed",
            &json!({"call_id": "c1", "output": big}),
        )
        .expect("append");

    assert_eq!(store.blob_count(), 1, "payload must have spilled");

    let (events, _) = store.events_since("s1", 0, 10).expect("replay");
    assert_eq!(
        events[0].payload["output"].as_str().expect("string"),
        big,
        "blob must re-inline byte for byte"
    );
}

#[test]
fn deleted_session_is_hidden_by_default() {
    let store = Store::open_in_memory().expect("store");
    session(&store, "s1");
    store
        .set_session_state("s1", SessionState::Deleted)
        .expect("delete");

    let visible = store.list_sessions(false).expect("list");
    assert!(visible.is_empty(), "tombstone must be hidden");

    let all = store.list_sessions(true).expect("list");
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].state, SessionState::Deleted);
}

#[test]
fn open_turn_detection_finds_the_dangling_tool_call() {
    let store = Store::open_in_memory().expect("store");
    session(&store, "s1");

    store
        .append_event("s1", "turn.started", &json!({"turn_id": "t1"}))
        .expect("append");
    store
        .append_event(
            "s1",
            "tool.requested",
            &json!({"turn_id": "t1", "call_id": "c1", "tool": "bash"}),
        )
        .expect("append");
    store
        .append_event(
            "s1",
            "tool.completed",
            &json!({"turn_id": "t1", "call_id": "c1", "output": "ok"}),
        )
        .expect("append");
    store
        .append_event(
            "s1",
            "tool.requested",
            &json!({"turn_id": "t1", "call_id": "c2", "tool": "read"}),
        )
        .expect("append");

    let open = store.open_turns().expect("open turns");
    assert_eq!(open.len(), 1);
    assert_eq!(open[0].turn_id, "t1");
    assert_eq!(open[0].pending_calls, vec!["c2".to_owned()]);
}

#[test]
fn a_completed_turn_is_not_open() {
    let store = Store::open_in_memory().expect("store");
    session(&store, "s1");
    store
        .append_event("s1", "turn.started", &json!({"turn_id": "t1"}))
        .expect("append");
    store
        .append_event("s1", "turn.completed", &json!({"turn_id": "t1"}))
        .expect("append");
    assert_eq!(
        store.open_turns().expect("open turns"),
        [] as [sa_core::store::OpenTurn; 0]
    );
}

#[test]
fn a_relocated_session_is_not_scanned_for_open_turns() {
    let store = Store::open_in_memory().expect("store");
    session(&store, "s1");
    store
        .append_event("s1", "turn.started", &json!({"turn_id": "t1"}))
        .expect("append");
    store
        .set_session_state("s1", SessionState::Relocated)
        .expect("relocate");
    assert!(
        store.open_turns().expect("open turns").is_empty(),
        "a relocated session is read-only, so nothing is interrupted"
    );
}

#[test]
fn reopening_a_file_store_keeps_the_log_consistent() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("sa.db");

    {
        let store = Store::open(&path).expect("store");
        session(&store, "s1");
        store
            .append_event("s1", "turn.started", &json!({"turn_id": "t1"}))
            .expect("append");
        store
            .append_event("s1", "llm.delta", &json!({"text": "hello"}))
            .expect("append");
    }

    let store = Store::open(&path).expect("reopen");
    let (events, _) = store.events_since("s1", 0, 100).expect("replay");
    assert_eq!(events.len(), 2);
    assert_eq!(events[1].payload["text"], "hello");
    assert_eq!(store.last_seq("s1").expect("last"), 2);
}

#[test]
fn concurrent_appends_never_collide_on_seq() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("sa.db");
    let store = Store::open(&path).expect("store");
    session(&store, "s1");

    let mut handles = Vec::new();
    for worker in 0..8 {
        let store = store.clone();
        handles.push(std::thread::spawn(move || {
            for n in 0..25 {
                store
                    .append_event("s1", "llm.delta", &json!({"w": worker, "n": n}))
                    .expect("append");
            }
        }));
    }
    for handle in handles {
        handle.join().expect("join");
    }

    let (events, _) = store.events_since("s1", 0, 1000).expect("replay");
    assert_eq!(events.len(), 200);
    let seqs: Vec<u64> = events.iter().map(|e| e.seq).collect();
    let mut expected: Vec<u64> = (1..=200).collect();
    expected.sort_unstable();
    assert_eq!(seqs, expected, "seq must be dense and unique");
}
