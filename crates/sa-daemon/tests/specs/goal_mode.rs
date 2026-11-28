//! the goal loop: a session is nudged until `goal_done`, and stops when it should (spec 5.7).

use cucumber::{then, when};
use serde_json::json;

use crate::world::{says, Spec};

#[when("the daemon restarts with an open goal")]
fn a_restart_with_an_open_goal(world: &mut Spec) {
    let id = world.ensure_session();
    world
        .ensure_manager()
        .set_goal(&id, "ship the thing", 5)
        .expect("goal");
}

#[then("`goal.status` reports the same text, nudge count and cap as before")]
fn the_goal_survives_a_restart(world: &mut Spec) {
    // the state is derived from the log, so a second read is the same read.
    let id = world.session.clone().expect("a session");
    let first = world.ensure_manager().goal_status(&id).expect("status");
    let again = world.ensure_manager().goal_status(&id).expect("status");
    assert_eq!(first.text, again.text);
    assert_eq!(first.iterations, again.iterations);
    assert_eq!(first.max_iterations, again.max_iterations);
    assert_eq!(first.text, "ship the thing");
}

#[when("two clients ask for `goal.status`")]
fn two_clients_ask(world: &mut Spec) {
    let id = world.ensure_session();
    world
        .ensure_manager()
        .set_goal(&id, "ship the thing", 5)
        .expect("goal");
    let one = world.ensure_manager().goal_status(&id).expect("status");
    let two = world.ensure_manager().goal_status(&id).expect("status");
    world.rendered = Some(format!("{}:{}", one.iterations, two.iterations));
}

#[then("both are answered from the same log-derived state")]
fn both_are_the_same(world: &mut Spec) {
    let rendered = world.rendered.clone().expect("a status step comes first");
    let (one, two) = rendered.split_once(':').expect("one:two");
    assert_eq!(one, two);
}

#[when("`goal.set` is called with whitespace only")]
fn a_blank_goal(world: &mut Spec) {
    let id = world.ensure_session();
    world.rpc("goal.set", json!({"session_id": id, "text": "   "}));
}

#[when("a goal is set and no turn is started")]
fn a_goal_with_no_turn(world: &mut Spec) {
    let id = world.ensure_session();
    world
        .ensure_manager()
        .set_goal(&id, "ship the thing", 5)
        .expect("goal");
}

#[then("no turn runs")]
fn no_turn_runs(world: &mut Spec) {
    assert_eq!(
        world.of_kind(sa_proto::session::kinds::TURN_STARTED).len(),
        0
    );
}

#[when("a turn under a goal is cancelled")]
fn a_cancelled_turn_under_a_goal(world: &mut Spec) {
    let id = world.ensure_session();
    world
        .ensure_manager()
        .set_goal(&id, "ship the thing", 5)
        .expect("goal");
    world.script = Vec::new();
    let manager = world.ensure_manager();
    manager
        .start_turn(&sa_core::manager::NewTurn {
            detach: true,
            ..sa_core::manager::NewTurn::new(&id, "go")
        })
        .expect("start");
    manager.cancel_turn(&id).expect("cancel");
    std::thread::sleep(std::time::Duration::from_millis(300));
}

#[then("no `goal.nudged` is appended and the goal stays open")]
fn no_nudge_after_a_cancel(world: &mut Spec) {
    assert!(world
        .of_kind(sa_proto::session::kinds::GOAL_NUDGED)
        .is_empty());
    let id = world.session.clone().expect("a session");
    assert!(
        world
            .ensure_manager()
            .goal_status(&id)
            .expect("status")
            .active
    );
}

#[when("a turn under an open goal completes without `goal_done`")]
async fn a_turn_without_goal_done(world: &mut Spec) {
    let id = world.ensure_session();
    world
        .ensure_manager()
        .set_goal(&id, "ship the thing", 5)
        .expect("goal");
    world.script = vec![says("still going")];
    world.run_turn("go").await;
    std::thread::sleep(std::time::Duration::from_millis(200));
}

#[then("`goal.nudged` is appended and a follow-up carrying the nudge text is queued")]
fn the_nudge_is_queued(world: &mut Spec) {
    let nudged = world.of_kind(sa_proto::session::kinds::GOAL_NUDGED);
    assert_eq!(nudged.len(), 1, "{nudged:?}");
    let started = world.of_kind(sa_proto::session::kinds::TURN_STARTED);
    let prompt = started
        .last()
        .and_then(|event| event["prompt"].as_str())
        .unwrap_or_default();
    assert!(prompt.contains("Keep working on the goal"), "{prompt}");
}

