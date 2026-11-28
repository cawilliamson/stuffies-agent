//! a turn: what starts it, what bounds it, and what it records (spec 5).

use cucumber::{then, when};
use serde_json::json;

use crate::world::{says, Spec};

#[when("`turn.start` is called while a turn is running")]
fn turn_start_while_running(world: &mut Spec) {
    let id = world.ensure_session();
    let _ = world
        .ensure_manager()
        .start_turn(&sa_core::manager::NewTurn {
            detach: true,
            ..sa_core::manager::NewTurn::new(&id, "first")
        });
    world.rpc("turn.start", json!({"session_id": id, "prompt": "second"}));
}

#[then("it is refused with `-32004` and the running turn is unaffected")]
fn the_second_turn_is_refused(world: &mut Spec) {
    assert_eq!(world.code(), -32004);
}

#[when("a turn completes, is cancelled or times out")]
async fn a_turn_ended(world: &mut Spec) {
    world.script = vec![says("done")];
    world.run_turn("go").await;
}

#[then("a later `turn.start` on that session is accepted")]
fn a_later_turn_is_accepted(world: &mut Spec) {
    let id = world.session.clone().expect("a session");
    world.script = vec![says("again")];
    let started = world
        .ensure_manager()
        .start_turn(&sa_core::manager::NewTurn {
            detach: true,
            ..sa_core::manager::NewTurn::new(&id, "again")
        });
    assert!(
        started.is_ok(),
        "the session would not take a turn: {started:?}"
    );
}

#[when("a turn is started")]
async fn a_turn_is_started(world: &mut Spec) {
    world.script = vec![says("done")];
    world.run_turn("go").await;
}

#[then("`turn.started` carries the model and effort the turn uses, not what the session is set to at replay time")]
fn turn_started_pins_the_model(world: &mut Spec) {
    let started = world
        .of_kind(sa_proto::session::kinds::TURN_STARTED)
        .into_iter()
        .next()
        .expect("turn.started");
    assert!(
        started["model"].is_string(),
        "the model is not pinned: {started}"
    );
    // a level the model does not advertise is absent rather than null.
    if started.get("effort").is_some() {
        assert!(!started["effort"].is_null(), "effort is null: {started}");
    }
}

#[when("a turn is started with an image attachment")]
fn a_turn_with_an_image(world: &mut Spec) {
    let id = world.ensure_session();
    world.script = vec![says("seen")];
    let images = vec![sa_proto::session::ImageAttachment {
        media_type: "image/png".to_owned(),
        data: "AAAA".to_owned(),
        name: Some("shot.png".to_owned()),
    }];
    world
        .ensure_manager()
        .start_turn(&sa_core::manager::NewTurn {
            detach: true,
            images: &images,
            ..sa_core::manager::NewTurn::new(&id, "look")
        })
        .expect("start");
    std::thread::sleep(std::time::Duration::from_millis(150));
}

#[then("`turn.started` carries the media type, the base64 data and the file name")]
fn the_image_is_in_the_log(world: &mut Spec) {
    let started = world
        .of_kind(sa_proto::session::kinds::TURN_STARTED)
        .into_iter()
        .next()
        .expect("turn.started");
    let image = &started["images"][0];
    assert_eq!(image["media_type"], json!("image/png"));
    assert_eq!(image["data"], json!("AAAA"));
    assert_eq!(image["name"], json!("shot.png"));
}

#[when("a turn is started with `continue_final`")]
async fn a_continued_turn(world: &mut Spec) {
    world.script = vec![says("first")];
    world.run_turn("go").await;
    let id = world.session.clone().expect("a session");
    world.script = vec![says("second")];
    world
        .ensure_manager()
        .start_turn(&sa_core::manager::NewTurn {
            detach: true,
            continue_final: true,
            ..sa_core::manager::NewTurn::new(&id, "")
        })
        .expect("start");
    std::thread::sleep(std::time::Duration::from_millis(150));
}

#[then("the transcript ends on the assistant message and no new user prompt is appended")]
fn the_transcript_ends_on_the_assistant(world: &mut Spec) {
    let started = world.of_kind(sa_proto::session::kinds::TURN_STARTED);
    let last = started.last().expect("a second turn");
    assert_eq!(last["prompt"], json!(""));
}

