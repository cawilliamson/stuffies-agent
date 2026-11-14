//! tool execution against a real filesystem (spec 9).

use std::path::PathBuf;

use sa_core::agent::ToolExecutor;
use sa_core::llm::ToolCall;
use sa_core::tools::ToolRegistry;
use serde_json::json;

fn registry(workspace: &std::path::Path) -> ToolRegistry {
    ToolRegistry::new(workspace.to_path_buf())
}

fn call(name: &str, args: serde_json::Value) -> ToolCall {
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
    registry.execute(call(name, args)).await
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
    assert!(out.contains("5"), "{out}");
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
async fn ls_lists_directory_entries() {
    let dir = workspace();
    let reg = registry(dir.path());
    let out = run(&reg, "ls", json!({"path": "."})).await.expect("ls");
    assert!(out.contains("main.rs"), "{out}");
    assert!(out.contains("src/"), "{out}");
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
    assert!(out.contains("exit 0"), "{out}");
    assert!(out.contains("hi"), "{out}");
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
