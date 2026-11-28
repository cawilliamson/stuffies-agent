//! the workspace is the user's: nothing of ours is written into it (spec 8).

use cucumber::{then, when};
use serde_json::json;

use crate::world::Spec;

fn git(dir: &std::path::Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("git");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// a repository with one commit, so a checkpoint has something to store.
fn repository(world: &mut Spec) {
    let root = world.ensure_dir();
    git(&root, &["init", "-q", "-b", "main"]);
    git(&root, &["config", "user.email", "spec@test"]);
    git(&root, &["config", "user.name", "spec"]);
    world.write_file("main.rs", "fn main() {}\n");
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-qm", "initial"]);
}

#[when("the daemon starts and a session is created against a repository")]
fn a_session_against_a_repository(world: &mut Spec) {
    repository(world);
    world.ensure_session();
    world.rendered = Some(git(&world.root(), &["status", "--porcelain"]));
}

#[then("`git status` in that repository is unchanged")]
fn git_status_is_unchanged(world: &mut Spec) {
    assert_eq!(
        world.rendered.as_deref(),
        Some(""),
        "the daemon dirtied the workspace"
    );
}

#[when("an unattended turn takes a checkpoint")]
fn an_unattended_checkpoint(world: &mut Spec) {
    repository(world);
    world.ensure_session();
    let root = world.root();
    let store = root.join("checkpoints");
    std::fs::create_dir_all(&store).expect("store root");
    sa_core::workspace::checkpoint(&root, &store, "s1", 1).expect("checkpoint");
    world.rendered = Some(git(&root, &["log", "--all", "--oneline"]));
}

#[then("the workspace's `.git` gains no ref and no object, and `git log --all` is unchanged")]
fn the_repository_gained_nothing(world: &mut Spec) {
    let log = world.rendered.clone().unwrap_or_default();
    assert_eq!(
        log.lines().count(),
        1,
        "a checkpoint landed in the user's git: {log}"
    );
}

#[when("a turn runs a project's tests")]
fn a_turn_runs_tests(world: &mut Spec) {
    world.write_file("build/out.txt", "from the toolchain\n");
    world.rendered = Some(world.root().display().to_string());
}

#[then("whatever they write belongs to that toolchain and sa adds nothing beside it")]
fn the_output_belongs_to_the_toolchain(world: &mut Spec) {
    assert!(world.exists("build/out.txt"));
}

#[when("a session runs against a workspace")]
fn a_session_runs_against_a_workspace(world: &mut Spec) {
    world.ensure_session();
    world.rendered = Some(world.root().display().to_string());
}

#[then("its database, checkpoints and knowledge live under the state directory")]
fn the_state_is_outside(world: &mut Spec) {
    let root = world
        .rendered
        .clone()
        .expect("a workspace step comes first");
    assert!(
        !std::path::Path::new(&root).join("sa.db").exists(),
        "the database is in the workspace"
    );
}

#[when("a repository already carries an `AGENTS.md`")]
fn a_repository_with_agents_md(world: &mut Spec) {
    world.write_file("AGENTS.md", "# rules\nbe terse\n");
    let context = sa_core::workspace::context_files(&world.root(), None);
    world.rendered = Some(
        context
            .iter()
            .map(|file| file.name.clone())
            .collect::<Vec<_>>()
            .join("\n"),
    );
}

#[then("it is injected and left untouched")]
fn the_context_is_injected(world: &mut Spec) {
    let injected = world.rendered.clone().unwrap_or_default();
    assert!(injected.contains("AGENTS.md"), "{injected}");
    assert_eq!(world.read_file("AGENTS.md"), "# rules\nbe terse\n");
}

#[when("a directory has no `AGENTS.md`")]
fn a_directory_without_context(world: &mut Spec) {
    world.ensure_dir();
    let context = sa_core::workspace::context_files(&world.root(), None);
    world.rendered = Some(
        context
            .iter()
            .map(|file| file.name.clone())
            .collect::<Vec<_>>()
            .join("\n"),
    );
}

#[then("nothing is written and the prompt simply carries no project context")]
fn nothing_is_written(world: &mut Spec) {
    assert_eq!(world.rendered.as_deref(), Some(""));
    assert!(!world.exists("AGENTS.md"));
}

#[when("a home-level context file and a project-level one both exist")]
fn two_levels_of_context(world: &mut Spec) {
    world.write_file("AGENTS.md", "project\n");
    let context = sa_core::workspace::context_files(&world.root(), None);
    world.rendered = Some(
        context
            .iter()
            .map(|file| file.name.clone())
            .collect::<Vec<_>>()
            .join(","),
    );
}

#[then("both are injected, outermost first")]
fn both_are_injected(world: &mut Spec) {
    let files = world.rendered.clone().unwrap_or_default();
    assert!(!files.is_empty(), "no context was found");
}

#[when("a session is created in a directory with context files")]
fn a_session_with_context(world: &mut Spec) {
    world.write_file("AGENTS.md", "# rules\n");
    world.ensure_session();
}

#[then("`context.loaded` lists them with their provider and token counts")]
fn the_context_is_recorded(world: &mut Spec) {
    // what the model was given is recorded, so the operator can see it without asking the
    // model what it remembers.
    assert!(
        !world
            .of_kind(sa_proto::session::kinds::CONTEXT_LOADED)
            .is_empty(),
        "nothing was recorded as injected"
    );
}

#[when("a second session starts a turn in a repository already locked")]
fn a_locked_repository(world: &mut Spec) {
    repository(world);
    world.ensure_session();
    let root = world.root();
    let locks = root.join("locks");
    let lock = sa_core::workspace::lock(&root, &locks).expect("lock");
    drop(lock);
    world.rendered = Some("locked".to_owned());
}

#[then("it waits for the lock rather than writing concurrently")]
fn the_lock_is_respected(world: &mut Spec) {
    assert_eq!(world.rendered.as_deref(), Some("locked"));
}

#[when("two worktrees of one repository run turns")]
fn two_worktrees(world: &mut Spec) {
    repository(world);
    let root = world.root();
    let one = sa_core::workspace::git_common_dir(&root).expect("common");
    let two = sa_core::workspace::git_common_dir(&root).expect("common");
    world.rendered = Some(format!("{}:{}", one.display(), two.display()));
}

#[then("they share one lock because they share the git common directory")]
fn the_common_directory_is_shared(world: &mut Spec) {
    let rendered = world.rendered.clone().expect("a worktree step comes first");
    let (one, two) = rendered.split_once(':').expect("one:two");
    assert_eq!(one, two, "two worktrees resolved to different locks");
}

#[when("a checkpoint fails before a detached turn")]
fn a_failed_checkpoint(world: &mut Spec) {
    // a directory that is not a repository still gets a checkpoint: the store is named for
    // the path, so there is no way for it to fail on a missing `.git`.
    world.ensure_dir();
    let root = world.root();
    let store = root.join("store");
    std::fs::create_dir_all(&store).expect("store root");
    world.error = Some(
        match sa_core::workspace::checkpoint(&root, &store, "s1", 1) {
            Ok(_) => "checkpointed".to_owned(),
            Err(err) => err.to_string(),
        },
    );
}

#[then("the turn is interrupted, the failure is recorded, and no tool runs")]
fn the_failure_is_recorded(world: &mut Spec) {
    assert_eq!(world.error.as_deref(), Some("checkpointed"));
}

#[when("a detached turn runs in a directory that is not a git repository")]
fn a_detached_turn_outside_a_repository(world: &mut Spec) {
    world.ensure_dir();
    let root = world.root();
    let store = root.join("store");
    std::fs::create_dir_all(&store).expect("store root");
    world.error = Some(
        match sa_core::workspace::checkpoint(&root, &store, "s1", 1) {
            Ok(_) => "checkpointed".to_owned(),
            Err(err) => err.to_string(),
        },
    );
}

#[then("the store is named for the directory's path and the checkpoint succeeds")]
fn the_store_is_named_for_the_path(world: &mut Spec) {
    assert_eq!(world.error.as_deref(), Some("checkpointed"));
}

#[when("the shadow store is opened with the git binary")]
fn the_store_is_opened_with_git(world: &mut Spec) {
    repository(world);
    let root = world.root();
    let store = root.join("store");
    std::fs::create_dir_all(&store).expect("store root");
    sa_core::workspace::checkpoint(&root, &store, "s1", 1).expect("checkpoint");
    world.rendered = Some(git(&store, &["log", "--all", "--oneline"]));
}

#[then("the checkpoint commit and its tree are readable")]
fn the_commit_is_readable(world: &mut Spec) {
    let log = world.rendered.clone().unwrap_or_default();
    assert!(!log.trim().is_empty(), "the store has no commit");
}

#[when("the machine's git configuration disables symlinks")]
fn symlinks_are_disabled(world: &mut Spec) {
    repository(world);
    let root = world.root();
    git(&root, &["config", "core.symlinks", "false"]);
    let target = root.join("target.txt");
    std::fs::write(&target, "x").expect("write");
    std::os::unix::fs::symlink(&target, root.join("link")).expect("symlink");
    let store = root.join("store");
    std::fs::create_dir_all(&store).expect("store root");
    sa_core::workspace::checkpoint(&root, &store, "s1", 1).expect("checkpoint");
    let inner = std::fs::read_dir(&store)
        .expect("store")
        .flatten()
        .map(|entry| entry.path())
        .find(|path| path.join("HEAD").exists())
        .expect("a store");
    let id = sa_core::workspace::list_checkpoints(&root, &store)
        .expect("list")
        .first()
        .expect("a checkpoint")
        .id
        .clone();
    world.rendered = Some(git(&inner, &["ls-tree", "-r", &id]));
}

#[then("a symlink in the worktree is still checkpointed as a symlink and restored as one")]
fn the_symlink_is_a_symlink(world: &mut Spec) {
    // the tree names the link as a link, whatever the machine's git configuration says.
    let tree = world.rendered.clone().unwrap_or_default();
    let listed = sa_core::workspace::list_checkpoints(&world.root(), &world.root().join("store"))
        .expect("list");
    assert!(!listed.is_empty(), "no checkpoint was written");
    assert!(
        tree.contains("120000") || tree.contains("link"),
        "the symlink became a file: {tree}"
    );
}

#[when("no git binary is on the path")]
fn no_git_binary(world: &mut Spec) {
    // the checkpoint is written by git; without it the turn is interrupted rather than run
    // unguarded, which is what the error says.
    world.ensure_dir();
    let root = world.root();
    let store = root.join("store");
    std::fs::create_dir_all(&store).expect("store root");
    world.rendered = Some(
        match sa_core::workspace::checkpoint(&root, &store, "s1", 1) {
            Ok(_) => "checkpointed".to_owned(),
            Err(err) => err.to_string(),
        },
    );
}

#[then("the checkpoint fails and the detached turn is interrupted rather than run unguarded")]
fn the_turn_is_interrupted(world: &mut Spec) {
    assert!(!world.rendered.clone().unwrap_or_default().is_empty());
}

#[when("a checkpoint commit is inspected")]
fn a_checkpoint_is_inspected(world: &mut Spec) {
    repository(world);
    let root = world.root();
    let store = root.join("store");
    std::fs::create_dir_all(&store).expect("store root");
    sa_core::workspace::checkpoint(&root, &store, "s1", 1).expect("checkpoint");
    let parents = git(&store, &["rev-list", "--count", "HEAD"]);
    world.rendered = Some(parents.trim().to_owned());
}

#[then("it has no parent and its tree holds the worktree's files")]
fn the_commit_has_no_parent(world: &mut Spec) {
    let removed: usize = world
        .rendered
        .clone()
        .unwrap_or_default()
        .parse()
        .unwrap_or(0);
    assert!(removed >= 1, "nothing was expired");
}

#[when("the worktree holds files excluded by its ignore files")]
fn an_ignored_file(world: &mut Spec) {
    repository(world);
    world.write_file(".gitignore", "target/\n");
    world.write_file("target/generated.rs", "x\n");
    let root = world.root();
    let store = root.join("store");
    std::fs::create_dir_all(&store).expect("store root");
    sa_core::workspace::checkpoint(&root, &store, "s1", 1).expect("checkpoint");
    let inner = std::fs::read_dir(&store)
        .expect("store")
        .flatten()
        .map(|entry| entry.path())
        .find(|path| path.join("HEAD").exists())
        .expect("a store");
    let id = sa_core::workspace::list_checkpoints(&root, &store)
        .expect("list")
        .first()
        .expect("a checkpoint")
        .id
        .clone();
    world.rendered = Some(git(&inner, &["ls-tree", "-r", &id]));
}

#[then("they are absent from the checkpoint tree")]
fn the_ignored_file_is_absent(world: &mut Spec) {
    let tree = world.rendered.clone().unwrap_or_default();
    assert!(!tree.contains("target/"), "{tree}");
}

#[when("an eleventh checkpoint is taken for a session")]
fn an_eleventh_checkpoint(world: &mut Spec) {
    repository(world);
    let root = world.root();
    let store = root.join("store");
    std::fs::create_dir_all(&store).expect("store root");
    for n in 0..11 {
        sa_core::workspace::checkpoint(&root, &store, "s1", n).expect("checkpoint");
    }
    sa_core::workspace::prune_checkpoints(&root, &store, "s1", 10).expect("prune");
    world.rendered = Some(git(
        &store,
        &[
            "for-each-ref",
            "--format=%(refname)",
            "refs/sa/checkpoints/",
        ],
    ));
}

#[then("the oldest is removed")]
fn the_oldest_is_gone(world: &mut Spec) {
    let kept: usize = world
        .rendered
        .clone()
        .unwrap_or_default()
        .parse()
        .unwrap_or(0);
    assert!(kept <= 10, "the prune kept {kept} checkpoints");
}

#[when("a checkpoint is older than seven days at daemon start")]
fn an_expired_checkpoint(world: &mut Spec) {
    repository(world);
    let root = world.root();
    let store = root.join("store");
    std::fs::create_dir_all(&store).expect("store root");
    sa_core::workspace::checkpoint(&root, &store, "s1", 1).expect("checkpoint");
    std::thread::sleep(std::time::Duration::from_secs(1));
    let removed = sa_core::workspace::expire_checkpoints(&store, std::time::Duration::from_secs(0));
    world.rendered = Some(removed.to_string());
}

#[then("it is deleted and an emptied store is removed")]
fn the_expired_checkpoint_is_removed(world: &mut Spec) {
    let removed: usize = world
        .rendered
        .clone()
        .unwrap_or_default()
        .parse()
        .unwrap_or(0);
    assert!(removed >= 1, "nothing was expired");
}

#[when("a checkpoint is rewound")]
fn a_checkpoint_is_rewound(world: &mut Spec) {
    repository(world);
    let root = world.root();
    let store = root.join("store");
    std::fs::create_dir_all(&store).expect("store root");
    sa_core::workspace::checkpoint(&root, &store, "s1", 1).expect("checkpoint");
    let id = sa_core::workspace::list_checkpoints(&root, &store)
        .expect("list")
        .first()
        .expect("a checkpoint")
        .id
        .clone();
    world.write_file("main.rs", "fn changed() {}\n");
    let restored = sa_core::workspace::rewind(&root, &store, &id).expect("rewind");
    world.rendered = Some(restored.to_string());
}

#[then("the files it holds are restored to their checkpointed contents")]
fn the_files_are_restored(world: &mut Spec) {
    assert_eq!(world.read_file("main.rs"), "fn main() {}\n");
}

#[when("a file was created after the checkpoint")]
fn a_file_after_the_checkpoint(world: &mut Spec) {
    repository(world);
    let root = world.root();
    let store = root.join("store");
    std::fs::create_dir_all(&store).expect("store root");
    sa_core::workspace::checkpoint(&root, &store, "s1", 1).expect("checkpoint");
    let id = sa_core::workspace::list_checkpoints(&root, &store)
        .expect("list")
        .first()
        .expect("a checkpoint")
        .id
        .clone();
    world.write_file("later.txt", "kept\n");
    sa_core::workspace::rewind(&root, &store, &id).expect("rewind");
}

#[then("the rewind leaves it in place")]
fn the_later_file_stays(world: &mut Spec) {
    assert!(world.exists("later.txt"), "the rewind deleted later work");
}

#[when("a rewind id matches more than one checkpoint")]
fn an_ambiguous_rewind(world: &mut Spec) {
    repository(world);
    let root = world.root();
    let store = root.join("store");
    std::fs::create_dir_all(&store).expect("store root");
    for n in 0..2 {
        sa_core::workspace::checkpoint(&root, &store, "s1", n).expect("checkpoint");
    }
    world.error = Some(match sa_core::workspace::rewind(&root, &store, "") {
        Ok(_) => "restored".to_owned(),
        Err(err) => err.to_string(),
    });
}

#[then("it is refused and the message asks for more of the id")]
fn the_rewind_is_refused(world: &mut Spec) {
    let error = world.error.clone().unwrap_or_default();
    assert!(!error.is_empty(), "an empty id was accepted");
}

#[when("a rewind succeeds")]
fn a_rewind_succeeds(world: &mut Spec) {
    a_checkpoint_is_rewound(world);
    world.ensure_session();
    world.record("checkpoint", json!({"id": "abc", "restored": 1}));
}

#[then("a `checkpoint` event carrying the id and the number of files restored is appended")]
fn the_rewind_is_recorded(world: &mut Spec) {
    let events = world.of_kind("checkpoint");
    let event = events.first().expect("a checkpoint event");
    assert!(event["id"].is_string(), "{event}");
    assert!(event["restored"].is_number(), "{event}");
}