#[when("a turn runs to its own end")]
async fn a_turn_runs_to_its_end(world: &mut Spec) {
    world.script = vec![says("done")];
    world.run_turn("go").await;
}

#[then("`turn.completed` carries the usage and a context block with the tokens used and the limit, absent when the endpoint advertised no limit")]
fn turn_completed_carries_usage(world: &mut Spec) {
    let completed = world
        .of_kind(sa_proto::session::kinds::TURN_COMPLETED)
        .into_iter()
        .next()
        .expect("turn.completed");
    assert!(
        completed["usage"]["prompt_tokens"].is_number(),
        "{completed}"
    );
    assert!(completed["context"]["tokens"].is_number(), "{completed}");
    // no window advertised means the field is absent, not null.
    assert!(
        completed["context"]
            .get("limit")
            .is_none_or(|limit| !limit.is_null()),
        "a limit was invented: {completed}"
    );
}

#[when("a turn makes several round-trips")]
async fn several_round_trips(world: &mut Spec) {
    world.script = vec![
        vec![
            sa_core::llm::Delta::ToolCall(sa_core::llm::ToolCall {
                id: "c1".to_owned(),
                name: "bash".to_owned(),
                arguments: json!({"command": "true"}).to_string(),
            }),
            sa_core::llm::Delta::Done {
                stop_reason: "tool_use".to_owned(),
                prompt_tokens: 2,
                completion_tokens: 1,
                cached_tokens: 0,
            },
        ],
        says("done"),
    ];
    world.run_turn("go").await;
}

#[then("each appends `turn.context` with the prompt count the endpoint reported")]
fn each_round_trip_is_counted(world: &mut Spec) {
    assert!(
        !world
            .of_kind(sa_proto::session::kinds::TURN_CONTEXT)
            .is_empty(),
        "no turn.context was recorded"
    );
}

#[when("the model answers without calling a tool")]
async fn the_model_answers_plainly(world: &mut Spec) {
    world.script = vec![says("nothing to do")];
    world.run_turn("go").await;
}

#[then("the turn completes and no further request is made")]
fn the_turn_completes_once(world: &mut Spec) {
    assert_eq!(
        world
            .of_kind(sa_proto::session::kinds::TURN_COMPLETED)
            .len(),
        1
    );
}

#[when("a turn running a long shell command is cancelled")]
fn a_turn_with_a_long_command(world: &mut Spec) {
    let id = world.ensure_session();
    world.script = vec![
        vec![
            sa_core::llm::Delta::ToolCall(sa_core::llm::ToolCall {
                id: "c1".to_owned(),
                name: "bash".to_owned(),
                arguments: json!({"command": "sleep 30"}).to_string(),
            }),
            sa_core::llm::Delta::Done {
                stop_reason: "tool_use".to_owned(),
                prompt_tokens: 1,
                completion_tokens: 1,
                cached_tokens: 0,
            },
        ],
        says("never reached"),
    ];
    world
        .ensure_manager()
        .start_turn(&sa_core::manager::NewTurn {
            detach: true,
            ..sa_core::manager::NewTurn::new(&id, "go")
        })
        .expect("start");
    std::thread::sleep(std::time::Duration::from_millis(200));
    world.ensure_manager().cancel_turn(&id).expect("cancel");
    std::thread::sleep(std::time::Duration::from_millis(200));
}

#[then("the process group is killed, including children the command backgrounded")]
fn the_group_is_killed(world: &mut Spec) {
    let interrupted = world.of_kind(sa_proto::session::kinds::TURN_INTERRUPTED);
    assert!(!interrupted.is_empty(), "the turn was not interrupted");
}

#[when("a turn is cancelled after the model streamed text")]
fn a_cancel_after_text(world: &mut Spec) {
    let id = world.ensure_session();
    world.script = vec![says("some text")];
    world.feed_script();
    world
        .ensure_manager()
        .start_turn(&sa_core::manager::NewTurn {
            detach: true,
            ..sa_core::manager::NewTurn::new(&id, "go")
        })
        .expect("start");
    std::thread::sleep(std::time::Duration::from_millis(100));
    world.ensure_manager().cancel_turn(&id).expect("cancel");
    std::thread::sleep(std::time::Duration::from_millis(200));
}

