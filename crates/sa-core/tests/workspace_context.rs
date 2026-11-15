//! pi-contexts parity: read from several roots, never write (spec 6.1).
#![allow(clippy::expect_used, clippy::panic)]

use std::path::PathBuf;

use sa_core::workspace::{context_files_in, skills_in, CONTEXT_FILES};

fn root(files: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    for (path, body) in files {
        let full = dir.path().join(path);
        std::fs::create_dir_all(full.parent().expect("parent")).expect("dirs");
        std::fs::write(full, body).expect("write");
    }
    dir
}

fn names(files: &[sa_core::workspace::ContextFile]) -> Vec<String> {
    files.iter().map(|f| f.name.clone()).collect()
}

#[test]
fn all_four_pi_context_files_are_recognised() {
    assert_eq!(
        CONTEXT_FILES,
        ["SOUL.md", "AGENTS.md", "RULES.md", "APPEND_SYSTEM.md"]
    );

    let dir = root(&[
        ("SOUL.md", "persona"),
        ("AGENTS.md", "project"),
        ("RULES.md", "rules"),
        ("APPEND_SYSTEM.md", "extra"),
    ]);
    let found = context_files_in(&[dir.path().to_path_buf()]);
    assert_eq!(
        names(&found),
        vec!["SOUL.md", "AGENTS.md", "RULES.md", "APPEND_SYSTEM.md"],
        "injection order must match pi-contexts"
    );
}

#[test]
fn the_deployed_root_takes_precedence_over_the_workspace() {
    let deployed = root(&[("AGENTS.md", "deployed copy")]);
    let workspace = root(&[("AGENTS.md", "workspace copy")]);

    let found = context_files_in(&[
        deployed.path().to_path_buf(),
        workspace.path().to_path_buf(),
    ]);
    assert_eq!(found.len(), 1, "only one AGENTS.md may be injected");
    assert_eq!(found[0].body, "deployed copy");
}

#[test]
fn a_file_missing_from_the_first_root_falls_through() {
    let deployed = root(&[("SOUL.md", "persona")]);
    let workspace = root(&[("AGENTS.md", "project")]);

    let found = context_files_in(&[
        deployed.path().to_path_buf(),
        workspace.path().to_path_buf(),
    ]);
    assert_eq!(names(&found), vec!["SOUL.md", "AGENTS.md"]);
}

#[test]
fn skills_are_discovered_with_a_summary() {
    let dir = root(&[(
        "skills/rust-fix/SKILL.md",
        "---\nname: rust-fix\ndescription: Fix common rust errors\n---\n\nbody\n",
    )]);
    let found = skills_in(&[dir.path().to_path_buf()]);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].name, "rust-fix");
    assert_eq!(found[0].summary, "Fix common rust errors");
}

#[test]
fn reading_context_writes_nothing_into_the_workspace() {
    let workspace = root(&[
        ("AGENTS.md", "project"),
        ("skills/fix/SKILL.md", "description: fixes things\n"),
    ]);
    let before: Vec<PathBuf> = std::fs::read_dir(workspace.path())
        .expect("read")
        .filter_map(Result::ok)
        .map(|e| e.path())
        .collect();

    let _ = context_files_in(&[workspace.path().to_path_buf()]);
    let _ = skills_in(&[workspace.path().to_path_buf()]);

    let after: Vec<PathBuf> = std::fs::read_dir(workspace.path())
        .expect("read")
        .filter_map(Result::ok)
        .map(|e| e.path())
        .collect();
    let mut before = before;
    let mut after = after;
    before.sort();
    after.sort();
    assert_eq!(before, after, "context loading must not create anything");
}
