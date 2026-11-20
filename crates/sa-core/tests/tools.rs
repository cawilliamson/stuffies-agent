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
    let out = run(&reg, "read", json!({"path": "main.rs"}))
        .await
        .expect("read");
    assert_eq!(out, "fn main() {}\n");
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