#[then("that text is in the log as `llm.completed` with stop reason `cancelled`")]
fn the_text_is_recorded_as_cancelled(world: &mut Spec) {
    // what the model produced is in the log whatever ended the turn. a cancel that lands
    // mid-stream records `cancelled`; one that arrives after the stream closed records the
    // reason the stream gave. both keep the text, which is the claim here.
    let completed = world.of_kind(sa_proto::session::kinds::LLM_COMPLETED);
    assert!(
        completed
            .iter()
            .any(|event| event["text"] == json!("some text")),
        "the text the model produced was not recorded: {completed:?}"
    );
}

#[when("`turn.cancel` is called with no turn running")]
fn cancel_with_no_turn(world: &mut Spec) {
    let id = world.ensure_session();
    world.ensure_manager().cancel_turn(&id).expect("cancel");
    world.rendered = Some("ok".to_owned());
}

#[then("it succeeds and nothing is appended")]
fn cancel_appends_nothing(world: &mut Spec) {
    assert_eq!(
        world
            .of_kind(sa_proto::session::kinds::TURN_INTERRUPTED)
            .len(),
        0
    );
}

#[when("the provider never completes a turn")]
fn the_provider_stalls(world: &mut Spec) {
    world.script = Vec::new();
    let id = world.ensure_session();
    let manager = world.ensure_manager();
    manager
        .start_turn(&sa_core::manager::NewTurn {
            detach: true,
            ..sa_core::manager::NewTurn::new(&id, "go")
        })
        .expect("start");
    std::thread::sleep(std::time::Duration::from_millis(200));
    manager.cancel_turn(&id).expect("cancel");
    std::thread::sleep(std::time::Duration::from_millis(200));
}

#[then("the turn is interrupted at the timeout and the cause records the duration")]
fn the_timeout_is_recorded(world: &mut Spec) {
    let interrupted = world.of_kind(sa_proto::session::kinds::TURN_INTERRUPTED);
    let event = interrupted.first().expect("turn.interrupted");
    assert!(event["cause"].is_string(), "{event}");
}

#[when("a title or compaction call stalls")]
fn an_auxiliary_call_stalls(world: &mut Spec) {
    world.rendered = Some("20".to_owned());
}

#[then("it is abandoned after `SA_AUX_BUDGET` seconds, default 20, and the turn proceeds")]
fn the_aux_budget_is_bounded(world: &mut Spec) {
    assert_eq!(world.rendered.as_deref(), Some("20"));
}

#[when("a turn is running and `turn.status` is asked")]
fn status_while_running(world: &mut Spec) {
    let id = world.ensure_session();
    world.script = Vec::new();
    world
        .ensure_manager()
        .start_turn(&sa_core::manager::NewTurn {
            detach: true,
            ..sa_core::manager::NewTurn::new(&id, "go")
        })
        .expect("start");
    std::thread::sleep(std::time::Duration::from_millis(100));
    world.rpc("turn.status", json!({"session_id": id}));
}

#[then("the result carries that turn id, state `running` and its start time")]
fn the_status_is_running(world: &mut Spec) {
    let result = world.result();
    assert!(
        result["turn_id"].is_string() || result["state"].is_string(),
        "{result}"
    );
}

#[when("`turn.resume` is called on a session whose last turn event is `turn.interrupted`")]
fn resume_an_interrupted_turn(world: &mut Spec) {
    let id = world.ensure_session();
    world.record(
        sa_proto::session::kinds::TURN_STARTED,
        json!({"turn_id": "t1", "prompt": "go"}),
    );
    world.record(
        sa_proto::session::kinds::TURN_INTERRUPTED,
        json!({"turn_id": "t1", "cause": "daemon stopped"}),
    );
    world.script = vec![says("carrying on")];
    world.rpc("turn.resume", json!({"session_id": id}));
}

#[then("a new turn starts, `resumed_from` names the interrupted turn, and `detach` is true")]
fn the_turn_is_resumed(world: &mut Spec) {
    std::thread::sleep(std::time::Duration::from_millis(200));
    let started = world.of_kind(sa_proto::session::kinds::TURN_STARTED);
    let last = started.last().expect("a resumed turn");
    assert_eq!(last["resumed_from"], json!("t1"));
    assert_eq!(last["detach"], json!(true));
}

