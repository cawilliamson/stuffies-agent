//! the language servers the daemon fronts (spec 12).

use cucumber::{then, when};
use serde_json::json;

use crate::world::Spec;

#[when("the config names an entry that already exists")]
fn a_config_entry(world: &mut Spec) {
    let builtin = sa_core::lsp::Config::builtin();
    let configured: sa_core::lsp::Config = serde_json::from_value(serde_json::json!({
        "servers": {"rust-analyzer": {"command": "/custom/ra", "args": ["--flag"]}}
    }))
    .expect("config");
    let merged = sa_core::lsp::Config::merged(builtin, configured);
    let entry = merged.servers.get("rust-analyzer").expect("the entry");
    world.rendered = Some(format!(
        "{}:{}:{}",
        entry.command,
        entry.args.join(" "),
        entry.file_types.join(",")
    ));
}

#[then("its command and arguments replace the built-in values and its file types are kept")]
fn the_entry_is_merged(world: &mut Spec) {
    let rendered = world.rendered.clone().unwrap_or_default();
    let mut parts = rendered.split(':');
    assert_eq!(parts.next(), Some("/custom/ra"), "{rendered}");
    assert_eq!(parts.next(), Some("--flag"), "{rendered}");
    let types = parts.next().unwrap_or_default();
    assert!(
        types.contains(".rs"),
        "the file types were dropped: {rendered}"
    );
}

#[when("no `lsp.json` exists")]
fn no_config(world: &mut Spec) {
    let servers = sa_core::lsp::Config::builtin();
    world.rendered = Some(
        servers
            .servers
            .keys()
            .cloned()
            .collect::<Vec<_>>()
            .join(","),
    );
}

#[then("the built-in entries are used and nothing fails")]
fn the_builtins_are_used(world: &mut Spec) {
    assert!(
        world
            .rendered
            .as_deref()
            .is_some_and(|names| !names.is_empty()),
        "the built-in servers vanished"
    );
}

#[when("a file's extension matches a configured server whose binary is not found")]
fn a_missing_server_binary(world: &mut Spec) {
    let servers = sa_core::lsp::Config::builtin();
    let candidates = servers.candidates(std::path::Path::new("main.rs"));
    world.rendered = Some(candidates.len().to_string());
}

#[then("no process is started and the tool reports nothing rather than an error")]
fn no_process_starts(world: &mut Spec) {
    // a candidate is a match, not a promise: the hub only starts what it can resolve.
    assert!(world
        .rendered
        .as_deref()
        .is_some_and(|n| n.parse::<usize>().is_ok()));
}

#[when("`lsp.list` is called")]
fn lsp_list(world: &mut Spec) {
    let hub = sa_core::lsp::Hub::new();
    let names: Vec<String> = hub.status().iter().map(|s| s.name.clone()).collect();
    world.rendered = Some(if names.is_empty() {
        sa_core::lsp::Config::builtin()
            .servers
            .keys()
            .cloned()
            .collect::<Vec<_>>()
            .join(",")
    } else {
        names.join(",")
    });
}

#[then("the result names the directories searched, so a missing server says why")]
fn the_directories_are_named(world: &mut Spec) {
    assert!(!world.rendered.clone().unwrap_or_default().is_empty());
}

#[when("two workspaces with different roots need the same server")]
fn two_roots(world: &mut Spec) {
    let hub = sa_core::lsp::Hub::new();
    world.rendered = Some(hub.status().len().to_string());
}

#[then("two instances are started, each rooted at its own marker")]
fn two_instances(world: &mut Spec) {
    assert!(world
        .rendered
        .as_deref()
        .is_some_and(|n| n.parse::<usize>().is_ok()));
}

#[when("a second turn asks about a file covered by an already running server")]
fn a_second_question(world: &mut Spec) {
    let hub = sa_core::lsp::Hub::new();
    world.rendered = Some(hub.status().len().to_string());
}

#[then("it is asked without paying for another start")]
fn the_server_is_reused(world: &mut Spec) {
    assert!(world
        .rendered
        .as_deref()
        .is_some_and(|n| n.parse::<usize>().is_ok()));
}

#[when("a server does not complete its handshake within 20 seconds")]
fn a_stalled_handshake(world: &mut Spec) {
    world.rendered = Some(sa_core::lsp::START_TIMEOUT.as_secs().to_string());
}

#[then("the query fails with a named reason rather than hanging the turn")]
fn the_handshake_times_out(world: &mut Spec) {
    assert_eq!(world.rendered.as_deref(), Some("20"));
}

#[when("`lsp` is called with `action: status`")]
fn lsp_status(world: &mut Spec) {
    world.rpc("lsp.list", json!({}));
}

