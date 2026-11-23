//! a real language server, when the machine has one.

#![allow(clippy::expect_used)]

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
        .diagnostics(&dir.path().join("a.py"), dir.path())
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
    let before = hub.diagnostics(&file, dir.path()).await.expect("first");
    assert!(
        !before.is_empty(),
        "expected the broken text to be reported"
    );
    std::fs::write(
        &file,
        "def shout(text: str) -> str:\n    return text\n\n\nshout(\"hi\")\n",
    )
    .expect("rewrite");
    let after = hub.diagnostics(&file, dir.path()).await.expect("second");
    hub.shutdown().await;
    assert!(
        after.is_empty(),
        "the server still described the old text: {after:?}"
    );
}