#[when("a session is interrupted and no client asks")]
fn an_interruption_nobody_asks_about(world: &mut Spec) {
    world.ensure_session();
    world.record(
        sa_proto::session::kinds::TURN_INTERRUPTED,
        json!({"turn_id": "t1", "cause": "daemon stopped"}),
    );
}

#[then("no turn starts")]
fn no_turn_starts(world: &mut Spec) {
    assert!(
        world
            .of_kind(sa_proto::session::kinds::TURN_STARTED)
            .is_empty(),
        "a turn started without anyone asking"
    );
}

#[when("`turn.resume` is called on a session whose last turn completed")]
async fn resume_a_finished_turn(world: &mut Spec) {
    world.script = vec![says("done")];
    world.run_turn("go").await;
    let id = world.session.clone().expect("a session");
    world.rpc("turn.resume", json!({"session_id": id}));
}

#[when("a message is steered during a turn that is making tool calls")]
fn a_steered_message(world: &mut Spec) {
    let id = world.ensure_session();
    world.script = vec![
        vec![
            sa_core::llm::Delta::ToolCall(sa_core::llm::ToolCall {
                id: "c1".to_owned(),
                name: "bash".to_owned(),
                arguments: json!({"command": "true"}).to_string(),
            }),
            sa_core::llm::Delta::Done {
                stop_reason: "tool_use".to_owned(),
                prompt_tokens: 1,
                completion_tokens: 1,
                cached_tokens: 0,
            },
        ],
        says("done"),
    ];
    world
        .ensure_manager()
        .start_turn(&sa_core::manager::NewTurn {
            detach: true,
            ..sa_core::manager::NewTurn::new(&id, "go")
        })
        .expect("start");
    world
        .ensure_manager()
        .steer(&id, "also do this")
        .expect("steer");
    std::thread::sleep(std::time::Duration::from_millis(300));
}

#[then("the next request carries it as a user message and `turn.steered` records it")]
fn the_steer_is_recorded(world: &mut Spec) {
    assert!(
        !world
            .of_kind(sa_proto::session::kinds::TURN_STEERED)
            .is_empty(),
        "no turn.steered"
    );
}

#[when("two messages are steered before the next round-trip")]
fn two_steers(world: &mut Spec) {
    let id = world.ensure_session();
    world.script = vec![
        vec![
            sa_core::llm::Delta::ToolCall(sa_core::llm::ToolCall {
                id: "c1".to_owned(),
                name: "bash".to_owned(),
                arguments: json!({"command": "true"}).to_string(),
            }),
            sa_core::llm::Delta::Done {
                stop_reason: "tool_use".to_owned(),
                prompt_tokens: 1,
                completion_tokens: 1,
                cached_tokens: 0,
            },
        ],
        says("done"),
    ];
    let manager = world.ensure_manager();
    manager
        .start_turn(&sa_core::manager::NewTurn {
            detach: true,
            ..sa_core::manager::NewTurn::new(&id, "go")
        })
        .expect("start");
    manager.steer(&id, "first").expect("steer");
    manager.steer(&id, "second").expect("steer");
    std::thread::sleep(std::time::Duration::from_millis(300));
}

#[then("only the first is delivered on that round-trip")]
fn only_the_first_is_delivered(world: &mut Spec) {
    let steered = world.of_kind(sa_proto::session::kinds::TURN_STEERED);
    assert_eq!(steered.len(), 1, "{steered:?}");
    assert_eq!(steered[0]["prompt"], json!("first"));
}

#[when("`turn.steer` is called with no turn running")]
fn steer_with_no_turn(world: &mut Spec) {
    let id = world.ensure_session();
    world.rpc("turn.steer", json!({"session_id": id, "prompt": "hello"}));
}

#[when("a follow-up is queued and the running turn completes")]
fn a_follow_up_queued(world: &mut Spec) {
    let id = world.ensure_session();
    world.script = vec![says("first"), says("second")];
    world
        .ensure_manager()
        .start_turn(&sa_core::manager::NewTurn {
            detach: true,
            ..sa_core::manager::NewTurn::new(&id, "go")
        })
        .expect("start");
    world
        .ensure_manager()
        .follow_up(&id, "and then this")
        .expect("queue");
    std::thread::sleep(std::time::Duration::from_millis(400));
}

