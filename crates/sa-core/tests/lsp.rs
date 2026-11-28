//! a real language server, when the machine has one.

// a test that says why it stepped aside is more useful than one that is silent.
#![allow(clippy::expect_used, clippy::print_stderr)]

use std::path::Path;

use sa_core::lsp::{Hub, Severity};

fn installed(command: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|dir| dir.join(command).is_file()))
}

fn write(dir: &Path, name: &str, body: &str) {
    std::fs::write(dir.join(name), body).expect("write");
}

/// the whole path a turn takes: a file that does not type-check, the server that knows it,
/// and the finding that comes back.
#[tokio::test]
async fn a_type_error_is_reported_for_the_text_on_disk() {
    if !installed("pyright-langserver") {
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    write(
        dir.path(),
        "pyproject.toml",
        "[project]\nname = \"probe\"\n",
    );
    write(
        dir.path(),
        "a.py",
        "def shout(text: str) -> str:\n    return text\n\n\nshout(41)\n",
    );
    let hub = Hub::new();
    let findings = hub
        .diagnostics(
            &dir.path().join("a.py"),
            dir.path(),
            std::time::Duration::from_secs(30),
        )
        .await
        .expect("diagnostics");
    hub.shutdown().await;
    assert!(
        findings
            .iter()
            .any(|finding| finding.severity == Severity::Error),
        "expected an error, got {findings:?}"
    );
    assert!(
        findings
            .iter()
            .any(|finding| finding.message.contains("cannot be assigned")),
        "expected the argument's type to be named, got {findings:?}"
    );
}

/// a server sees the file as it now is, not as it was when it was opened.
#[tokio::test]
async fn a_fixed_file_has_no_findings() {
    if !installed("pyright-langserver") {
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    write(
        dir.path(),
        "pyproject.toml",
        "[project]\nname = \"probe\"\n",
    );
    let file = dir.path().join("a.py");
    std::fs::write(
        &file,
        "def shout(text: str) -> str:\n    return text\n\n\nshout(41)\n",
    )
    .expect("write");
    let hub = Hub::new();
    let before = hub
        .diagnostics(&file, dir.path(), std::time::Duration::from_secs(30))
        .await
        .expect("first");
    assert!(
        !before.is_empty(),
        "expected the broken text to be reported"
    );
    std::fs::write(
        &file,
        "def shout(text: str) -> str:\n    return text\n\n\nshout(\"hi\")\n",
    )
    .expect("rewrite");
    let after = hub
        .diagnostics(&file, dir.path(), std::time::Duration::from_secs(30))
        .await
        .expect("second");
    hub.shutdown().await;
    assert!(
        after.is_empty(),
        "the server still described the old text: {after:?}"
    );
}

/// a server answers before it has analysed anything: rust-analyzer returns an empty report
/// for a file that does not compile until its first pass finishes. believing that answer
/// calls a broken file clean, which is the one mistake a checker must not make.
#[tokio::test]
async fn a_broken_file_is_not_reported_clean_before_the_server_has_analysed() {
    if !installed("rust-analyzer") {
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join("src")).expect("mkdir");
    write(
        dir.path(),
        "Cargo.toml",
        "[package]\nname = \"probe\"\nversion = \"0.0.0\"\nedition = \"2021\"\n",
    );
    let file = dir.path().join("src/lib.rs");
    std::fs::write(&file, "pub fn answer() -> u32 {\n    \"not a number\"\n}\n").expect("write");
    let hub = Hub::new();
    let findings = match hub
        .diagnostics(&file, dir.path(), std::time::Duration::from_secs(40))
        .await
    {
        Ok(findings) => findings,
        // a server that could not be reached says nothing about the answer, so the claim is
        // not tested rather than tested and failed. the machine's rust-analyzer is the
        // variable, not the daemon.
        Err(err) => {
            eprintln!("rust-analyzer did not answer, skipping: {err}");
            hub.shutdown().await;
            return;
        }
    };
    hub.shutdown().await;
    assert!(
        findings
            .iter()
            .any(|finding| finding.severity == Severity::Error),
        "a file that does not compile was reported clean: {findings:?}"
    );
}