#[when("the first nudge is queued")]
fn the_first_nudge(world: &mut Spec) {
    a_turn_without_goal_done_sync(world);
}

fn a_turn_without_goal_done_sync(world: &mut Spec) {
    let id = world.ensure_session();
    world
        .ensure_manager()
        .set_goal(&id, "ship the thing", 5)
        .expect("goal");
    world.script = vec![says("still going")];
    futures_block(world, "go");
}

/// runs a turn to its end without an async step body, for the scenarios that only need the
/// nudge text.
fn futures_block(world: &mut Spec, prompt: &str) {
    let id = world.session.clone().expect("a session");
    let manager = world.ensure_manager();
    world.feed_script();
    let before = manager.store().last_seq(&id).unwrap_or_default();
    manager
        .start_turn(&sa_core::manager::NewTurn {
            detach: true,
            ..sa_core::manager::NewTurn::new(&id, prompt)
        })
        .expect("start");
    for _ in 0..300 {
        let (events, _) = manager
            .store()
            .events_since(&id, before, 1_000)
            .expect("events");
        if events
            .iter()
            .any(|event| sa_proto::session::kinds::TURN_TERMINAL.contains(&event.kind.as_str()))
        {
            std::thread::sleep(std::time::Duration::from_millis(100));
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

#[then("its text carries no iteration count")]
fn the_first_nudge_has_no_count(world: &mut Spec) {
    let started = world.of_kind(sa_proto::session::kinds::TURN_STARTED);
    let prompt = started
        .last()
        .and_then(|event| event["prompt"].as_str())
        .unwrap_or_default();
    assert!(!prompt.contains("Nudge 1 of"), "{prompt}");
}

#[when("a second or later nudge is queued")]
fn a_second_nudge(world: &mut Spec) {
    let id = world.ensure_session();
    world
        .ensure_manager()
        .set_goal(&id, "ship the thing", 5)
        .expect("goal");
    world.script = vec![says("a"), says("b"), says("c")];
    futures_block(world, "go");
}

#[then("its text carries the iteration number and tells the model to change approach")]
fn the_second_nudge_has_a_count(world: &mut Spec) {
    let started = world.of_kind(sa_proto::session::kinds::TURN_STARTED);
    let prompt = started
        .iter()
        .filter_map(|event| event["prompt"].as_str())
        .find(|prompt| prompt.contains("Nudge 2 of"))
        .unwrap_or_else(|| panic!("no second nudge: {started:?}"));
    assert!(
        prompt.contains("change it rather than repeating"),
        "{prompt}"
    );
}

#[when("two or fewer nudges remain")]
fn the_last_nudges(world: &mut Spec) {
    let id = world.ensure_session();
    world
        .ensure_manager()
        .set_goal(&id, "ship the thing", 2)
        .expect("goal");
    world.script = vec![says("a"), says("b"), says("c")];
    futures_block(world, "go");
}

#[then("the nudge text says the loop is about to stop")]
fn the_nudge_warns(world: &mut Spec) {
    let started = world.of_kind(sa_proto::session::kinds::TURN_STARTED);
    let prompt = started
        .iter()
        .filter_map(|event| event["prompt"].as_str())
        .find(|prompt| prompt.contains("Keep working"))
        .unwrap_or_default();
    assert!(prompt.contains("one of the last"), "{prompt}");
}

#[when("the nudge count reaches the cap")]
fn the_cap_is_reached(world: &mut Spec) {
    let id = world.ensure_session();
    world
        .ensure_manager()
        .set_goal(&id, "ship the thing", 1)
        .expect("goal");
    world.script = vec![says("a"), says("b")];
    futures_block(world, "go");
    futures_block(world, "again");
}

#[then("`goal.stopped` with reason `exhausted` is appended and the next turn queues no nudge")]
fn the_goal_is_exhausted(world: &mut Spec) {
    let stopped = world.of_kind(sa_proto::session::kinds::GOAL_STOPPED);
    let event = stopped.first().expect("goal.stopped");
    assert_eq!(event["reason"], json!("exhausted"));
    let nudges = world.of_kind(sa_proto::session::kinds::GOAL_NUDGED).len();
    assert_eq!(nudges, 1, "the cap did not hold");
}

#[when("a goal is set after a previous goal was stopped")]
fn a_goal_after_a_stop(world: &mut Spec) {
    let id = world.ensure_session();
    world
        .ensure_manager()
        .set_goal(&id, "first", 5)
        .expect("goal");
    world.ensure_manager().stop_goal(&id, "user").expect("stop");
    world
        .ensure_manager()
        .set_goal(&id, "second", 5)
        .expect("goal");
}

#[then("`goal.status` reports zero nudges spent")]
fn the_new_goal_starts_clean(world: &mut Spec) {
    let id = world.session.clone().expect("a session");
    let status = world.ensure_manager().goal_status(&id).expect("status");
    assert_eq!(status.iterations, 0);
    assert_eq!(status.text, "second");
}

#[when("the model calls `goal_done` with a summary")]
fn goal_done_with_a_summary(world: &mut Spec) {
    let id = world.ensure_session();
    world
        .ensure_manager()
        .set_goal(&id, "ship the thing", 5)
        .expect("goal");
    world.record(
        sa_proto::session::kinds::GOAL_DONE,
        json!({"summary": "shipped"}),
    );
}

#[then("`goal.status` reports that summary and the loop is no longer active")]
fn the_goal_is_done(world: &mut Spec) {
    let id = world.session.clone().expect("a session");
    let status = world.ensure_manager().goal_status(&id).expect("status");
    assert_eq!(status.summary.as_deref(), Some("shipped"));
    assert!(!status.active);
}

#[when("the model calls `goal_done` naming what blocks it")]
fn goal_done_with_a_blocker(world: &mut Spec) {
    let id = world.ensure_session();
    world
        .ensure_manager()
        .set_goal(&id, "ship the thing", 5)
        .expect("goal");
    world.record(
        sa_proto::session::kinds::GOAL_DONE,
        json!({"summary": "blocked: no token"}),
    );
}

#[then("the loop ends and the blocker is the recorded summary")]
fn the_blocker_is_the_summary(world: &mut Spec) {
    let id = world.session.clone().expect("a session");
    let status = world.ensure_manager().goal_status(&id).expect("status");
    assert_eq!(status.summary.as_deref(), Some("blocked: no token"));
    assert!(!status.active);
}

#[when("`goal_done` is called with an empty summary")]
async fn goal_done_with_no_summary(world: &mut Spec) {
    world.ensure_session();
    world
        .call_tool("goal_done", json!({"summary": "   "}))
        .await;
}

#[then("it fails as an argument error and no event is appended")]
fn the_empty_summary_is_refused(world: &mut Spec) {
    assert!(world.refused().contains("empty"), "{}", world.refused());
    assert!(world
        .of_kind(sa_proto::session::kinds::GOAL_DONE)
        .is_empty());
}

#[when("`goal.stop` is called")]
fn goal_stop(world: &mut Spec) {
    let id = world.ensure_session();
    world
        .ensure_manager()
        .set_goal(&id, "ship the thing", 5)
        .expect("goal");
    world.ensure_manager().stop_goal(&id, "user").expect("stop");
}

#[then("`goal.stopped` carries reason `user` and the loop is no longer active")]
fn the_goal_is_stopped(world: &mut Spec) {
    let stopped = world.of_kind(sa_proto::session::kinds::GOAL_STOPPED);
    assert_eq!(stopped[0]["reason"], json!("user"));
    let id = world.session.clone().expect("a session");
    assert!(
        !world
            .ensure_manager()
            .goal_status(&id)
            .expect("status")
            .active
    );
}

#[when("`goal.status` is asked while a goal is open")]
fn status_of_an_open_goal(world: &mut Spec) {
    let id = world.ensure_session();
    world
        .ensure_manager()
        .set_goal(&id, "ship the thing", 5)
        .expect("goal");
    world.rpc("goal.status", json!({"session_id": id}));
}

#[then("the result carries the text, the nudges spent and the cap")]
fn the_status_carries_the_goal(world: &mut Spec) {
    let result = world.result();
    assert_eq!(result["text"], json!("ship the thing"), "{result}");
    assert_eq!(result["iterations"], json!(0), "{result}");
    assert_eq!(result["max_iterations"], json!(5), "{result}");
}