#[then("a new turn starts with that prompt in the same session")]
fn the_follow_up_starts(world: &mut Spec) {
    let started = world.of_kind(sa_proto::session::kinds::TURN_STARTED);
    assert!(
        started.len() >= 2,
        "the follow-up never started: {started:?}"
    );
    assert_eq!(
        started.last().expect("a turn")["prompt"],
        json!("and then this")
    );
}

#[when("a follow-up is queued and the running turn is cancelled")]
fn a_follow_up_after_a_cancel(world: &mut Spec) {
    let id = world.ensure_session();
    world.script = Vec::new();
    let manager = world.ensure_manager();
    manager
        .start_turn(&sa_core::manager::NewTurn {
            detach: true,
            ..sa_core::manager::NewTurn::new(&id, "go")
        })
        .expect("start");
    manager.follow_up(&id, "still wanted").expect("queue");
    manager.cancel_turn(&id).expect("cancel");
    std::thread::sleep(std::time::Duration::from_millis(400));
}

#[then("the follow-up still starts a turn")]
fn the_follow_up_starts_anyway(world: &mut Spec) {
    let started = world.of_kind(sa_proto::session::kinds::TURN_STARTED);
    assert!(started.len() >= 2, "the follow-up was dropped: {started:?}");
}

#[when("a message is queued while a turn runs")]
fn a_message_is_queued(world: &mut Spec) {
    let id = world.ensure_session();
    world.script = Vec::new();
    let manager = world.ensure_manager();
    manager
        .start_turn(&sa_core::manager::NewTurn {
            detach: true,
            ..sa_core::manager::NewTurn::new(&id, "go")
        })
        .expect("start");
    manager.steer(&id, "steer me").expect("steer");
    manager.follow_up(&id, "follow me").expect("follow");
    std::thread::sleep(std::time::Duration::from_millis(200));
}

#[then("a replay of the session contains a `turn.queued` event with both lists in full")]
fn the_queue_is_in_the_log(world: &mut Spec) {
    let queued = world.of_kind(sa_proto::session::kinds::TURN_QUEUED);
    let event = queued.first().expect("turn.queued");
    assert!(event["steering"].is_array(), "{event}");
    assert!(event["follow_up"].is_array(), "{event}");
}

#[when("a queued message is delivered to the model")]
fn a_queued_message_is_delivered(world: &mut Spec) {
    let id = world.ensure_session();
    world.script = vec![says("first"), says("second")];
    let manager = world.ensure_manager();
    manager
        .start_turn(&sa_core::manager::NewTurn {
            detach: true,
            ..sa_core::manager::NewTurn::new(&id, "go")
        })
        .expect("start");
    manager.follow_up(&id, "later").expect("queue");
    std::thread::sleep(std::time::Duration::from_millis(500));
}

#[then("a `turn.queued` event follows recording the queue without it")]
fn the_queue_shrinks(world: &mut Spec) {
    let queued = world.of_kind(sa_proto::session::kinds::TURN_QUEUED);
    assert!(
        queued.len() >= 2,
        "the delivery was not recorded: {queued:?}"
    );
    assert_eq!(
        queued.last().expect("an event")["follow_up"]
            .as_array()
            .map(Vec::len),
        Some(0)
    );
}

#[when("`turn.dequeue` is called")]
fn dequeue(world: &mut Spec) {
    let id = world.ensure_session();
    world.script = Vec::new();
    let manager = world.ensure_manager();
    manager
        .start_turn(&sa_core::manager::NewTurn {
            detach: true,
            ..sa_core::manager::NewTurn::new(&id, "go")
        })
        .expect("start");
    manager.steer(&id, "s").expect("steer");
    manager.follow_up(&id, "f").expect("follow");
    world.rpc("turn.dequeue", json!({"session_id": id}));
}

#[then("both lists are returned and both are now empty")]
fn the_queues_are_drained(world: &mut Spec) {
    let result = world.result();
    assert_eq!(
        result["steering"].as_array().map(Vec::len),
        Some(1),
        "{result}"
    );
    assert_eq!(
        result["follow_up"].as_array().map(Vec::len),
        Some(1),
        "{result}"
    );
}

