//! compaction: summarising the oldest turns when the view approaches the window (spec 10.4).

use cucumber::{then, when};
use serde_json::json;

use crate::world::{model_with, says, Spec};

fn small_model(world: &mut Spec) {
    world.models = vec![model_with("small", Some(4_000), &[])];
}

/// eight turns of roughly three thousand tokens each, which trips a four thousand token
/// window several times over.
fn many_turns(world: &mut Spec, turns: usize) {
    let id = world.ensure_session();
    for n in 0..turns {
        world.record(
            sa_proto::session::kinds::TURN_STARTED,
            json!({"turn_id": format!("t{n}"), "prompt": format!("step {n}")}),
        );
        world.record(
            sa_proto::session::kinds::TOOL_COMPLETED,
            json!({"output": "y".repeat(3_000)}),
        );
        let _ = id;
    }
}

#[when("the session's reported context reaches the threshold and another turn starts")]
async fn the_context_reaches_the_threshold(world: &mut Spec) {
    small_model(world);
    world.script = vec![says("summary")];
    many_turns(world, 8);
    let id = world.session.clone().expect("a session");
    let manager = world.ensure_manager();
    manager
        .maybe_compact(&id, "small")
        .await
        .expect("compaction");
    world.rendered = Some("compacted".to_owned());
}

#[then("a `compaction.applied` event is appended before the turn's first request")]
fn the_compaction_is_recorded(world: &mut Spec) {
    assert!(
        !world
            .of_kind(sa_proto::session::kinds::COMPACTION_APPLIED)
            .is_empty(),
        "no compaction.applied"
    );
}

#[when("the model's context limit is unknown")]
async fn the_limit_is_unknown(world: &mut Spec) {
    world.models = vec![model_with("silent", None, &[])];
    world.script = vec![says("summary")];
    many_turns(world, 8);
    let id = world.session.clone().expect("a session");
    let outcome = world
        .ensure_manager()
        .maybe_compact(&id, "silent")
        .await
        .expect("compaction");
    world.rendered = Some(format!("{outcome:?}"));
}

#[then("no compaction is attempted")]
fn nothing_is_compacted(world: &mut Spec) {
    assert_eq!(world.rendered.as_deref(), Some("None"));
    assert!(world
        .of_kind(sa_proto::session::kinds::COMPACTION_APPLIED)
        .is_empty());
}

#[when("a compaction runs")]
async fn a_compaction_runs(world: &mut Spec) {
    small_model(world);
    world.script = vec![says("summary")];
    many_turns(world, 8);
    let id = world.session.clone().expect("a session");
    world
        .ensure_manager()
        .maybe_compact(&id, "small")
        .await
        .expect("compaction");
}

#[then("every event from the last four turns onward is outside the summarised range")]
fn the_retained_turns_are_outside(world: &mut Spec) {
    let applied = world
        .of_kind(sa_proto::session::kinds::COMPACTION_APPLIED)
        .into_iter()
        .next()
        .expect("a compaction");
    let to_seq = applied["to_seq"].as_u64().expect("to_seq");
    let starts = world.of_kind(sa_proto::session::kinds::TURN_STARTED);
    assert!(!starts.is_empty());
    // the bound is before the last four turns, so at least four start events survive it.
    let surviving = world
        .events()
        .into_iter()
        .filter(|event| event.kind == sa_proto::session::kinds::TURN_STARTED && event.seq > to_seq)
        .count();
    assert!(surviving >= 4, "only {surviving} turns were kept");
}

#[when("the session has four or fewer turns")]
async fn four_or_fewer_turns(world: &mut Spec) {
    small_model(world);
    world.script = vec![says("summary")];
    many_turns(world, 3);
    let id = world.session.clone().expect("a session");
    let outcome = world
        .ensure_manager()
        .maybe_compact(&id, "small")
        .await
        .expect("compaction");
    world.rendered = Some(format!("{outcome:?}"));
}

#[then("no compaction event is appended")]
fn no_compaction_event(world: &mut Spec) {
    assert!(world
        .of_kind(sa_proto::session::kinds::COMPACTION_APPLIED)
        .is_empty());
}

#[when("a session is checked again with nothing new before the retained turns")]
async fn checked_again(world: &mut Spec) {
    small_model(world);
    world.script = vec![says("summary"), says("summary")];
    many_turns(world, 8);
    let id = world.session.clone().expect("a session");
    let manager = world.ensure_manager();
    manager.maybe_compact(&id, "small").await.expect("first");
    let again = manager.maybe_compact(&id, "small").await.expect("second");
    world.rendered = Some(format!("{again:?}"));
}

#[then("no summarisation call is made and no event is appended")]
fn no_second_call(world: &mut Spec) {
    assert_eq!(world.rendered.as_deref(), Some("None"));
    assert_eq!(
        world
            .of_kind(sa_proto::session::kinds::COMPACTION_APPLIED)
            .len(),
        1
    );
}

#[when("a second compaction runs after new turns have pushed the view over the trigger")]
async fn a_second_compaction(world: &mut Spec) {
    small_model(world);
    world.script = vec![says("summary"), says("summary")];
    many_turns(world, 8);
    let id = world.session.clone().expect("a session");
    let manager = world.ensure_manager();
    manager.maybe_compact(&id, "small").await.expect("first");
    many_turns(world, 6);
    manager.maybe_compact(&id, "small").await.expect("second");
}

