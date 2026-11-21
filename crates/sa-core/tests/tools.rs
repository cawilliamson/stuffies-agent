//! tool execution against a real filesystem (spec 9).
#![allow(clippy::expect_used, clippy::panic)]

use std::path::PathBuf;

use sa_core::agent::ToolExecutor;
use sa_core::llm::ToolCall;
use sa_core::tools::ToolRegistry;
use serde_json::json;

fn registry(workspace: &std::path::Path) -> ToolRegistry {
    ToolRegistry::new(workspace.to_path_buf())
}

fn call(name: &str, args: &serde_json::Value) -> ToolCall {
    ToolCall {
        id: "c1".to_owned(),
        name: name.to_owned(),
        arguments: args.to_string(),
    }
}

fn workspace() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("main.rs"), "fn main() {}\n").expect("write");
    std::fs::create_dir_all(dir.path().join("src")).expect("mkdir");
    std::fs::write(dir.path().join("src/lib.rs"), "pub fn hello() {}\n").expect("write");
    dir
}

async fn run(
    registry: &ToolRegistry,
    name: &str,
    args: serde_json::Value,
) -> Result<String, String> {
    registry
        .execute(call(name, &args))
        .await
        .map(|outcome| outcome.output)
}

#[tokio::test]
async fn read_returns_file_contents() {
    let dir = workspace();
    let reg = registry(dir.path());

    // the default form is addressable: every line carries the hash that names it.
    let hashed = run(&reg, "read", json!({"path": "main.rs"}))
        .await
        .expect("read");
    let line = hashed.lines().next().expect("a line");
    let (hash, content) = line.split_once('|').expect("hash|content");
    assert_eq!(hash.len(), 6, "a six character hash: {line:?}");
    assert!(hash.bytes().all(|b| b.is_ascii_hexdigit()), "{line:?}");
    assert_eq!(content, "fn main() {}", "the text is untouched");

    // and the raw form is one flag away.
    let raw = run(&reg, "read", json!({"path": "main.rs", "hashline": false}))
        .await
        .expect("read");
    assert_eq!(raw, "fn main() {}\n");
}

#[tokio::test]
async fn write_creates_a_file_and_reports_size() {
    let dir = workspace();
    let reg = registry(dir.path());
    let out = run(
        &reg,
        "write",
        json!({"path": "new.txt", "content": "hello"}),
    )
    .await
    .expect("write");
    assert!(out.contains('5'), "{out}");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("new.txt")).expect("read"),
        "hello"
    );
}

#[tokio::test]
async fn write_creates_missing_parent_directories() {
    let dir = workspace();
    let reg = registry(dir.path());
    run(&reg, "write", json!({"path": "a/b/c.txt", "content": "x"}))
        .await
        .expect("write");
    assert!(dir.path().join("a/b/c.txt").exists());
}

#[tokio::test]
async fn edit_replaces_one_unique_occurrence() {
    let dir = workspace();
    let reg = registry(dir.path());
    run(
        &reg,
        "edit",
        json!({"path": "main.rs", "old": "fn main()", "new": "fn other()"}),
    )
    .await
    .expect("edit");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("main.rs")).expect("read"),
        "fn other() {}\n"
    );
}

#[tokio::test]
async fn edit_refuses_an_ambiguous_match() {
    let dir = workspace();
    std::fs::write(dir.path().join("dup.txt"), "x x").expect("write");
    let reg = registry(dir.path());
    let err = run(
        &reg,
        "edit",
        json!({"path": "dup.txt", "old": "x", "new": "y"}),
    )
    .await
    .expect_err("ambiguous edit must be refused");
    assert!(err.contains("exactly one"), "{err}");
}

#[tokio::test]
async fn find_matches_names() {
    let dir = workspace();
    let reg = registry(dir.path());
    let out = run(&reg, "find", json!({"pattern": "lib.rs"}))
        .await
        .expect("find");
    assert!(out.contains("src/lib.rs"), "{out}");
}

#[tokio::test]
async fn grep_returns_matching_lines() {
    let dir = workspace();
    let reg = registry(dir.path());
    let out = run(&reg, "grep", json!({"pattern": "pub fn"}))
        .await
        .expect("grep");
    assert!(out.contains("src/lib.rs:1"), "{out}");
    assert!(out.contains("pub fn hello"), "{out}");
}

#[tokio::test]
async fn bash_runs_a_command() {
    let dir = workspace();
    let reg = registry(dir.path());
    let out = run(&reg, "bash", json!({"command": "echo hi"}))
        .await
        .expect("bash");
    // no section headers: the output is what the command printed, and nothing else.
    assert_eq!(out.trim(), "hi", "unexpected output: {out}");
}

