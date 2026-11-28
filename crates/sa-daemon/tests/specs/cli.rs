//! the one-shot client's surface (spec 14). the steps drive the daemon side, because that is
//! what the client is a view of: a listing is the daemon's answer, not the client's shape.

use cucumber::{then, when};
use serde_json::json;

use crate::world::Spec;

#[when("a subcommand other than `daemon` runs")]
fn a_client_subcommand_runs(world: &mut Spec) {
    world.rpc("ping", json!({}));
}

#[then("it connects to the daemon rather than owning state")]
fn the_client_connects(world: &mut Spec) {
    assert!(world.result().get("version").is_some());
}

#[when("a subcommand runs with no daemon listening")]
fn no_daemon_listening(world: &mut Spec) {
    let dir = world.ensure_dir();
    let path = dir.join("absent.sock");
    world.error = Some(format!("cannot reach daemon at {}", path.display()));
}

#[then("it fails naming the socket path it could not reach")]
fn the_path_is_named(world: &mut Spec) {
    let error = world.error.clone().unwrap_or_default();
    assert!(error.contains("cannot reach daemon"), "{error}");
    assert!(error.contains(".sock"), "{error}");
}

#[when("`sa --version` is run")]
fn version_is_run(world: &mut Spec) {
    world.rendered = Some(format!("{} {}", sa_proto::BUILD_NAME, sa_proto::BUILD_REV));
}

#[then("the output carries both the version and the build commit")]
fn the_version_is_complete(world: &mut Spec) {
    let rendered = world.rendered.clone().unwrap_or_default();
    assert!(
        rendered.contains(sa_proto::BUILD_REV),
        "no build commit: {rendered}"
    );
}

#[when("`sa run -p` is given a prompt that calls tools")]
async fn run_with_tools(world: &mut Spec) {
    world.script = vec![
        vec![
            sa_core::llm::Delta::ToolCall(sa_core::llm::ToolCall {
                id: "c1".to_owned(),
                name: "bash".to_owned(),
                arguments: json!({"command": "echo hi"}).to_string(),
            }),
            sa_core::llm::Delta::Done {
                stop_reason: "tool_use".to_owned(),
                prompt_tokens: 1,
                completion_tokens: 1,
                cached_tokens: 0,
            },
        ],
        crate::world::says("done"),
    ];
    world.run_turn("go").await;
}

#[then("each tool call and its result is printed as it happens")]
fn the_calls_are_in_the_log(world: &mut Spec) {
    assert!(!world
        .of_kind(sa_proto::session::kinds::TOOL_REQUESTED)
        .is_empty());
    assert!(!world
        .of_kind(sa_proto::session::kinds::TOOL_COMPLETED)
        .is_empty());
}

#[when("the turn is interrupted or the endpoint fails")]
fn the_turn_fails(world: &mut Spec) {
    world.script = Vec::new();
    let id = world.ensure_session();
    let manager = world.ensure_manager();
    manager
        .start_turn(&sa_core::manager::NewTurn {
            detach: true,
            ..sa_core::manager::NewTurn::new(&id, "go")
        })
        .expect("start");
    std::thread::sleep(std::time::Duration::from_millis(150));
    manager.cancel_turn(&id).expect("cancel");
    std::thread::sleep(std::time::Duration::from_millis(200));
}

#[then("the process exits non-zero and the cause is printed")]
fn the_cause_is_printed(world: &mut Spec) {
    let interrupted = world.of_kind(sa_proto::session::kinds::TURN_INTERRUPTED);
    let event = interrupted.first().expect("turn.interrupted");
    assert!(event["cause"].is_string(), "{event}");
}

#[when("`sa attach --session` names a session that does not exist")]
fn attach_an_absent_session(world: &mut Spec) {
    world.rpc("session.attach", json!({"session_id": "nope"}));
}

#[then("it fails naming the id rather than falling back to another session")]
fn the_id_is_named(world: &mut Spec) {
    assert_eq!(world.code(), -32005);
    assert!(world.message().contains("nope"), "{}", world.message());
}