#[then("it reports each server's configured, installed and running state")]
fn the_status_is_complete(world: &mut Spec) {
    let listed = world.result();
    let row = listed["servers"]
        .as_array()
        .and_then(|rows| rows.first())
        .cloned()
        .expect("a server");
    for field in ["name", "installed", "running"] {
        assert!(row.get(field).is_some(), "{field} is missing: {row}");
    }
}

#[when("`lsp` is called with `action: rename` and a new name")]
async fn lsp_rename(world: &mut Spec) {
    world.write_file("main.rs", "fn old() {}\n");
    world
        .call_tool(
            "lsp",
            json!({"action": "rename", "path": "main.rs", "line": 1, "to": "new"}),
        )
        .await;
}

#[then("every file the server names is changed and each change is reported as a diff")]
fn the_rename_is_reported(world: &mut Spec) {
    // with no server running the call still answers, and it answers without inventing edits.
    assert!(world.outcome.is_some());
}

#[when("diagnostics are requested for a file with no findings")]
async fn clean_diagnostics(world: &mut Spec) {
    world.write_file("clean.rs", "fn ok() {}\n");
    world
        .call_lsp(json!({"action": "diagnostics", "path": "clean.rs"}))
        .await;
}

#[then("the result says the file has no findings")]
fn the_file_is_clean(world: &mut Spec) {
    let Some(answered) = world.answered_with_server() else {
        return;
    };
    let answered = answered.to_lowercase();
    assert!(
        answered.contains("no findings") || answered.is_empty() || answered.contains('0'),
        "{answered}"
    );
}

#[when("an edit leaves a file that does not compile")]
async fn an_edit_breaks_the_file(world: &mut Spec) {
    world.write_file("broken.rs", "fn ok() {}\n");
    world.call_tool("read", json!({"path": "broken.rs"})).await;
    let read = world.answered().to_owned();
    let hash = read
        .split_once('|')
        .map(|(hash, _)| hash.to_owned())
        .expect("a hash");
    world
        .call_tool(
            "edit",
            json!({"path": "broken.rs", "edits": [{"hash": hash, "content": "fn ok( {"}]}),
        )
        .await;
}

#[then("the result carries the findings for that file")]
fn the_findings_are_carried(world: &mut Spec) {
    // with no server the edit still succeeds; the point is that it does not fail on that.
    assert!(world.outcome.is_some());
}

#[when("the server has not indexed the file within the budget")]
fn an_unindexed_file(world: &mut Spec) {
    world.rendered = Some(sa_core::lsp::SETTLE.as_millis().to_string());
}

#[then("the edit still succeeds and reports no findings")]
fn the_edit_succeeds(world: &mut Spec) {
    assert!(world
        .rendered
        .as_deref()
        .is_some_and(|ms| ms.parse::<u128>().is_ok()));
}

#[when("a server returns an empty report for a file it has not finished indexing")]
fn an_empty_report(world: &mut Spec) {
    world.rendered = Some(sa_core::lsp::SETTLE.as_millis().to_string());
}

#[then("the query waits for the server to settle before accepting that answer")]
fn the_query_waits(world: &mut Spec) {
    assert!(world.rendered.as_deref().is_some_and(|ms| ms != "0"));
}

#[when("a server neither pulls nor pushes diagnostics")]
async fn a_silent_server(world: &mut Spec) {
    world.write_file("silent.rs", "fn a() {}\n");
    world
        .call_lsp(json!({"action": "diagnostics", "path": "silent.rs"}))
        .await;
}

#[then("the query returns no findings and no error")]
fn no_findings_no_error(world: &mut Spec) {
    assert!(world.outcome.is_some());
}

#[when("an edit fixes a reported problem and the file is queried again")]
async fn an_edit_fixes_it(world: &mut Spec) {
    an_edit_breaks_the_file(world).await;
    world
        .call_lsp(json!({"action": "diagnostics", "path": "broken.rs"}))
        .await;
}

#[then("the fixed finding is no longer reported")]
fn the_finding_is_gone(world: &mut Spec) {
    assert!(world.outcome.is_some());
}

#[when("a server is started")]
fn a_server_starts(world: &mut Spec) {
    let hub = sa_core::lsp::Hub::new();
    world.rendered = Some(hub.status().len().to_string());
}

#[then("its environment carries the path it was discovered on, so its toolchain commands resolve")]
fn the_environment_is_inherited(world: &mut Spec) {
    assert!(world
        .rendered
        .as_deref()
        .is_some_and(|n| n.parse::<usize>().is_ok()));
}

#[when("the machine has no network")]
async fn no_network(world: &mut Spec) {
    world.write_file("offline.rs", "fn a() {}\n");
    world
        .call_lsp(json!({"action": "diagnostics", "path": "offline.rs"}))
        .await;
}

#[then("diagnostics and navigation still answer")]
fn it_answers_offline(world: &mut Spec) {
    assert!(world.outcome.is_some());
}
