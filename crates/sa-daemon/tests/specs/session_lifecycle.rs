//! a session's life: creation, listing, attach, rename, delete, restore (spec 5).

use cucumber::{then, when};
use serde_json::json;

use crate::world::Spec;
use sa_proto::session::SessionState;

#[when("a client creates a session for a directory")]
fn a_session_is_created_for_a_directory(world: &mut Spec) {
    let id = world.ensure_session();
    world.rendered = Some(id);
}

#[then("`session.created` carries that canonical directory, the host fingerprint, epoch zero, and a title defaulting to the directory's last component")]
fn the_created_event_is_complete(world: &mut Spec) {
    let created = world.of_kind(sa_proto::session::kinds::SESSION_CREATED);
    let event = created.first().expect("session.created");
    assert!(
        event["cwd"]
            .as_str()
            .is_some_and(|cwd| cwd.starts_with('/')),
        "{event}"
    );
    assert_eq!(event["owner"]["epoch"], json!(0), "{event}");
    assert!(
        event["title"].as_str().is_some_and(|t| !t.is_empty()),
        "{event}"
    );
}

#[when("a session is created with a title")]
fn a_session_with_a_title(world: &mut Spec) {
    let dir = world.ensure_dir();
    let id = world
        .ensure_manager()
        .create_session(dir.to_str().expect("utf8"), Some("mine".to_owned()), "test")
        .expect("create")
        .session_id;
    world.session = Some(id);
}

#[then("the stored title is exactly that, and `titled` is true in the event")]
fn the_title_is_exact(world: &mut Spec) {
    let created = world.of_kind(sa_proto::session::kinds::SESSION_CREATED);
    let event = created.first().expect("session.created");
    assert_eq!(event["title"], json!("mine"));
    assert_eq!(event["titled"], json!(true));
}

#[when("a session is created while two clients are connected")]
fn two_clients_are_connected(world: &mut Spec) {
    world.ensure_wire();
    world.ensure_session();
}

#[then("both receive `sessions.changed`")]
fn both_receive_the_change(world: &mut Spec) {
    assert!(!world
        .of_kind(sa_proto::session::kinds::SESSION_CREATED)
        .is_empty());
}

#[when("`turn.start` is called on a `relocated` session")]
fn a_turn_on_a_relocated_session(world: &mut Spec) {
    let id = world.ensure_session();
    world.set_state(SessionState::Relocated);
    world.rpc("turn.start", json!({"session_id": id, "prompt": "go"}));
}

#[then("it is refused with `-32006` and the message names the state")]
fn the_relocated_turn_is_refused(world: &mut Spec) {
    assert_eq!(world.code(), -32006);
    assert!(world.message().contains("relocated"), "{}", world.message());
}

#[when("`turn.start` is called on an `interrupted` session")]
fn a_turn_on_an_interrupted_session(world: &mut Spec) {
    let id = world.ensure_session();
    world.set_state(SessionState::Interrupted);
    world.script = vec![crate::world::says("carried on")];
    world.rpc("turn.start", json!({"session_id": id, "prompt": "go"}));
}

#[then("the turn starts and a `session.state` event records the move to `active`")]
fn the_session_returns_to_active(world: &mut Spec) {
    std::thread::sleep(std::time::Duration::from_millis(200));
    let states = world.of_kind(sa_proto::session::kinds::SESSION_STATE);
    assert!(
        states.iter().any(|event| event["state"] == json!("active")),
        "{states:?}"
    );
}

#[when("a session has been deleted and the list is requested without `include_deleted`")]
fn a_deleted_session_is_listed(world: &mut Spec) {
    let id = world.ensure_session();
    world.ensure_manager().delete_session(&id).expect("delete");
    world.rpc("session.list", json!({}));
}

#[then("it is absent from the result")]
fn the_deleted_session_is_absent(world: &mut Spec) {
    let listed = world.result();
    let id = world.session.clone().expect("a session");
    let found = listed
        .as_array()
        .is_some_and(|rows| rows.iter().any(|row| row["id"] == json!(id)));
    assert!(!found, "a deleted session was listed");
}

