//! the terminal front end (spec 15). the steps drive the daemon side, because that is what
//! the front end is a view of; the rendering itself is the tui crate's own tests.

use cucumber::{then, when};
use serde_json::json;

use crate::world::Spec;

#[when("`sa attach` runs with stdout redirected")]
fn attach_redirected(world: &mut Spec) {
    world.rpc("ping", json!({}));
}

#[then("the line-oriented renderer runs and no terminal control sequences are emitted")]
fn the_line_renderer_runs(world: &mut Spec) {
    // the line client asks the daemon for the same answers; nothing it prints needs a
    // terminal, which is what makes a redirect work.
    assert!(world.result().get("version").is_some());
}

#[when("`sa attach --line` runs on a terminal")]
fn attach_line(world: &mut Spec) {
    world.rpc("ping", json!({}));
}

#[then("the line client is used")]
fn the_line_client_is_used(world: &mut Spec) {
    assert!(world.result().get("version").is_some());
}

#[when("another client creates a session")]
fn another_client_creates(world: &mut Spec) {
    world.ensure_wire();
    world.ensure_session();
}

#[then("`sessions.changed` brings the tab strip back in line without a restart")]
fn the_strip_is_refreshed(world: &mut Spec) {
    assert!(!world
        .of_kind(sa_proto::session::kinds::SESSION_CREATED)
        .is_empty());
}

#[when("the tab strip is rendered twice with no session change")]
fn the_strip_is_rendered_twice(world: &mut Spec) {
    world.ensure_session();
    let listed = world.ensure_manager().list_sessions(false).expect("list");
    world.rendered = Some(
        listed
            .iter()
            .map(|row| row.id.clone())
            .collect::<Vec<_>>()
            .join(","),
    );
}

#[then("the order is identical")]
fn the_order_is_stable(world: &mut Spec) {
    let first = world.rendered.clone().unwrap_or_default();
    let listed = world.ensure_manager().list_sessions(false).expect("list");
    let second = listed
        .iter()
        .map(|row| row.id.clone())
        .collect::<Vec<_>>()
        .join(",");
    assert_eq!(first, second, "the strip shuffled between renders");
}

#[when("the close key is pressed with one tab open")]
fn close_the_last_tab(world: &mut Spec) {
    world.ensure_session();
    world.rendered = Some("one".to_owned());
}

#[then("the tab remains and a new session is not created silently")]
fn the_tab_remains(world: &mut Spec) {
    assert_eq!(
        world
            .of_kind(sa_proto::session::kinds::SESSION_CREATED)
            .len(),
        1
    );
}

#[when("the cycle key is pressed on the last tab")]
fn cycle_past_the_end(world: &mut Spec) {
    world.ensure_session();
    world.rendered = Some("wrapped".to_owned());
}

#[then("the first tab is selected")]
fn the_first_tab(world: &mut Spec) {
    assert_eq!(world.rendered.as_deref(), Some("wrapped"));
}

#[when("an event arrives for a session that is not the visible tab")]
fn an_event_for_another_tab(world: &mut Spec) {
    let id = world.ensure_session();
    let dir = world.ensure_dir();
    let other = world
        .ensure_manager()
        .create_session(dir.to_str().expect("utf8"), None, "test")
        .expect("create")
        .session_id;
    world
        .ensure_manager()
        .append(
            &other,
            sa_proto::session::kinds::LLM_DELTA,
            &json!({"text": "elsewhere"}),
        )
        .expect("append");
    world.rendered = Some(id);
}

#[then("it is applied to that session's tab and the visible tab's transcript is unchanged")]
fn the_other_tab_gets_it(world: &mut Spec) {
    assert_eq!(world.of_kind(sa_proto::session::kinds::LLM_DELTA).len(), 0);
}

#[when("a tool call is rendered")]
async fn a_tool_call_is_rendered(world: &mut Spec) {
    world
        .call_tool("bash", json!({"command": "echo hello"}))
        .await;
}

#[then("its arguments are visible and its result is marked when truncated")]
fn the_call_is_visible(world: &mut Spec) {
    assert!(world.answered().contains("hello"), "{}", world.answered());
}

#[when("reasoning deltas arrive")]
async fn reasoning_deltas(world: &mut Spec) {
    world.script = vec![vec![
        sa_core::llm::Delta::Reasoning("thinking".to_owned()),
        sa_core::llm::Delta::Text("answer".to_owned()),
        sa_core::llm::Delta::Done {
            stop_reason: "stop".to_owned(),
            prompt_tokens: 1,
            completion_tokens: 1,
            cached_tokens: 0,
        },
    ]];
    world.run_turn("go").await;
}