#[tokio::test]
async fn bash_reports_the_exit_code_structurally() {
    let dir = workspace();
    let reg = registry(dir.path());
    let outcome = reg
        .execute(call("bash", &json!({"command": "exit 3"})))
        .await
        .expect("bash");
    assert_eq!(outcome.exit_code, Some(3), "the exit code must be reported");
}

#[tokio::test]
async fn bash_merges_stdout_and_stderr() {
    let dir = workspace();
    let reg = registry(dir.path());
    let out = run(&reg, "bash", json!({"command": "echo out; echo err 1>&2"}))
        .await
        .expect("bash");
    assert!(out.contains("out"), "{out}");
    assert!(out.contains("err"), "{out}");
    assert!(!out.contains("--- stdout ---"), "no headers: {out}");
}

#[tokio::test]
async fn bash_reports_a_non_zero_exit_as_a_result_not_a_failure() {
    let dir = workspace();
    let reg = registry(dir.path());
    let out = run(&reg, "bash", json!({"command": "git status"})).await;
    // git status in a non-repo exits non-zero; that is a result, not a tool error
    assert!(out.is_ok(), "{out:?}");
}

#[tokio::test]
async fn a_path_escaping_the_workspace_is_refused() {
    let dir = workspace();
    let reg = registry(dir.path());
    let err = run(&reg, "read", json!({"path": "../outside.txt"}))
        .await
        .expect_err("escape must be refused");
    assert!(err.contains("escapes"), "{err}");
}

#[tokio::test]
async fn bash_runs_in_the_workspace() {
    let dir = workspace();
    let reg = registry(dir.path());
    let out = run(&reg, "bash", json!({"command": "pwd"}))
        .await
        .expect("bash");
    let expected: PathBuf = std::fs::canonicalize(dir.path()).expect("canonical");
    assert!(out.contains(expected.to_str().expect("utf8")), "{out}");
}

#[tokio::test]
async fn an_unknown_tool_is_refused() {
    let dir = workspace();
    let reg = registry(dir.path());
    let err = run(&reg, "nope", json!({}))
        .await
        .expect_err("unknown tool");
    assert!(err.contains("unknown tool"), "{err}");
}

#[tokio::test]
async fn bash_inherits_the_environment_it_was_started_with() {
    let dir = workspace();
    let reg = registry(dir.path());
    let inherited = std::env::var("PATH").expect("PATH in the test process");

    let out = reg
        .execute(call("bash", &json!({"command": "printf '%s' \"$PATH\""})))
        .await
        .expect("bash");

    assert_eq!(
        out.output.trim(),
        inherited.trim(),
        "a command must see the daemon's PATH, not one rebuilt for it"
    );
}

#[tokio::test]
async fn bash_inherits_variables_beyond_path() {
    let dir = workspace();
    let reg = registry(dir.path());
    let expected = std::env::var("HOME").expect("HOME in the test process");

    let out = reg
        .execute(call("bash", &json!({"command": "printf '%s' \"$HOME\""})))
        .await
        .expect("bash");

    assert_eq!(out.output.trim(), expected.trim());
}

#[tokio::test]
async fn web_fetch_refuses_a_scheme_it_cannot_speak() {
    // the check runs before any request, so this test needs no network.
    let dir = workspace();
    let reg = registry(dir.path());
    let err = run(&reg, "web_fetch", json!({"url": "ftp://example.com/file"}))
        .await
        .expect_err("ftp is not fetched");
    assert!(err.contains("only http and https"), "{err}");
}

#[tokio::test]
async fn web_search_refuses_an_empty_query() {
    let dir = workspace();
    let reg = registry(dir.path());
    let err = run(&reg, "web_search", json!({"query": "   "}))
        .await
        .expect_err("an empty query is an argument error");
    assert!(err.contains("empty query"), "{err}");
}

#[tokio::test]
#[ignore = "network"]
async fn web_fetch_is_reachable_by_name() {
    let dir = workspace();
    let reg = registry(dir.path());
    let out = run(&reg, "web_fetch", json!({"url": "https://example.com"}))
        .await
        .expect("web_fetch");
    assert!(out.contains("Example Domain"), "{out}");
}

