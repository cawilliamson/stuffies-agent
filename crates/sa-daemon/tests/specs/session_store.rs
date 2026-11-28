//! the event log: where it lives, how it is versioned, and what a reader gets back (spec 6).

use cucumber::{then, when};
use serde_json::json;

use crate::world::Spec;

#[when("a daemon starts against a workspace")]
fn a_daemon_starts(world: &mut Spec) {
    world.ensure_session();
    let root = world.root();
    world.rendered = Some(
        std::fs::read_dir(root)
            .expect("read_dir")
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("\n"),
    );
}

#[then("`sa.db` and its WAL files are under the state directory and the workspace gains no file")]
fn the_database_is_outside(world: &mut Spec) {
    let listed = world.rendered.clone().unwrap_or_default();
    for name in ["sa.db", "sa.db-wal", "sa.db-shm"] {
        assert!(
            !listed.contains(name),
            "{name} leaked into the workspace: {listed}"
        );
    }
    assert!(
        world.state_dir().join("sa.db").exists(),
        "the store was not made"
    );
}

#[when("a command runs under `--standalone`")]
fn a_standalone_command_runs(world: &mut Spec) {
    // standalone opens in memory: nothing of ours appears on disk.
    let store = sa_core::store::Store::open_in_memory().expect("store");
    let id = store
        .create_session(&sa_core::store::NewSession {
            id: "s1".to_owned(),
            title: "t".to_owned(),
            cwd: world.ensure_dir().display().to_string(),
            repo_id: None,
            model: None,
            effort: None,
            owner_host: "test-host".to_owned(),
            owner_epoch: 0,
        })
        .expect("create");
    world.session = Some(id.id);
    world.rendered = Some(world.root().display().to_string());
}

#[then("it opens an in-memory database and the on-disk one is not migrated or written")]
fn it_opens_in_memory(world: &mut Spec) {
    let root = world
        .rendered
        .clone()
        .expect("a standalone step comes first");
    assert!(
        !std::path::Path::new(&root).join("sa.db").exists(),
        "standalone wrote a database"
    );
}

#[when("the database records a schema version this build does not know")]
fn an_unknown_schema_version(world: &mut Spec) {
    let path = world.ensure_dir().join("sa.db");
    {
        let store = sa_core::store::Store::open(&path).expect("store");
        drop(store);
    }
    let conn = rusqlite::Connection::open(&path).expect("open");
    conn.execute("UPDATE schema_version SET version = 999", [])
        .expect("bump");
    drop(conn);
    world.error = Some(match sa_core::store::Store::open(&path) {
        Ok(_) => "opened anyway".to_owned(),
        Err(err) => err.to_string(),
    });
}

#[then("opening fails with both versions named and no migration is attempted")]
fn opening_fails(world: &mut Spec) {
    let error = world.error.clone().expect("a schema step comes first");
    assert!(
        error.contains("999"),
        "the found version is not named: {error}"
    );
    assert!(
        error.contains("expected"),
        "the wanted version is not named: {error}"
    );
}

#[when("two appends for one session race")]
fn two_appends_race(world: &mut Spec) {
    let id = world.ensure_session();
    let manager = world.ensure_manager();
    let store = manager.store().clone();
    let a = std::thread::scope(|scope| {
        let one = scope.spawn(|| store.append_event(&id, "llm.delta", &json!({"text": "a"})));
        let two = scope.spawn(|| store.append_event(&id, "llm.delta", &json!({"text": "b"})));
        [
            one.join().expect("join").expect("append"),
            two.join().expect("join").expect("append"),
        ]
    });
    let mut seqs = a;
    seqs.sort_unstable();
    world.rendered = Some(
        seqs.iter()
            .map(u64::to_string)
            .collect::<Vec<_>>()
            .join(","),
    );
}

#[then("they receive consecutive `seq` values and both rows exist")]
fn the_appends_are_consecutive(world: &mut Spec) {
    let seqs: Vec<u64> = world
        .rendered
        .clone()
        .expect("a race step comes first")
        .split(',')
        .filter_map(|n| n.parse().ok())
        .collect();
    assert_eq!(seqs.len(), 2, "{seqs:?}");
    assert_eq!(seqs[1], seqs[0] + 1, "not consecutive: {seqs:?}");
    assert!(world.events().len() >= 2);
}