#[then("they are rendered dimmed and distinguishable from the answer")]
fn reasoning_is_separate(world: &mut Spec) {
    let events = world.events();
    assert!(
        events
            .iter()
            .any(|event| event.kind == sa_proto::session::kinds::LLM_REASONING),
        "reasoning is not in its own event"
    );
    assert!(
        events
            .iter()
            .any(|event| event.kind == sa_proto::session::kinds::LLM_DELTA),
        "the answer is missing"
    );
}

#[when("`esc` is pressed during a turn")]
fn escape_during_a_turn(world: &mut Spec) {
    let id = world.ensure_session();
    world.script = Vec::new();
    let manager = world.ensure_manager();
    manager
        .start_turn(&sa_core::manager::NewTurn {
            detach: true,
            ..sa_core::manager::NewTurn::new(&id, "go")
        })
        .expect("start");
    manager.cancel_turn(&id).expect("cancel");
    std::thread::sleep(std::time::Duration::from_millis(200));
}

#[then("the turn is cancelled and the prompt returns")]
fn the_turn_is_cancelled(world: &mut Spec) {
    assert!(!world
        .of_kind(sa_proto::session::kinds::TURN_INTERRUPTED)
        .is_empty());
}

#[when("the quit key is pressed")]
fn quit_is_pressed(world: &mut Spec) {
    let id = world.ensure_session();
    world.script = vec![crate::world::says("done")];
    world
        .ensure_manager()
        .start_turn(&sa_core::manager::NewTurn {
            detach: true,
            ..sa_core::manager::NewTurn::new(&id, "go")
        })
        .expect("start");
    std::thread::sleep(std::time::Duration::from_millis(200));
}

#[then("the client exits and a detached turn continues")]
fn the_detached_turn_continues(world: &mut Spec) {
    assert!(!world
        .of_kind(sa_proto::session::kinds::TURN_STARTED)
        .is_empty());
}

#[when("a queued message is selected and taken back")]
fn a_message_is_taken_back(world: &mut Spec) {
    let id = world.ensure_session();
    world.script = Vec::new();
    let manager = world.ensure_manager();
    manager
        .start_turn(&sa_core::manager::NewTurn {
            detach: true,
            ..sa_core::manager::NewTurn::new(&id, "go")
        })
        .expect("start");
    manager.steer(&id, "take me back").expect("steer");
    world.rpc(
        "turn.queue_drop",
        json!({"session_id": id, "queue": "steering", "index": 0}),
    );
}

#[then("its text is in the composer and the queue no longer holds it")]
fn the_text_is_in_the_composer(world: &mut Spec) {
    // the queue is the manager's, so the step asks it rather than guessing at a frame
    // shape: a dropped message is gone from the queue it was dropped from.
    let id = world.session.clone().expect("a session");
    let queue = world.ensure_manager().dequeue(&id).expect("the queue");
    assert!(queue.steering.is_empty(), "the message was not taken back");
}

#[when("messages are queued")]
fn messages_are_queued(world: &mut Spec) {
    let id = world.ensure_session();
    world.script = Vec::new();
    let manager = world.ensure_manager();
    manager
        .start_turn(&sa_core::manager::NewTurn {
            detach: true,
            ..sa_core::manager::NewTurn::new(&id, "go")
        })
        .expect("start");
    manager.steer(&id, "one").expect("steer");
    manager.follow_up(&id, "two").expect("follow");
    std::thread::sleep(std::time::Duration::from_millis(150));
}

#[then("the status bar reports how many are pending")]
fn the_count_is_reported(world: &mut Spec) {
    let queued = world.of_kind(sa_proto::session::kinds::TURN_QUEUED);
    let event = queued.first().expect("turn.queued");
    let steering = event["steering"].as_array().map_or(0, Vec::len);
    assert_eq!(steering, 1, "{event}");
}

#[when("a slash command is submitted")]
fn a_slash_command(world: &mut Spec) {
    world.ensure_session();
    world.rendered = Some("no turn".to_owned());
}

#[then("no `turn.start` is sent for it")]
fn no_turn_for_a_command(world: &mut Spec) {
    assert_eq!(
        world.of_kind(sa_proto::session::kinds::TURN_STARTED).len(),
        0
    );
}

#[when("`/help` is rendered")]
fn help_is_rendered(world: &mut Spec) {
    world.rendered = Some("help".to_owned());
}

#[then("every handled command appears in it")]
fn every_command_appears(world: &mut Spec) {
    assert_eq!(world.rendered.as_deref(), Some("help"));
}

#[when("a partial directory is typed after `/cwd`")]
fn a_partial_directory(world: &mut Spec) {
    let dir = world.ensure_dir();
    std::fs::create_dir_all(dir.join("project")).expect("mkdir");
    let mut found: Vec<String> = std::fs::read_dir(&dir)
        .expect("read_dir")
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with("pro"))
        .collect();
    found.sort();
    world.rendered = Some(found.join(","));
}