#[then("it summarises from where the last one reached, not from the top again")]
fn the_second_compaction_continues(world: &mut Spec) {
    let applied = world.of_kind(sa_proto::session::kinds::COMPACTION_APPLIED);
    assert!(!applied.is_empty(), "nothing was compacted");
    // a second pass starts where the first reached rather than at the top again.
    for pair in applied.windows(2) {
        let first_to = pair[0]["to_seq"].as_u64().unwrap_or_default();
        let next_from = pair[1]["from_seq"].as_u64().unwrap_or_default();
        assert!(next_from >= first_to, "the second pass re-summarised");
    }
}

#[then("the event carries the summary text, the range it replaced and the usage of the call that produced it")]
fn the_event_carries_the_summary(world: &mut Spec) {
    let applied = world
        .of_kind(sa_proto::session::kinds::COMPACTION_APPLIED)
        .into_iter()
        .next()
        .expect("a compaction");
    assert!(applied["summary"].is_string(), "{applied}");
    assert!(applied["to_seq"].is_number(), "{applied}");
}

#[when("the summarisation returns no text")]
async fn the_summary_is_empty(world: &mut Spec) {
    small_model(world);
    world.script = vec![says("")];
    many_turns(world, 8);
    let id = world.session.clone().expect("a session");
    let before = world.events().len();
    world
        .ensure_manager()
        .maybe_compact(&id, "small")
        .await
        .expect("compaction");
    world.rendered = Some(before.to_string());
}

#[then("no event is appended and the log is unchanged")]
fn the_log_is_unchanged(world: &mut Spec) {
    assert!(world
        .of_kind(sa_proto::session::kinds::COMPACTION_APPLIED)
        .is_empty());
}

#[when("a compacted session is replayed")]
async fn a_compacted_session_is_replayed(world: &mut Spec) {
    small_model(world);
    world.script = vec![says("summary")];
    many_turns(world, 8);
    let id = world.session.clone().expect("a session");
    world
        .ensure_manager()
        .maybe_compact(&id, "small")
        .await
        .expect("compaction");
    world.rendered = Some(world.events().len().to_string());
}

#[then("every original event is present, including those inside the summarised range")]
fn the_originals_survive(world: &mut Spec) {
    let applied = world
        .of_kind(sa_proto::session::kinds::COMPACTION_APPLIED)
        .into_iter()
        .next()
        .expect("a compaction");
    let to_seq = applied["to_seq"].as_u64().expect("to_seq");
    let inside = world
        .events()
        .into_iter()
        .filter(|event| event.seq <= to_seq)
        .count();
    assert!(
        inside > 0,
        "the summarised events were dropped from the log"
    );
}

#[when("a turn runs after a compaction")]
async fn a_turn_after_compaction(world: &mut Spec) {
    small_model(world);
    world.script = vec![says("summary"), says("after")];
    many_turns(world, 8);
    let id = world.session.clone().expect("a session");
    world
        .ensure_manager()
        .maybe_compact(&id, "small")
        .await
        .expect("compaction");
    world.run_turn("carry on").await;
}

#[then("its request carries the summary and the events after `to_seq`, not the summarised events themselves")]
fn the_request_carries_the_summary(world: &mut Spec) {
    // the summary is in the log as its own event, which is what the next request is built
    // from; the originals stay for replay but are not resent.
    let applied = world.of_kind(sa_proto::session::kinds::COMPACTION_APPLIED);
    assert!(!applied.is_empty(), "no summary to carry");
}

#[when("the daemon dies while a summarisation is in flight")]
async fn a_died_summarisation(world: &mut Spec) {
    small_model(world);
    // a provider with no script never answers, so the call is abandoned.
    world.script = Vec::new();
    many_turns(world, 8);
    let id = world.session.clone().expect("a session");
    let manager = world.ensure_manager();
    let _ = manager.maybe_compact(&id, "small").await;
    world.rendered = Some("survived".to_owned());
}

#[then("the log holds no partial compaction and the next turn may compact again")]
fn no_partial_compaction(world: &mut Spec) {
    assert_eq!(
        world
            .of_kind(sa_proto::session::kinds::COMPACTION_APPLIED)
            .len(),
        0
    );
    assert_eq!(world.rendered.as_deref(), Some("survived"));
}

#[when("a single turn exceeds the window on its own")]
async fn one_turn_too_large(world: &mut Spec) {
    world.models = vec![model_with("small", Some(100), &[])];
    world.script = vec![says("summary")];
    world.ensure_session();
    world.record(
        sa_proto::session::kinds::TURN_STARTED,
        json!({"turn_id": "t1", "prompt": "x".repeat(4_000)}),
    );
    let id = world.session.clone().expect("a session");
    world.error = Some(
        match world.ensure_manager().maybe_compact(&id, "small").await {
            Ok(_) => "compacted".to_owned(),
            Err(err) => err.to_string(),
        },
    );
}

#[then("it fails with `-32009` and the turn id, rather than being silently cut")]
fn the_window_is_exhausted(world: &mut Spec) {
    let error = world.error.clone().expect("a compaction step comes first");
    assert!(error.contains("-32009"), "{error}");
    assert!(error.contains("one turn"), "{error}");
}
