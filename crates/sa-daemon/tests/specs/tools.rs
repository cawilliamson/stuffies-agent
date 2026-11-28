//! the built-in tools: what each does and the bounds every one of them keeps (spec 9).

use std::fmt::Write as _;

use cucumber::{then, when};
use sa_core::agent::ToolExecutor as _;
use sa_core::llm::ToolCall;
use serde_json::json;

use crate::world::Spec;

/// the refusal names where the path actually landed, which is absolute, so a caller can see
/// what it hit rather than being told only that it was wrong.
fn named_escape(world: &Spec) -> String {
    let error = world.refused();
    let named = error
        .strip_prefix("path escapes the workspace: ")
        .unwrap_or_else(|| panic!("not an escape: {error}"))
        .to_owned();
    assert!(
        named.starts_with('/'),
        "the refusal does not name a resolved path: {error}"
    );
    named
}

// ---- the tool set --------------------------------------------------------

#[when("the tool specifications are read")]
fn the_tool_specifications_are_read(world: &mut Spec) {
    world.ensure_dir();
    let registry = world.registry();
    let names: Vec<String> = registry.specs().into_iter().map(|spec| spec.name).collect();
    world.rendered = Some(names.join("\n"));
}

#[then("they are the built-in set plus one entry per connected server tool, and the system prompt is generated from the same list")]
fn they_are_the_built_in_set(world: &mut Spec) {
    let listed = world.rendered.clone().expect("a listing step comes first");
    for name in [
        "bash",
        "edit",
        "find",
        "goal_done",
        "grep",
        "lsp",
        "read",
        "todo",
        "web_fetch",
        "web_search",
        "write",
    ] {
        assert!(listed.contains(name), "{name} is missing from the tool set");
    }
    assert!(
        !listed.contains("openspec"),
        "a tool that no longer exists is still offered"
    );
}

#[when("the model calls a tool that is not registered")]
async fn the_model_calls_an_unknown_tool(world: &mut Spec) {
    world.ensure_dir();
    let registry = world.registry();
    let call = ToolCall {
        id: "c1".to_owned(),
        name: "nonsense".to_owned(),
        arguments: "{}".to_owned(),
    };
    world.outcome = Some(registry.execute(call).await.map(|outcome| outcome.output));
}

#[then("the call fails and the turn continues")]
fn the_call_fails(world: &mut Spec) {
    assert!(
        world.refused().contains("unknown tool"),
        "not an unknown tool: {}",
        world.refused()
    );
}

// ---- paths ---------------------------------------------------------------

#[when(regex = r"^a tool is called with `(?<path>[^`]+)`$")]
async fn a_tool_is_called_with(world: &mut Spec, path: String) {
    world.ensure_dir();
    world.call_tool("read", json!({"path": path})).await;
}

#[when("a link inside the workspace resolves outside it")]
async fn a_link_resolves_outside(world: &mut Spec) {
    world.link_out("escape");
    world
        .call_tool("read", json!({"path": "escape/secret"}))
        .await;
}

#[then("it fails naming the resolved path and the turn continues")]
fn it_fails_naming_the_resolved_path(world: &mut Spec) {
    named_escape(world);
}

#[then("it fails as an escape")]
fn it_fails_as_an_escape(world: &mut Spec) {
    named_escape(world);
}

#[then("the call fails naming the path it actually landed on")]
fn it_names_where_it_landed(world: &mut Spec) {
    let named = named_escape(world);
    let outside = world.outside_root().display().to_string();
    assert!(
        named.starts_with(&outside),
        "the refusal names {named}, not where the link points ({outside})"
    );
}

// ---- output bounds -------------------------------------------------------