/// a registry with the session lens wired in, as the daemon builds it.
fn lensed(workspace: &std::path::Path) -> ToolRegistry {
    ToolRegistry::with_lens(
        workspace.to_path_buf(),
        std::sync::Arc::new(sa_core::mcp::Hub::empty()),
        std::sync::Arc::new(sa_core::lens::Lens::for_workspace(workspace)),
    )
}

/// a workspace carrying one structural rule, so a write has something to be checked against.
fn ruled_workspace() -> tempfile::TempDir {
    let dir = workspace();
    std::fs::create_dir_all(dir.path().join("rules")).expect("rules dir");
    std::fs::write(dir.path().join("sgconfig.yml"), "ruleDirs:\n  - rules\n").expect("sgconfig");
    std::fs::write(
        dir.path().join("rules/no-unwrap.yml"),
        "id: no-unwrap\nlanguage: rust\nseverity: error\nmessage: unwrap panics\nrule:\n  pattern: $X.unwrap()\n",
    )
    .expect("rule");
    dir
}

#[tokio::test]
async fn a_blind_edit_is_refused_until_the_file_has_been_read() {
    // the guard: an edit to text the model has not seen is an edit to a guess.
    let dir = workspace();
    let registry = lensed(dir.path());

    let refused = run(
        &registry,
        "edit",
        json!({"path": "main.rs", "old": "fn main() {}", "new": "fn main() { run(); }"}),
    )
    .await
    .expect_err("an unread file may not be edited");
    assert!(refused.contains("read it first"), "{refused}");

    // reading it is what licenses the edit.
    run(&registry, "read", json!({"path": "main.rs"}))
        .await
        .expect("read");
    run(
        &registry,
        "edit",
        json!({"path": "main.rs", "old": "fn main() {}", "new": "fn main() { run(); }"}),
    )
    .await
    .expect("a read file may be edited");
}

#[tokio::test]
async fn a_new_file_needs_no_read_before_it_is_written() {
    let dir = workspace();
    let registry = lensed(dir.path());
    run(
        &registry,
        "write",
        json!({"path": "new.rs", "content": "fn a() {}\n"}),
    )
    .await
    .expect("a file that did not exist could not have been read");
}

#[tokio::test]
async fn a_written_file_is_checked_against_the_workspace_rules() {
    let dir = ruled_workspace();
    let registry = lensed(dir.path());

    // a clean file says nothing about the lens.
    let clean = run(
        &registry,
        "write",
        json!({"path": "a.rs", "content": "fn a() {}\n"}),
    )
    .await
    .expect("write");
    assert!(!clean.contains("lens:"), "{clean}");

    // a finding is carried in the tool result, where the model reads it.
    let flagged = run(
        &registry,
        "write",
        json!({"path": "b.rs", "content": "fn b() {\n    let x = y.unwrap();\n}\n"}),
    )
    .await
    .expect("write");
    assert!(flagged.contains("lens: 1 finding"), "{flagged}");
    assert!(flagged.contains("b.rs:2:"), "the line is named: {flagged}");
    assert!(flagged.contains("unwrap panics"), "{flagged}");
    assert!(
        flagged.contains("error"),
        "the severity is named: {flagged}"
    );
}

#[tokio::test]
async fn the_outline_lists_a_files_declarations_with_their_ranges() {
    let dir = workspace();
    let registry = lensed(dir.path());
    let listed = run(&registry, "outline", json!({"path": "src/lib.rs"}))
        .await
        .expect("outline");
    assert!(listed.contains("hello"), "{listed}");
    assert!(listed.contains("function_item 1-1"), "{listed}");
}

#[tokio::test]
async fn a_search_respects_the_projects_own_ignore_files() {
    // the regression: a plain directory walk searched the build output and the git
    // directory, so a grep returned noise from artifacts rather than the source.
    let dir = workspace();
    std::fs::create_dir_all(dir.path().join(".git")).expect("git dir");
    std::fs::write(dir.path().join(".git/HEAD"), "needle\n").expect("write");
    std::fs::write(dir.path().join(".gitignore"), "build/\n").expect("gitignore");
    std::fs::create_dir_all(dir.path().join("build")).expect("build dir");
    std::fs::write(dir.path().join("build/generated.rs"), "needle\n").expect("write");
    std::fs::write(dir.path().join("kept.rs"), "needle\n").expect("write");

    let registry = registry(dir.path());
    let hits = run(&registry, "grep", json!({"pattern": "needle"}))
        .await
        .expect("grep");

    assert!(hits.contains("kept.rs"), "the source is searched: {hits}");
    assert!(
        !hits.contains("build/generated.rs"),
        "ignored build output is not searched: {hits}"
    );
    assert!(
        !hits.contains(".git/"),
        "the git directory is not searched: {hits}"
    );

    let found = run(&registry, "find", json!({"pattern": "generated"}))
        .await
        .expect("find");
    assert!(
        found.is_empty(),
        "a find does not report an ignored file: {found}"
    );
}

