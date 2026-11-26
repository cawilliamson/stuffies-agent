//! the openspec tree a workspace carries, fronted rather than reimplemented (spec 9).
//!
//! the tree's format is defined by the `openspec` program, not by a written standard, so
//! every answer here is that program's answer: what it prints is what we report, and a change
//! upstream needs no backport here. nothing runs unless the program is installed and the
//! workspace carries the tree, and nothing of ours is written into the workspace.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

/// the directory a workspace carries its spec tree in.
pub const DIR: &str = "openspec";

/// the program that owns the tree.
pub const PROGRAM: &str = "openspec";

/// how long one call may take. the program is a node cli and a cold run is a few hundred
/// milliseconds, so this is a hang, not a slow machine.
pub const TIMEOUT: Duration = Duration::from_secs(60);

/// the most output one call returns, so a listing cannot flood the transcript.
pub const MAX_OUTPUT: usize = 64 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum OpenSpecError {
    #[error("no {DIR}/ directory in {0}")]
    Absent(String),
    #[error("{PROGRAM} is not installed: nothing named {PROGRAM} on PATH")]
    Missing,
    #[error("{PROGRAM} {args} timed out after {}s", TIMEOUT.as_secs())]
    Timeout { args: String },
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

/// what one call to the program produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Output {
    pub exit_code: i32,
    pub text: String,
}

impl Output {
    #[must_use]
    pub const fn ok(&self) -> bool {
        self.exit_code == 0
    }
}

/// the tree a workspace carries, if it carries one.
#[must_use]
pub fn tree(workspace: &Path) -> Option<PathBuf> {
    let root = workspace.join(DIR);
    root.is_dir().then_some(root)
}

/// where the program is, when the workspace carries a tree for it to read. both halves are
/// required: a program with nothing to read and a tree with no program are both silent.
#[must_use]
pub fn available(workspace: &Path) -> Option<PathBuf> {
    tree(workspace)?;
    crate::lsp::resolve(PROGRAM)
}

/// runs the program in the workspace with the arguments as written, and returns what it said.
pub async fn run(workspace: &Path, args: &[String]) -> Result<Output, OpenSpecError> {
    tree(workspace).ok_or_else(|| OpenSpecError::Absent(workspace.display().to_string()))?;
    let program = crate::lsp::resolve(PROGRAM).ok_or(OpenSpecError::Missing)?;
    run_program(&program, workspace, args).await
}

/// runs one call, bounded in time and output. the child is killed if the call is abandoned,
/// so a hang cannot outlive the request that started it.
pub async fn run_program(
    program: &Path,
    cwd: &Path,
    args: &[String],
) -> Result<Output, OpenSpecError> {
    let child = tokio::process::Command::new(program)
        .args(args)
        .current_dir(cwd)
        // no terminal is promised, so nothing may wait for one.
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    let finished = tokio::time::timeout(TIMEOUT, child.wait_with_output()).await;
    let output = match finished {
        Ok(result) => result?,
        Err(_) => {
            return Err(OpenSpecError::Timeout {
                args: args.join(" "),
            })
        }
    };
    Ok(Output {
        exit_code: output.status.code().unwrap_or(-1),
        text: cap(&joined(&output.stdout, &output.stderr)),
    })
}

/// what the program said, as one answer: its findings are often on the error stream, and a
/// reader wants the whole report rather than half of it.
fn joined(stdout: &[u8], stderr: &[u8]) -> String {
    let out = String::from_utf8_lossy(stdout);
    let err = String::from_utf8_lossy(stderr);
    match (out.trim().is_empty(), err.trim().is_empty()) {
        (true, true) => String::new(),
        (false, true) => out.trim_end().to_owned(),
        (true, false) => err.trim_end().to_owned(),
        (false, false) => format!("{}\n{}", out.trim_end(), err.trim_end()),
    }
}

/// output above the cap is cut, and the cut is marked rather than silent.
fn cap(text: &str) -> String {
    if text.len() <= MAX_OUTPUT {
        return text.to_owned();
    }
    let mut end = MAX_OUTPUT;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n… truncated at {MAX_OUTPUT} bytes", &text[..end])
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    fn workspace(with_tree: bool) -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        if with_tree {
            std::fs::create_dir_all(dir.path().join(DIR)).expect("tree");
        }
        dir
    }

    /// a program that reports what it was given, so the call itself can be checked without
    /// the real cli being installed.
    fn script(dir: &Path, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt as _;

        let path = dir.join("fake-openspec");
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("write");
        let mut permissions = std::fs::metadata(&path).expect("metadata").permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&path, permissions).expect("chmod");
        path
    }

    #[test]
    fn a_tree_is_found_by_the_directory_it_sits_in() {
        let dir = workspace(true);
        assert!(tree(dir.path()).is_some());
        assert!(tree(&dir.path().join("elsewhere")).is_none());
    }

    #[tokio::test]
    async fn a_call_passes_its_arguments_and_reports_the_exit_code() {
        let dir = workspace(true);
        let program = script(dir.path(), "echo \"$@\"; echo oops >&2; exit 3");
        let args = ["list".to_owned(), "--specs".to_owned()];
        let output = run_program(&program, dir.path(), &args).await.expect("run");
        assert_eq!(output.exit_code, 3);
        assert!(output.text.contains("list --specs"), "{}", output.text);
        assert!(output.text.contains("oops"), "{}", output.text);
        assert!(!output.ok());
    }

    #[tokio::test]
    async fn a_call_runs_where_the_workspace_is() {
        let dir = workspace(true);
        let program = script(dir.path(), "pwd");
        let output = run_program(&program, dir.path(), &[]).await.expect("run");
        assert_eq!(output.text, dir.path().display().to_string());
    }

    #[tokio::test]
    async fn a_call_that_hangs_is_given_up_on() {
        let dir = workspace(true);
        let program = script(dir.path(), "sleep 600");
        // abandoned before its own budget runs out: the child is killed on drop rather than
        // waited out, which is what keeps a hung cli from outliving the request.
        let args: Vec<String> = Vec::new();
        let abandoned = tokio::time::timeout(
            Duration::from_millis(200),
            run_program(&program, dir.path(), &args),
        )
        .await;
        assert!(abandoned.is_err(), "the child outlived the call");
    }

    #[tokio::test]
    async fn no_tree_is_refused_before_anything_runs() {
        let dir = workspace(false);
        let err = run(dir.path(), &[]).await.expect_err("no tree");
        assert!(matches!(err, OpenSpecError::Absent(_)), "{err}");
    }

    #[test]
    fn output_above_the_cap_is_cut_and_marked() {
        let long = "x".repeat(MAX_OUTPUT + 10);
        let capped = cap(&long);
        assert!(capped.contains("truncated"), "{capped}");
        assert!(capped.len() < long.len() + 40);
        assert_eq!(cap("short"), "short");
    }
}