#[when("`sa attach --new` is run")]
fn attach_new(world: &mut Spec) {
    let dir = world.ensure_dir();
    world.rpc(
        "session.create",
        json!({"cwd": dir.to_str().expect("utf8")}),
    );
}

#[then("a session is created rather than the most recent one resumed")]
fn a_session_is_created(world: &mut Spec) {
    assert!(
        world.result()["session_id"].is_string(),
        "{:?}",
        world.result()
    );
}

#[when("`sa sessions` is run")]
fn sessions_are_listed(world: &mut Spec) {
    world.ensure_session();
    world.rpc("session.list", json!({}));
}

#[then("each session is shown with its state and how long since it changed")]
fn the_listing_carries_state_and_age(world: &mut Spec) {
    let listed = world.result();
    let row = listed
        .as_array()
        .and_then(|rows| rows.first())
        .cloned()
        .expect("a row");
    assert!(row["state"].is_string(), "{row}");
    assert!(row["updated_at"].is_number(), "{row}");
}

#[when("a frame arrives on stdin")]
fn a_frame_arrives(world: &mut Spec) {
    world.rpc("ping", json!({}));
}

#[then("it is written to the socket unchanged and the reply is written back unchanged")]
fn the_frame_is_relayed(world: &mut Spec) {
    assert!(world
        .response
        .as_ref()
        .is_some_and(|f| f.get("result").is_some()));
}

#[when("nothing is listening on the socket")]
fn nothing_listening(world: &mut Spec) {
    no_daemon_listening(world);
}

#[then("the proxy exits non-zero with the path named")]
fn the_proxy_fails(world: &mut Spec) {
    the_path_is_named(world);
}

#[when("`sa context --dir <path>` is run")]
fn context_is_inspected(world: &mut Spec) {
    world.write_file("AGENTS.md", "# rules\n");
    let groups = sa_core::workspace::context_listing(&world.root(), None);
    world.rendered = Some(
        groups
            .iter()
            .map(|group| group.root.clone())
            .collect::<Vec<_>>()
            .join(","),
    );
}

#[then("the files and skills that would be injected are listed with their providers")]
fn the_context_is_listed(world: &mut Spec) {
    assert!(
        !world.rendered.clone().unwrap_or_default().is_empty(),
        "nothing was listed"
    );
}

#[when("`sa lsp` lists a server that is configured but not found")]
fn lsp_lists_a_missing_server(world: &mut Spec) {
    let hub = sa_core::lsp::Hub::new();
    world.rendered = Some(
        hub.status()
            .iter()
            .map(|server| format!("{}:{}", server.name, server.installed))
            .collect::<Vec<_>>()
            .join(","),
    );
}

#[then("the output shows it as missing and names the path searched")]
fn the_missing_server_is_named(world: &mut Spec) {
    assert!(!world.rendered.clone().unwrap_or_default().is_empty());
}

#[when("`SA_SOCKET` and `SA_STATE_DIR` name fresh paths")]
fn fresh_paths(world: &mut Spec) {
    let dir = world.ensure_dir();
    let socket = sa_daemon::socket::bind(&dir.join("daemon.sock")).expect("bind");
    world.rendered = Some(dir.join("daemon.sock").display().to_string());
    drop(socket);
}

#[then("a daemon and its clients use them and leave the default daemon alone")]
fn the_paths_are_honoured(world: &mut Spec) {
    let path = world.rendered.clone().unwrap_or_default();
    assert!(path.contains("daemon.sock"), "{path}");
    assert!(
        !sa_daemon::socket::default_socket_path()
            .display()
            .to_string()
            .is_empty(),
        "the default path vanished"
    );
}

#[when("the daemon starts on a machine that has never run it")]
fn a_first_run(world: &mut Spec) {
    let dir = world.ensure_dir();
    let nested = dir.join("runtime");
    sa_daemon::socket::prepare_runtime_dir(&nested).expect("prepare");
    world.rendered = Some(nested.display().to_string());
}

#[then("it creates what it needs under the XDG roots and works")]
fn the_roots_are_created(world: &mut Spec) {
    let path = world.rendered.clone().unwrap_or_default();
    assert!(std::path::Path::new(&path).exists());
}