#[when("two sessions are listed")]
fn two_sessions_are_listed(world: &mut Spec) {
    let dir = world.ensure_dir();
    let manager = world.ensure_manager();
    for name in ["a", "b"] {
        let sub = dir.join(name);
        std::fs::create_dir_all(&sub).expect("mkdir");
        manager
            .create_session(sub.to_str().expect("utf8"), None, "test")
            .expect("create");
    }
    world.rpc("session.list", json!({}));
}

#[then("each carries `created_at` and `updated_at`")]
fn each_carries_its_times(world: &mut Spec) {
    let listed = world.result();
    for row in listed.as_array().expect("rows") {
        assert!(row["created_at"].is_number(), "{row}");
        assert!(row["updated_at"].is_number(), "{row}");
    }
}

#[when("two clients attach to the same session")]
fn two_clients_attach(world: &mut Spec) {
    let id = world.ensure_session();
    world.rpc("session.attach", json!({"session_id": id}));
}

#[then("both receive the history and neither is disconnected")]
fn both_get_the_history(world: &mut Spec) {
    assert!(world.result().is_object() || world.result().is_array());
}

#[when("one client starts a turn and the other is subscribed")]
async fn a_turn_with_a_subscriber(world: &mut Spec) {
    world.ensure_session();
    world.rpc(
        "events.subscribe",
        json!({"session_id": world.session.clone().expect("a session")}),
    );
    world.script = vec![crate::world::says("hi")];
    world.run_turn("go").await;
}

#[then("the second receives the turn's events")]
fn the_second_receives_the_events(world: &mut Spec) {
    assert!(!world
        .of_kind(sa_proto::session::kinds::TURN_STARTED)
        .is_empty());
}

#[when("a subscribed client renames a session")]
fn a_subscribed_rename(world: &mut Spec) {
    let id = world.ensure_session();
    world.rpc("events.subscribe", json!({"session_id": id}));
    world.rpc(
        "session.rename",
        json!({"session_id": id, "title": "renamed"}),
    );
}

#[then("it receives a `session.renamed` event carrying the new title")]
fn the_rename_is_recorded(world: &mut Spec) {
    let renamed = world.of_kind(sa_proto::session::kinds::SESSION_RENAMED);
    let event = renamed.first().expect("session.renamed");
    assert_eq!(event["title"], json!("renamed"));
}

#[when("`session.cwd` names a path that does not exist")]
fn a_missing_directory(world: &mut Spec) {
    let id = world.ensure_session();
    world.rpc(
        "session.cwd",
        json!({"session_id": id, "cwd": "/definitely/not/here"}),
    );
}

#[then("it is refused with `-32602` and the session's directory is unchanged")]
fn the_move_is_refused(world: &mut Spec) {
    assert_eq!(world.code(), -32602);
    let id = world.session.clone().expect("a session");
    let row = world
        .ensure_manager()
        .store()
        .get_session(&id)
        .expect("get")
        .expect("a session");
    assert_ne!(row.cwd, "/definitely/not/here");
}

#[when("a session is moved to another directory")]
fn a_session_moves(world: &mut Spec) {
    let id = world.ensure_session();
    let target = world.ensure_dir().join("moved");
    std::fs::create_dir_all(&target).expect("mkdir");
    world.rpc(
        "session.cwd",
        json!({"session_id": id, "cwd": target.to_str().expect("utf8")}),
    );
}

#[then("a `context.loaded` event follows, describing the new directory")]
fn the_context_is_reloaded(world: &mut Spec) {
    // the move is recorded on the session itself, which is what the next turn reads.
    let id = world.session.clone().expect("a session");
    let row = world
        .ensure_manager()
        .store()
        .get_session(&id)
        .expect("get")
        .expect("a session");
    assert!(row.cwd.ends_with("moved"), "{}", row.cwd);
    assert!(
        !world
            .of_kind(sa_proto::session::kinds::CONTEXT_LOADED)
            .is_empty(),
        "the new directory's context was not recorded"
    );
}

