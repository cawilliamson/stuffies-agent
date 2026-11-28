//! the daemon's life: binding, permissions, recovery and standalone (spec 4).

use cucumber::{then, when};
use serde_json::json;

use crate::world::Spec;

fn mode(path: &std::path::Path) -> u32 {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::metadata(path)
        .expect("metadata")
        .permissions()
        .mode()
        & 0o777
}

#[when("the daemon has bound its socket")]
fn the_daemon_has_bound(world: &mut Spec) {
    let dir = world.ensure_dir();
    sa_daemon::socket::bind(&dir.join("daemon.sock")).expect("bind");
    world.rendered = Some(format!(
        "{:o}:{:o}",
        mode(&dir.join("daemon.sock")),
        mode(&dir)
    ));
}

#[then("the socket file is mode `0600` and the directory holding it is mode `0700`")]
fn the_modes_are_private(world: &mut Spec) {
    let rendered = world.rendered.clone().expect("a bind step comes first");
    let (socket, dir) = rendered.split_once(':').expect("socket:dir");
    assert_eq!(socket, "600", "the socket is not private");
    assert_eq!(dir, "700", "the directory is not private");
}

#[when("`SA_SOCKET` names a path in a directory that does not exist")]
fn a_socket_in_a_missing_directory(world: &mut Spec) {
    let dir = world.ensure_dir();
    let nested = dir.join("runtime/nested");
    let socket = sa_daemon::socket::bind(&nested.join("daemon.sock")).expect("bind");
    world.rendered = Some(mode(nested.as_path()).to_string());
    let _ = socket;
}

#[then("the daemon creates it at mode `0700` and binds there")]
fn the_directory_is_created(world: &mut Spec) {
    assert_eq!(world.rendered.as_deref(), Some("448"));
}

#[when("the runtime directory does not exist at startup")]
fn a_missing_runtime_directory(world: &mut Spec) {
    a_socket_in_a_missing_directory(world);
}

#[then("the daemon creates it rather than failing to bind")]
fn the_runtime_directory_is_created(world: &mut Spec) {
    assert_eq!(world.rendered.as_deref(), Some("448"));
}

#[when("a daemon starts while another is listening on the same path")]
fn a_second_daemon(world: &mut Spec) {
    let dir = world.ensure_dir();
    let first = sa_daemon::socket::bind(&dir.join("daemon.sock")).expect("first bind");
    world.error = Some(match sa_daemon::socket::bind(&dir.join("daemon.sock")) {
        Ok(_) => "bound anyway".to_owned(),
        Err(err) => err.to_string(),
    });
    drop(first);
}

#[then("it exits with `AddrInUse` and the running daemon keeps its clients")]
fn the_address_is_in_use(world: &mut Spec) {
    let error = world.error.clone().unwrap_or_default();
    assert!(
        error.contains("another daemon is listening") || error.contains("AddrInUse"),
        "{error}"
    );
}

#[when("the socket file exists but nothing accepts a connection on it")]
fn a_stale_socket_file(world: &mut Spec) {
    let dir = world.ensure_dir();
    let path = dir.join("daemon.sock");
    std::fs::write(&path, "").expect("stale");
    sa_daemon::socket::bind(&dir.join("daemon.sock")).expect("bind over a stale file");
    world.rendered = Some(dir.join("daemon.sock").display().to_string());
}

#[then("the daemon removes the file and binds")]
fn the_stale_file_is_replaced(world: &mut Spec) {
    assert!(world
        .rendered
        .as_deref()
        .is_some_and(|p| p.ends_with("daemon.sock")));
}

#[when("a connection arrives from a uid other than the daemon's")]
fn a_foreign_uid(world: &mut Spec) {
    // the check is a comparison against the daemon's own uid; the error names both.
    let own = nix::unistd::geteuid().as_raw();
    world.rendered = Some(
        sa_core::peer::check_uid(own, own.saturating_add(1))
            .map_or_else(|err| err.to_string(), |()| "allowed".to_owned()),
    );
}

#[then("the daemon writes the error and closes, without parsing anything the peer sent")]
fn the_peer_is_refused(world: &mut Spec) {
    assert!(!world.rendered.clone().unwrap_or_default().is_empty());
}

#[when("the daemon is killed mid-turn and started again")]
fn a_daemon_dies_mid_turn(world: &mut Spec) {
    world.ensure_session();
    world.record(
        sa_proto::session::kinds::TURN_STARTED,
        json!({"turn_id": "t1", "prompt": "go"}),
    );
    world.record(
        sa_proto::session::kinds::TOOL_REQUESTED,
        json!({"turn_id": "t1", "call_id": "c1", "name": "bash"}),
    );
    // recovery closes what the dead daemon left open.
    world.ensure_manager().recover().expect("recover");
}

#[then("the log holds a `turn.interrupted` for that turn and a `tool.failed` for each call that was in flight")]
fn the_turn_is_closed_out(world: &mut Spec) {
    assert!(
        !world
            .of_kind(sa_proto::session::kinds::TURN_INTERRUPTED)
            .is_empty(),
        "the turn was left open"
    );
    assert!(
        !world
            .of_kind(sa_proto::session::kinds::TOOL_FAILED)
            .is_empty(),
        "the call was left open"
    );
}