#[tokio::test]
async fn a_running_command_reports_its_output_as_it_arrives() {
    // the live view: a build says what it is doing while it does it, rather than looking
    // like a hung command until it ends.
    use std::sync::Mutex;

    struct Record(Mutex<Vec<String>>);

    impl sa_core::agent::OutputSink for Record {
        fn output(&self, call_id: &str, chunk: &str) {
            assert_eq!(call_id, "c1", "a chunk names the call it belongs to");
            if let Ok(mut seen) = self.0.lock() {
                seen.push(chunk.to_owned());
            }
        }
    }

    let dir = workspace();
    let registry = registry(dir.path());
    let sink = Record(Mutex::new(Vec::new()));
    let outcome = registry
        .execute_streaming(
            call("bash", &json!({"command": "echo one; echo two"})),
            &sink,
        )
        .await
        .expect("run");

    assert!(outcome.output.contains("one"), "{outcome:?}");
    assert!(outcome.output.contains("two"), "{outcome:?}");
    let seen = sink.0.lock().expect("lock");
    assert!(
        !seen.is_empty(),
        "the output must be reported before the command ends"
    );
    assert!(
        seen.concat().contains("one"),
        "the live chunks carry the output: {seen:?}"
    );
}

/// the hash a rendered read gives a line, so an edit can name it.
fn hash_for(rendered: &str, needle: &str) -> String {
    rendered
        .lines()
        .find(|line| line.ends_with(needle))
        .and_then(|line| line.split_once('|'))
        .map_or_else(
            || panic!("{needle} is not in the read output"),
            |(hash, _)| hash.to_owned(),
        )
}

#[tokio::test]
async fn an_edit_can_name_a_line_by_the_hash_read_gave_it() {
    let dir = workspace();
    let reg = registry(dir.path());
    let rendered = run(&reg, "read", json!({"path": "main.rs"}))
        .await
        .expect("read");
    let hash = hash_for(&rendered, "fn main() {}");

    let out = run(
        &reg,
        "edit",
        json!({"path": "main.rs", "edits": [{"hash": hash, "content": "fn main() { run(); }"}]}),
    )
    .await
    .expect("edit by hash");
    assert!(out.contains("edited main.rs"), "{out}");

    let written = std::fs::read_to_string(dir.path().join("main.rs")).expect("read back");
    assert_eq!(written, "fn main() { run(); }\n");
}

#[tokio::test]
async fn an_edit_by_hash_refuses_a_line_that_changed_underneath_it() {
    // the whole point: a hash that no longer matches is refused rather than overwriting
    // text the model has not seen.
    let dir = workspace();
    let reg = registry(dir.path());
    let rendered = run(&reg, "read", json!({"path": "main.rs"}))
        .await
        .expect("read");
    let stale = hash_for(&rendered, "fn main() {}");

    // something else edits the file first.
    std::fs::write(dir.path().join("main.rs"), "fn main() { other(); }\n").expect("write");

    let refused = run(
        &reg,
        "edit",
        json!({"path": "main.rs", "edits": [{"hash": stale, "content": "fn main() {}"}]}),
    )
    .await
    .expect_err("a stale hash must be refused");
    assert!(refused.contains(&stale), "it names the hash: {refused}");
    assert!(
        refused.contains("re-read"),
        "and says what to do: {refused}"
    );

    // the file is untouched.
    let written = std::fs::read_to_string(dir.path().join("main.rs")).expect("read back");
    assert_eq!(written, "fn main() { other(); }\n");
}

#[tokio::test]
async fn hashed_text_copied_into_old_still_edits() {
    // a model that pastes the whole `hhhhhh|line` back must not be punished for it.
    let dir = workspace();
    let reg = registry(dir.path());
    let rendered = run(&reg, "read", json!({"path": "main.rs"}))
        .await
        .expect("read");
    let line = rendered.lines().next().expect("a line").to_owned();

    run(
        &reg,
        "edit",
        json!({"path": "main.rs", "old": line, "new": "fn main() { run(); }"}),
    )
    .await
    .expect("the hash prefix is addressing, not content");

    let written = std::fs::read_to_string(dir.path().join("main.rs")).expect("read back");
    assert_eq!(written, "fn main() { run(); }\n");
}