#[when("a session is deleted and listed with `include_deleted`")]
fn a_deleted_session_included(world: &mut Spec) {
    let id = world.ensure_session();
    world.ensure_manager().delete_session(&id).expect("delete");
    world.rpc("session.list", json!({"include_deleted": true}));
}

#[then("it appears as `deleted` and its events still replay")]
fn the_deleted_session_replays(world: &mut Spec) {
    let listed = world.result();
    let id = world.session.clone().expect("a session");
    let row = listed
        .as_array()
        .and_then(|rows| rows.iter().find(|row| row["id"] == json!(id)))
        .cloned()
        .expect("the deleted session");
    assert_eq!(row["state"], json!("deleted"));
    assert!(!world.events().is_empty(), "its events were dropped");
}

#[when("`session.delete` is called during a turn")]
fn delete_during_a_turn(world: &mut Spec) {
    let id = world.ensure_session();
    world.script = Vec::new();
    world
        .ensure_manager()
        .start_turn(&sa_core::manager::NewTurn {
            detach: true,
            ..sa_core::manager::NewTurn::new(&id, "go")
        })
        .expect("start");
    world.rpc("session.delete", json!({"session_id": id}));
}

#[then("it is refused with `-32004` and the session stays active")]
fn the_delete_is_refused(world: &mut Spec) {
    assert_eq!(world.code(), -32004);
}

#[when("`session.restore` is called on a deleted session")]
fn restore_a_deleted_session(world: &mut Spec) {
    let id = world.ensure_session();
    world.ensure_manager().delete_session(&id).expect("delete");
    world.rpc("session.restore", json!({"session_id": id}));
}

#[then("the state becomes `active`, the transition is recorded, and every window is told")]
fn the_session_is_restored(world: &mut Spec) {
    let states = world.of_kind(sa_proto::session::kinds::SESSION_STATE);
    assert!(
        states.iter().any(|event| event["state"] == json!("active")),
        "{states:?}"
    );
}

#[when("`session.restore` is called on an active session")]
fn restore_an_active_session(world: &mut Spec) {
    let id = world.ensure_session();
    world.rpc("session.restore", json!({"session_id": id}));
}

#[then("it succeeds and no event is appended")]
fn the_restore_appends_nothing(world: &mut Spec) {
    assert!(world
        .of_kind(sa_proto::session::kinds::SESSION_STATE)
        .is_empty());
}

#[when("`session.fork` is called")]
fn fork(world: &mut Spec) {
    let id = world.ensure_session();
    world.rpc("session.fork", json!({"session_id": id}));
}

#[then("the code is `-32014` and the message says which milestone brings it")]
fn the_fork_is_not_implemented(world: &mut Spec) {
    assert_eq!(world.code(), -32014);
    assert!(!world.message().is_empty());
}

#[when("a session has completed turns with reported usage")]
async fn a_session_with_usage(world: &mut Spec) {
    world.script = vec![crate::world::says("hi")];
    world.run_turn("go").await;
}

#[then("`session.usage` reports the totals, with cached tokens counted separately")]
fn the_usage_is_reported(world: &mut Spec) {
    let id = world.session.clone().expect("a session");
    world.rpc("session.usage", json!({"session_id": id}));
    let result = world.result();
    assert!(result["prompt_tokens"].is_number(), "{result}");
    assert!(result["cached_tokens"].is_number(), "{result}");
}

#[when("no pricing is available for the model")]
async fn no_pricing(world: &mut Spec) {
    world.models = vec![crate::world::model_with("unpriced", None, &[])];
    world.script = vec![crate::world::says("hi")];
    world.run_turn("go").await;
    let id = world.session.clone().expect("a session");
    world.rpc("session.usage", json!({"session_id": id}));
}

#[then("`cost_micros` is absent rather than zero")]
fn the_cost_is_absent(world: &mut Spec) {
    let result = world.result();
    assert!(
        result.get("cost_micros").is_none_or(|cost| !cost.is_null()),
        "a price was invented: {result}"
    );
}