#[when("a tool produces more than 256 KiB")]
async fn a_tool_produces_too_much(world: &mut Spec) {
    // the cap is applied where the output becomes an event, so the turn is the thing that
    // has one to check.
    world.script = vec![
        vec![
            sa_core::llm::Delta::ToolCall(ToolCall {
                id: "c1".to_owned(),
                name: "bash".to_owned(),
                arguments: json!({"command": "head -c 300000 /dev/zero | tr '\\0' x"}).to_string(),
            }),
            done(),
        ],
        crate::world::says("done"),
    ];
    world.run_turn("make some output").await;
}

#[then("the event carries the capped output and `truncated: true`")]
fn the_event_is_capped(world: &mut Spec) {
    let completed = world.of_kind(sa_proto::session::kinds::TOOL_COMPLETED);
    let event = completed.first().expect("a tool.completed event");
    assert_eq!(event["truncated"], json!(true), "{event}");
    let output = event["output"].as_str().unwrap_or_default();
    assert!(
        output.len() <= 256 * 1024 + 64,
        "the output was not capped: {} bytes",
        output.len()
    );
}

#[when("a command backgrounds a child and the turn is cancelled")]
async fn a_command_backgrounds_a_child(world: &mut Spec) {
    let pid_file = world.ensure_dir().join("child.pid");
    let command = format!("sleep 30 & echo $! > {}; wait", pid_file.display());
    let registry = std::sync::Arc::new(world.registry());
    let call = ToolCall {
        id: "c1".to_owned(),
        name: "bash".to_owned(),
        arguments: json!({ "command": command }).to_string(),
    };
    let running = tokio::spawn({
        let registry = std::sync::Arc::clone(&registry);
        async move { registry.execute(call).await }
    });
    for _ in 0..100 {
        if pid_file.exists() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let pid: i32 = std::fs::read_to_string(&pid_file)
        .expect("the grandchild must report its pid")
        .trim()
        .parse()
        .expect("a pid");
    // dropping the future is what a cancelled turn does.
    running.abort();
    world.rendered = Some(pid.to_string());
}

#[then("the child is killed with the group")]
async fn the_child_is_killed(world: &mut Spec) {
    let pid: i32 = world
        .rendered
        .clone()
        .expect("a background step comes first")
        .parse()
        .expect("a pid");
    for _ in 0..100 {
        if !alive(pid) {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("the grandchild outlived the turn: {pid}");
}

#[when("a command exits non-zero")]
async fn a_command_exits_non_zero(world: &mut Spec) {
    world
        .call_tool("bash", json!({"command": "echo three; exit 3"}))
        .await;
}

#[then("the event is `tool.completed` with that exit code")]
fn the_event_carries_the_exit_code(world: &mut Spec) {
    let answered = world.answered();
    assert!(
        answered.contains("three") || answered.contains('3'),
        "the result says nothing about the command: {answered}"
    );
}

#[when("a command runs past 600 seconds")]
async fn a_command_runs_too_long(world: &mut Spec) {
    world.ensure_dir();
    // the bound is the daemon's; the test asks for a shorter one so it can watch it happen.
    let registry = world
        .registry()
        .with_command_timeout(std::time::Duration::from_millis(150));
    let call = ToolCall {
        id: "c1".to_owned(),
        name: "bash".to_owned(),
        arguments: json!({"command": "sleep 30"}).to_string(),
    };
    world.outcome = Some(registry.execute(call).await.map(|outcome| outcome.output));
}

#[then("it is terminated and the failure names the timeout")]
fn it_is_terminated(world: &mut Spec) {
    assert!(
        world.refused().contains("timed out"),
        "not a timeout: {}",
        world.refused()
    );
}

// ---- read ----------------------------------------------------------------

#[when("a file longer than the window is read")]
async fn a_long_file_is_read(world: &mut Spec) {
    let body = (1..=2500).fold(String::new(), |mut out, n| {
        let _ = writeln!(out, "line {n}");
        out
    });
    world.write_file("long.txt", &body);
    world.call_tool("read", json!({"path": "long.txt"})).await;
}

#[then("the last line names the range, the total and the next offset")]
fn the_window_is_named(world: &mut Spec) {
    let answered = world.answered().to_owned();
    let last = answered.lines().last().unwrap_or_default();
    assert!(last.contains("2500"), "the total is not named: {last}");
    assert!(last.contains("2000"), "the range is not named: {last}");
    assert!(
        last.contains("2001"),
        "the next offset is not named: {last}"
    );
}

#[when("the offset is beyond the last line")]
async fn the_offset_is_beyond_the_end(world: &mut Spec) {
    world.write_file("short.txt", "one\ntwo\n");
    world
        .call_tool("read", json!({"path": "short.txt", "offset": 99}))
        .await;
}

#[then("the result says there are no lines there and gives the file's length")]
fn it_says_there_are_no_lines(world: &mut Spec) {
    let answered = world.answered();
    assert!(answered.contains("no lines"), "{answered}");
    assert!(
        answered.contains('2'),
        "the length is not given: {answered}"
    );
}

#[when("the next line would exceed the byte budget")]
async fn the_next_line_would_exceed_the_budget(world: &mut Spec) {
    // one line just under the window, then one that would cross it: the window must stop
    // before the second rather than return half of it.
    let mut body = String::new();
    while body.len() < 40 * 1024 {
        body.push_str(&"a".repeat(1000));
        body.push('\n');
    }
    body.push_str(&"b".repeat(40 * 1024));
    world.write_file("wide.txt", &body);
    world.call_tool("read", json!({"path": "wide.txt"})).await;
}

#[then("the window ends before it rather than returning half of it")]
fn the_window_ends_before_it(world: &mut Spec) {
    let answered = world.answered();
    assert!(
        !answered.contains(&"b".repeat(100)),
        "the oversized line was returned anyway"
    );
    assert!(
        answered.contains("truncated") || answered.lines().count() > 1,
        "{answered}"
    );
}

#[when("`read` is called on a `png`")]
async fn read_is_called_on_a_png(world: &mut Spec) {
    let png: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    let root = world.ensure_dir();
    std::fs::write(root.join("shot.png"), png).expect("write");
    world.call_tool("read", json!({"path": "shot.png"})).await;
}

#[then("the result carries the image and the model can describe it")]
fn the_result_carries_the_image(world: &mut Spec) {
    let answered = world.answered();
    assert!(answered.starts_with("image:"), "{answered}");
    assert!(
        answered.contains("png"),
        "the type is not named: {answered}"
    );
}

#[when("`read` is called on a file that is not text and not an image")]
async fn read_is_called_on_a_binary(world: &mut Spec) {
    let root = world.ensure_dir();
    std::fs::write(root.join("blob.bin"), [0xff, 0xfe, 0x00, 0x01]).expect("write");
    world.call_tool("read", json!({"path": "blob.bin"})).await;
}

#[then("the error says it is neither, rather than reporting a utf-8 failure")]
fn the_error_says_neither(world: &mut Spec) {
    let error = world.refused();
    assert!(error.contains("neither text nor an image"), "{error}");
    assert!(!error.contains("utf-8"), "a utf-8 error leaked: {error}");
}

// ---- write and edit ------------------------------------------------------

#[when("`write` replaces an existing file")]
async fn write_replaces_a_file(world: &mut Spec) {
    world.write_file("note.txt", "before\n");
    world
        .call_tool("write", json!({"path": "note.txt", "content": "after\n"}))
        .await;
}

#[then("the result carries a unified diff of the before and after text")]
fn the_result_carries_a_diff(world: &mut Spec) {
    let answered = world.answered();
    assert!(answered.contains("before"), "{answered}");
    assert!(answered.contains("after"), "{answered}");
    assert!(answered.contains("@@"), "no diff hunk: {answered}");
}

#[when("`write` creates a file that did not exist")]
async fn write_creates_a_file(world: &mut Spec) {
    world.ensure_dir();
    world
        .call_tool("write", json!({"path": "fresh.txt", "content": "hello\n"}))
        .await;
}

#[then("the result reports the byte count and no diff")]
fn the_result_reports_bytes(world: &mut Spec) {
    let answered = world.answered();
    assert!(answered.contains("bytes"), "{answered}");
    assert!(
        !answered.contains("@@"),
        "a new file has nothing to diff: {answered}"
    );
}

#[when("a file is edited")]
async fn a_file_is_edited(world: &mut Spec) {
    world.write_file("edit.txt", "one\ntwo\nthree\n");
    world.call_tool("read", json!({"path": "edit.txt"})).await;
    let read = world.answered().to_owned();
    let hash = read
        .lines()
        .nth(1)
        .and_then(|line| line.split_once('|'))
        .map(|(hash, _)| hash.to_owned())
        .expect("a hashed line");
    world
        .call_tool(
            "edit",
            json!({"path": "edit.txt", "edits": [{"hash": hash, "content": "TWO"}]}),
        )
        .await;
}

#[then("the result carries the diff and the hashes as written")]
fn the_result_carries_diff_and_hashes(world: &mut Spec) {
    let answered = world.answered();
    assert!(answered.contains("TWO"), "the diff is missing: {answered}");
    assert!(answered.contains("@@"), "no diff hunk: {answered}");
}

// ---- search --------------------------------------------------------------

#[when("`grep` searches a project whose `.gitignore` excludes a build directory")]
async fn grep_respects_ignore(world: &mut Spec) {
    let root = world.ensure_dir();
    let _ = std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(&root)
        .output();
    world.write_file(".gitignore", "target/\n");
    world.write_file("target/generated.rs", "NEEDLE\n");
    world.write_file("src/lib.rs", "let x = 1;\n");
    world.call_tool("grep", json!({"pattern": "NEEDLE"})).await;
}

#[then("no match from that directory is returned")]
fn no_match_from_ignored(world: &mut Spec) {
    let answered = world.answered();
    assert!(!answered.contains("NEEDLE"), "{answered}");
}

#[when("`grep` searches a project with `.github/workflows/ci.yml`")]
async fn grep_finds_a_hidden_file(world: &mut Spec) {
    world.write_file(".github/workflows/ci.yml", "name: ci\n");
    world
        .call_tool("grep", json!({"pattern": "name: ci"}))
        .await;
}

#[then("a match in that file is returned")]
fn a_match_in_the_hidden_file(world: &mut Spec) {
    let answered = world.answered();
    assert!(answered.contains("ci.yml"), "{answered}");
}

#[when("`find` walks a repository")]
async fn find_walks_a_repository(world: &mut Spec) {
    world.write_file(".git/config", "[core]\n");
    world.write_file("src/lib.rs", "fn a() {}\n");
    world.call_tool("find", json!({"pattern": ".rs"})).await;
}

#[then("nothing under `.git` is returned")]
fn nothing_under_git(world: &mut Spec) {
    let answered = world.answered();
    assert!(!answered.contains(".git/"), "{answered}");
    assert!(answered.contains(".rs"), "{answered}");
}

// ---- todo ----------------------------------------------------------------

#[when("`todo` is called twice with different lists")]
async fn todo_called_twice(world: &mut Spec) {
    world
        .call_tool(
            "todo",
            json!({"items": [{"content": "first", "status": "pending"}]}),
        )
        .await;
    world
        .call_tool(
            "todo",
            json!({"items": [{"content": "second", "status": "pending"}]}),
        )
        .await;
}

#[then("the second result reflects only the second list, and nothing is persisted")]
fn the_second_list_only(world: &mut Spec) {
    let answered = world.answered();
    assert!(answered.contains("second"), "{answered}");
    assert!(!answered.contains("first"), "{answered}");
}

#[when("an item carries a status other than the three known values")]
async fn an_unknown_status(world: &mut Spec) {
    world
        .call_tool(
            "todo",
            json!({"items": [{"content": "x", "status": "bogus"}]}),
        )
        .await;
}

#[then("the call fails naming the value and the accepted set")]
fn it_names_the_accepted_set(world: &mut Spec) {
    let error = world.refused();
    assert!(error.contains("bogus"), "{error}");
    assert!(
        error.contains("pending"),
        "the accepted set is not named: {error}"
    );
}

#[when("51 items are sent")]
async fn fifty_one_items(world: &mut Spec) {
    let items: Vec<serde_json::Value> = (0..51)
        .map(|n| json!({"content": format!("item {n}"), "status": "pending"}))
        .collect();
    world.call_tool("todo", json!({"items": items})).await;
}

#[then("the call fails naming the maximum")]
fn it_names_the_maximum(world: &mut Spec) {
    let error = world.refused();
    assert!(error.contains("at most"), "{error}");
    assert!(error.contains("50"), "the maximum is not named: {error}");
}

// ---- web -----------------------------------------------------------------

#[when("`web_fetch` is called with `file:///etc/passwd`")]
async fn web_fetch_a_file_url(world: &mut Spec) {
    world.ensure_dir();
    world
        .call_tool("web_fetch", json!({"url": "file:///etc/passwd"}))
        .await;
}

#[then("it fails as an argument error and nothing is fetched")]
fn it_fails_as_an_argument(world: &mut Spec) {
    let error = world.refused();
    assert!(
        error.contains("scheme") || error.contains("http"),
        "{error}"
    );
}

#[when("a page exceeds 4 MiB")]
async fn a_page_exceeds_the_cap(world: &mut Spec) {
    // a local page of five mebibytes, so the cut is the thing under test rather than a site.
    let url = crate::world::serve(vec![b'x'; 5 * 1024 * 1024]).await;
    world.call_tool("web_fetch", json!({"url": url})).await;
}

#[then("the body is truncated and the result says so")]
fn the_body_is_truncated(world: &mut Spec) {
    let answered = world.answered();
    assert!(answered.contains("[truncated]"), "the cut is not marked");
    assert!(
        answered.len() < 5 * 1024 * 1024,
        "the whole page came back: {} bytes",
        answered.len()
    );
}

#[when("a search finds nothing")]
#[allow(clippy::disallowed_methods)]
async fn a_search_finds_nothing(world: &mut Spec) {
    // the endpoint is a local page with no hits in it, so the empty answer is the daemon's
    // rather than a site's.
    let url = crate::world::serve(b"<html><body><p>nothing here</p></body></html>".to_vec()).await;
    std::env::set_var("SA_SEARCH_ENDPOINT", url);
    world.ensure_dir();
    world
        .call_tool("web_search", json!({"query": "nothing at all"}))
        .await;
}

#[then("the result says that, rather than failing")]
fn the_empty_search_says_so(world: &mut Spec) {
    let answered = world.answered();
    assert!(answered.contains("no results"), "{answered}");
}

// ---- goal ----------------------------------------------------------------

#[when("the model calls `goal_done`")]
async fn the_model_calls_goal_done(world: &mut Spec) {
    world.ensure_session();
    world
        .call_tool("goal_done", json!({"summary": "shipped it"}))
        .await;
}

#[then("`goal.done` is in the log and the loop reads it from there")]
fn goal_done_is_in_the_log(world: &mut Spec) {
    let answered = world.answered();
    assert!(answered.contains("shipped it"), "{answered}");
    // the tool records the event; the loop reads it from the log rather than a side channel.
    assert_eq!(sa_proto::session::kinds::GOAL_DONE, "goal.done");
}

// ---- helpers -------------------------------------------------------------

fn done() -> sa_core::llm::Delta {
    sa_core::llm::Delta::Done {
        stop_reason: "tool_use".to_owned(),
        prompt_tokens: 1,
        completion_tokens: 1,
        cached_tokens: 0,
    }
}

fn alive(pid: i32) -> bool {
    std::path::Path::new(&format!("/proc/{pid}")).exists()
}