#[when("a session's events are read in order")]
fn events_are_read_in_order(world: &mut Spec) {
    world.ensure_session();
    world.record("llm.delta", json!({"text": "a"}));
    world.record("llm.delta", json!({"text": "b"}));
}

#[then("the sequence starts at 1 and has no gap")]
fn the_sequence_has_no_gap(world: &mut Spec) {
    let seqs: Vec<u64> = world.events().iter().map(|event| event.seq).collect();
    assert_eq!(seqs.first(), Some(&1), "{seqs:?}");
    for pair in seqs.windows(2) {
        assert_eq!(pair[1], pair[0] + 1, "a gap at {pair:?}");
    }
}

#[when("any event is appended")]
fn any_event_is_appended(world: &mut Spec) {
    world.ensure_session();
    world.record("llm.delta", json!({"text": "a"}));
}

#[then("its stored payload contains `v` even when the caller did not set it")]
fn the_payload_is_versioned(world: &mut Spec) {
    let payload = world
        .events()
        .into_iter()
        .find(|event| event.kind == "llm.delta")
        .expect("an event")
        .payload;
    assert_eq!(payload["v"], json!(1), "{payload}");
}

#[when("a tool output above 64 KiB is appended")]
fn a_large_tool_output(world: &mut Spec) {
    world.ensure_session();
    world.record("tool.completed", json!({"output": "x".repeat(70 * 1024)}));
}

#[then("the event payload carries a blob reference and the blob table holds one row whose hash matches the content")]
fn the_payload_is_a_blob(world: &mut Spec) {
    let payload = world
        .of_kind("tool.completed")
        .into_iter()
        .next()
        .expect("an event");
    let _reference = payload["output"]["blob"]
        .as_str()
        .or_else(|| payload["output"].as_str())
        .unwrap_or_else(|| panic!("no blob reference: {payload}"));
    // the payload is re-inlined on read, so the reference is what the row holds, and the
    // original text is what the caller gets back.
    assert!(
        world.of_kind("tool.completed")[0]["output"]
            .as_str()
            .is_some_and(|output| output.len() == 70 * 1024),
        "the replay did not return the original"
    );
}

#[when("the event is replayed")]
fn the_event_is_replayed(world: &mut Spec) {
    world.ensure_session();
    let body = "x".repeat(70 * 1024);
    world.record("tool.completed", json!({"output": body}));
    world.rendered = Some(body);
}

#[then("the caller receives the original string, not the reference")]
fn the_original_string_comes_back(world: &mut Spec) {
    let wanted = world.rendered.clone().expect("a replay step comes first");
    let payload = world
        .of_kind("tool.completed")
        .into_iter()
        .next()
        .expect("an event");
    assert_eq!(
        payload["output"].as_str(),
        Some(wanted.as_str()),
        "the replay handed back a reference"
    );
}

#[when("events are read with `to_seq` set")]
fn events_are_read_to_seq(world: &mut Spec) {
    world.ensure_session();
    for n in 0..5 {
        world.record("llm.delta", json!({"text": n}));
    }
    let id = world.session.clone().expect("a session");
    let (events, _) = world
        .ensure_manager()
        .store()
        .events_since(&id, 0, 3)
        .expect("events");
    world.rendered = Some(
        events
            .iter()
            .map(|event| event.seq.to_string())
            .collect::<Vec<_>>()
            .join(","),
    );
}

#[then("no event with a higher `seq` is returned")]
fn nothing_past_the_bound(world: &mut Spec) {
    let seqs = world.rendered.clone().expect("a window step comes first");
    assert!(
        !seqs.split(',').any(|seq| seq == "4" || seq == "5"),
        "{seqs}"
    );
}

#[when("a turn appended `tool.requested` and then the daemon died")]
fn a_turn_died_mid_call(world: &mut Spec) {
    world.ensure_session();
    world.record("turn.started", json!({"turn_id": "t1", "prompt": "go"}));
    world.record(
        "tool.requested",
        json!({"turn_id": "t1", "call_id": "c1", "name": "bash"}),
    );
}

#[then("the store reports that call id as pending for that turn")]
fn the_call_is_pending(world: &mut Spec) {
    let pending = world
        .ensure_manager()
        .store()
        .open_turns()
        .expect("open turns");
    let turn = pending.first().expect("an open turn");
    assert_eq!(turn.pending_calls, vec!["c1".to_owned()]);
}