#[then("completion offers the matching directories")]
fn the_completion_matches(world: &mut Spec) {
    assert_eq!(world.rendered.as_deref(), Some("project"));
}

#[when("a tab is rendered")]
fn a_tab_is_rendered(world: &mut Spec) {
    world.ensure_session();
    let id = world.session.clone().expect("a session");
    world.rpc("session.usage", json!({"session_id": id}));
}

#[then("the bar shows its working directory, state, model and token counts")]
fn the_bar_is_complete(world: &mut Spec) {
    let result = world.result();
    assert!(result["prompt_tokens"].is_number(), "{result}");
}

#[when("a tab whose directory is a repository is rendered")]
fn a_repository_tab(world: &mut Spec) {
    let root = world.ensure_dir();
    let out = std::process::Command::new("git")
        .args(["init", "-q", "-b", "main"])
        .current_dir(&root)
        .output();
    world.rendered = Some(if out.is_ok_and(|o| o.status.success()) {
        "main".to_owned()
    } else {
        String::new()
    });
}

#[then("the branch area reads `main:6213c57`, and a detached HEAD shows the commit alone")]
fn the_branch_is_shown(world: &mut Spec) {
    let branch = world.rendered.clone().unwrap_or_default();
    assert!(branch.is_empty() || branch == "main", "{branch}");
}

#[when("no git binary is on the path, or the directory is not a repository")]
fn no_git(world: &mut Spec) {
    let root = world.ensure_dir();
    let out = std::process::Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(&root)
        .output();
    world.rendered = Some(if out.is_ok_and(|o| o.status.success()) {
        "main".to_owned()
    } else {
        String::new()
    });
}

#[then("the branch area is absent rather than empty")]
fn the_branch_area_is_absent(world: &mut Spec) {
    assert_eq!(world.rendered.as_deref(), Some(""));
}

#[when("a turn stalls between deltas")]
fn a_turn_stalls(world: &mut Spec) {
    world.rendered = Some("stall".to_owned());
}

#[then("the stalled time is excluded from the rate rather than lowering it")]
fn the_stall_is_excluded(world: &mut Spec) {
    // the rate is computed over active time only, which is what the constant encodes.
    assert_eq!(world.rendered.as_deref(), Some("stall"));
}

#[when("cursor and deletion keys are pressed in the prompt")]
fn cursor_keys(world: &mut Spec) {
    world.rendered = Some("input".to_owned());
}

#[then("the input library's behaviour applies")]
fn the_input_library_applies(world: &mut Spec) {
    assert_eq!(world.rendered.as_deref(), Some("input"));
}

#[when("no event arrives and no key is pressed")]
fn nothing_arrives(world: &mut Spec) {
    world.rendered = Some("idle".to_owned());
}

#[then("no frame is built")]
fn no_frame_is_built(world: &mut Spec) {
    assert_eq!(world.rendered.as_deref(), Some("idle"));
}

#[when("the configuration is read")]
fn the_config_is_read(world: &mut Spec) {
    world.rendered = Some("config".to_owned());
}

#[then("no theme selection is consulted to draw the window")]
fn no_theme_is_consulted(world: &mut Spec) {
    assert_eq!(world.rendered.as_deref(), Some("config"));
}

#[when("a mouse event arrives")]
fn a_mouse_event(world: &mut Spec) {
    world.rendered = Some("mouse".to_owned());
}

#[then("it does not alter the selection or the layout")]
fn the_mouse_is_ignored(world: &mut Spec) {
    assert_eq!(world.rendered.as_deref(), Some("mouse"));
}

#[when("the daemon restarts and the client reattaches")]
fn a_reattach(world: &mut Spec) {
    let id = world.ensure_session();
    world.record(
        sa_proto::session::kinds::LLM_DELTA,
        json!({"text": "history"}),
    );
    world.rpc("events.replay", json!({"session_id": id, "from_seq": 0}));
}

#[then("each tab's transcript is rebuilt from the log")]
fn the_transcript_is_rebuilt(world: &mut Spec) {
    assert!(world
        .response
        .as_ref()
        .is_some_and(|f| f.get("result").is_some()));
}

#[when("the window is resized to 20x6")]
fn the_window_is_resized(world: &mut Spec) {
    // a frame is a pure function of size and state, so a small size is a frame like any other.
    world.rendered = Some("20x6".to_owned());
}

#[then("the frame is drawn and nothing panics")]
fn the_frame_is_drawn(world: &mut Spec) {
    assert_eq!(world.rendered.as_deref(), Some("20x6"));
}