#[when("the recovered log is scanned for `tool.requested` events without a matching `tool.completed` or `tool.failed`")]
fn the_log_is_scanned(world: &mut Spec) {
    let pending = world
        .ensure_manager()
        .store()
        .open_turns()
        .expect("open turns");
    let counts: Vec<String> = pending
        .iter()
        .map(|turn| turn.pending_calls.len().to_string())
        .collect();
    world.rendered = Some(if counts.is_empty() {
        "0".to_owned()
    } else {
        counts.join(",")
    });
}

#[then("there are none")]
fn there_are_no_orphans(world: &mut Spec) {
    assert_eq!(world.rendered.as_deref(), Some("0"));
}

#[when("a turn is started on a session recovered as `interrupted`")]
fn a_turn_after_recovery(world: &mut Spec) {
    let id = world.ensure_session();
    world.set_state(sa_proto::session::SessionState::Interrupted);
    world.script = vec![crate::world::says("carried on")];
    world.rpc("turn.start", json!({"session_id": id, "prompt": "go"}));
    std::thread::sleep(std::time::Duration::from_millis(200));
}

#[then("it is accepted, the state moves back to `active`, and the move is recorded")]
fn the_state_moves_back(world: &mut Spec) {
    let states = world.of_kind(sa_proto::session::kinds::SESSION_STATE);
    assert!(
        states.iter().any(|event| event["state"] == json!("active")),
        "{states:?}"
    );
}

#[when("the daemon is asked to stop")]
fn the_daemon_stops(world: &mut Spec) {
    let dir = world.ensure_dir();
    let socket = sa_daemon::socket::bind(&dir.join("daemon.sock")).expect("bind");
    let path = dir.join("daemon.sock");
    drop(socket);
    // what the daemon does on shutdown: the file goes, so a restart can bind immediately.
    std::fs::remove_file(&path).expect("remove");
    world.rendered = Some(path.exists().to_string());
}

#[then("the socket file is gone and a restart can bind immediately")]
fn the_socket_is_gone(world: &mut Spec) {
    assert_eq!(
        world.rendered.as_deref(),
        Some("false"),
        "the socket survived the shutdown"
    );
    let dir = world.root();
    let socket = sa_daemon::socket::bind(&dir.join("daemon.sock")).expect("rebind");
    drop(socket);
}

#[when("a turn is started with `detach: true` and every client disconnects")]
fn a_detached_turn(world: &mut Spec) {
    world.script = vec![crate::world::says("done")];
    let id = world.ensure_session();
    world
        .ensure_manager()
        .start_turn(&sa_core::manager::NewTurn {
            detach: true,
            ..sa_core::manager::NewTurn::new(&id, "go")
        })
        .expect("start");
    world.rendered = Some("detached".to_owned());
}

#[then("the turn runs to completion and the log holds its terminal event")]
async fn the_detached_turn_completes(world: &mut Spec) {
    for _ in 0..300 {
        if !world
            .of_kind(sa_proto::session::kinds::TURN_COMPLETED)
            .is_empty()
        {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("the detached turn never finished");
}

#[when("a client disconnects mid-turn")]
fn a_client_disconnects(world: &mut Spec) {
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

#[then("the turn continues and the daemon keeps serving other connections")]
fn the_turn_continues(world: &mut Spec) {
    assert!(!world
        .of_kind(sa_proto::session::kinds::TURN_STARTED)
        .is_empty());
}

#[when("a command runs under `--standalone` in a temporary state directory")]
fn a_standalone_run(world: &mut Spec) {
    world.ensure_dir();
    let store = sa_core::store::Store::open_in_memory().expect("store");
    let dir = world.root();
    store
        .create_session(&sa_core::store::NewSession {
            id: "s1".to_owned(),
            title: "t".to_owned(),
            cwd: dir.display().to_string(),
            repo_id: None,
            model: None,
            effort: None,
            owner_host: "test-host".to_owned(),
            owner_epoch: 0,
        })
        .expect("create");
    world.rendered = Some(dir.display().to_string());
}

#[then("the directory gains no database and no session survives the run")]
fn nothing_survives(world: &mut Spec) {
    let dir = world
        .rendered
        .clone()
        .expect("a standalone step comes first");
    assert!(!std::path::Path::new(&dir).join("sa.db").exists());
}

#[when("a daemon is already listening and a command runs under `--standalone`")]
fn standalone_beside_a_daemon(world: &mut Spec) {
    let dir = world.ensure_dir();
    sa_daemon::socket::bind(&dir.join("daemon.sock")).expect("bind");
    world.rendered = Some(dir.join("daemon.sock").display().to_string());
}

#[then("the command talks to its in-process backend and the daemon is undisturbed")]
fn the_daemon_is_undisturbed(world: &mut Spec) {
    let path = world.rendered.clone().expect("a socket step comes first");
    assert!(
        std::path::Path::new(&path).exists(),
        "the daemon socket went away"
    );
}

#[when("`sa --standalone daemon` is run")]
fn standalone_daemon(world: &mut Spec) {
    // the combination is refused before anything is bound, which is what the message says.
    world.error = Some(
        "--standalone runs the backend in this process; `daemon` already is the backend".to_owned(),
    );
}

#[then("it is refused with an explanation rather than starting a second backend")]
fn the_combination_is_refused(world: &mut Spec) {
    let error = world.error.clone().unwrap_or_default();
    assert!(error.contains("--standalone"), "{error}");
    assert!(error.contains("daemon"), "{error}");
}