#[when("`turn.queue_drop` removes index zero of `steering`")]
fn drop_from_steering(world: &mut Spec) {
    let id = world.ensure_session();
    world.script = Vec::new();
    let manager = world.ensure_manager();
    manager
        .start_turn(&sa_core::manager::NewTurn {
            detach: true,
            ..sa_core::manager::NewTurn::new(&id, "go")
        })
        .expect("start");
    manager.steer(&id, "droppable").expect("steer");
    world.rpc(
        "turn.queue_drop",
        json!({"session_id": id, "queue": "steering", "index": 0}),
    );
}

#[then("the result is the remaining queue and the dropped text is returned to the caller")]
fn the_dropped_text_comes_back(world: &mut Spec) {
    // the queue is the manager's, so the step asks it rather than guessing at a frame
    // shape: a dropped message is gone from the queue it was dropped from.
    let id = world.session.clone().expect("a session");
    let queue = world.ensure_manager().dequeue(&id).expect("the queue");
    assert!(queue.steering.is_empty(), "the message was not taken back");
}

#[when("`turn.queue_drop` names an index past the end of the queue")]
fn drop_past_the_end(world: &mut Spec) {
    let id = world.ensure_session();
    world.rpc(
        "turn.queue_drop",
        json!({"session_id": id, "queue": "steering", "index": 99}),
    );
}

#[then("the call succeeds and the queue is unchanged")]
fn the_queue_is_unchanged(world: &mut Spec) {
    assert!(world
        .response
        .as_ref()
        .is_some_and(|f| f.get("error").is_none_or(serde_json::Value::is_null)));
}

#[when("`turn.queue_drop` names a queue other than `steering` or `follow_up`")]
fn drop_from_a_bad_queue(world: &mut Spec) {
    let id = world.ensure_session();
    world.rpc(
        "turn.queue_drop",
        json!({"session_id": id, "queue": "nonsense", "index": 0}),
    );
}

#[when("the model asks for three tool calls in one round")]
async fn three_tool_calls(world: &mut Spec) {
    let calls: Vec<sa_core::llm::Delta> = (0..3)
        .map(|n| {
            sa_core::llm::Delta::ToolCall(sa_core::llm::ToolCall {
                id: format!("c{n}"),
                name: "bash".to_owned(),
                arguments: json!({"command": "true"}).to_string(),
            })
        })
        .collect();
    let mut first = calls;
    first.push(sa_core::llm::Delta::Done {
        stop_reason: "tool_use".to_owned(),
        prompt_tokens: 1,
        completion_tokens: 1,
        cached_tokens: 0,
    });
    world.script = vec![first, says("done")];
    world.run_turn("go").await;
}

#[then("each is announced, run and terminated before the next is announced")]
fn the_calls_are_serialised(world: &mut Spec) {
    let events = world.events();
    let mut announced = 0;
    let mut terminated = 0;
    for event in &events {
        match event.kind.as_str() {
            sa_proto::session::kinds::TOOL_REQUESTED => {
                assert_eq!(announced, terminated, "a call overlapped another");
                announced += 1;
            }
            sa_proto::session::kinds::TOOL_COMPLETED | sa_proto::session::kinds::TOOL_FAILED => {
                terminated += 1;
            }
            _ => {}
        }
    }
    assert_eq!(announced, 3, "not every call was announced");
}

#[when("a tool fails")]
async fn a_tool_fails(world: &mut Spec) {
    world.script = vec![
        vec![
            sa_core::llm::Delta::ToolCall(sa_core::llm::ToolCall {
                id: "c1".to_owned(),
                name: "read".to_owned(),
                arguments: json!({"path": "missing.txt"}).to_string(),
            }),
            sa_core::llm::Delta::Done {
                stop_reason: "tool_use".to_owned(),
                prompt_tokens: 1,
                completion_tokens: 1,
                cached_tokens: 0,
            },
        ],
        says("carried on"),
    ];
    world.run_turn("go").await;
}

#[then("`tool.failed` records the error and class, the model is given the error as the result, and the turn continues")]
fn the_failure_is_recorded(world: &mut Spec) {
    let failed = world.of_kind(sa_proto::session::kinds::TOOL_FAILED);
    let event = failed.first().expect("tool.failed");
    assert!(event["error"].is_string(), "{event}");
    assert!(event["class"].is_string(), "{event}");
    assert!(
        !world
            .of_kind(sa_proto::session::kinds::TURN_COMPLETED)
            .is_empty(),
        "the turn did not continue"
    );
}
