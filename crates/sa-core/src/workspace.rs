//! workspace locking and the unattended checkpoint (spec 7.4, 8.4).

use std::fs::File;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

#[derive(Debug, thiserror::Error)]
pub enum WorkspaceError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("git: {0}")]
    Git(String),
    #[error("not a git repository: {0}")]
    NotARepository(String),
}

/// an exclusive lock on a repository's git common directory.
///
/// ponytail: blocking flock on the calling thread. a contended lock means another session in
/// the same repository is mid-turn, which is rare and short; move to an async lock if that
/// ever queues.
pub struct WorkspaceLock {
    _lock: nix::fcntl::Flock<File>,
}

/// the git common directory, which is shared by every worktree of one repository.
pub fn git_common_dir(workspace: &Path) -> Result<PathBuf, WorkspaceError> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(workspace)
        .args(["rev-parse", "--git-common-dir"])
        .output()?;
    if !output.status.success() {
        return Err(WorkspaceError::NotARepository(
            workspace.display().to_string(),
        ));
    }
    let raw = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if raw.is_empty() {
        return Err(WorkspaceError::NotARepository(
            workspace.display().to_string(),
        ));
    }
    let path = Path::new(&raw);
    Ok(if path.is_absolute() {
        path.to_path_buf()
    } else {
        workspace.join(path)
    })
}

/// takes the workspace lock, creating the lock directory if needed.
pub fn lock(workspace: &Path, lock_dir: &Path) -> Result<WorkspaceLock, WorkspaceError> {
    use nix::fcntl::{Flock, FlockArg};
    use sha2::{Digest, Sha256};

    let common = git_common_dir(workspace)?;
    std::fs::create_dir_all(lock_dir)?;

    let mut hasher = Sha256::new();
    hasher.update(common.display().to_string().as_bytes());
    let mut name = String::new();
    for byte in hasher.finalize() {
        use std::fmt::Write as _;
        let _ = write!(name, "{byte:02x}");
    }

    let file = File::create(lock_dir.join(format!("{name}.lock")))?;
    let lock = Flock::lock(file, FlockArg::LockExclusive)
        .map_err(|(_, err)| WorkspaceError::Io(std::io::Error::from_raw_os_error(err as i32)))?;
    Ok(WorkspaceLock { _lock: lock })
}

/// a checkpoint commit built against a temporary index, so neither the worktree nor the real
/// index is touched (spec 8.4).
pub struct Checkpoint {
    pub reference: String,
    pub base_commit: String,
    pub tree: String,
}

fn git(workspace: &Path, args: &[&str]) -> Result<String, WorkspaceError> {
    git_env(workspace, args, None)
}

fn git_env(
    workspace: &Path,
    args: &[&str],
    env: Option<(&str, &Path)>,
) -> Result<String, WorkspaceError> {
    let mut command = std::process::Command::new("git");
    command.arg("-C").arg(workspace).args(args);
    if let Some((key, value)) = env {
        command.env(key, value);
    }
    let output = command.output()?;
    if !output.status.success() {
        return Err(WorkspaceError::Git(format!(
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// creates a checkpoint for an unattended turn.
pub fn checkpoint(
    workspace: &Path,
    session_id: &str,
    seq: u64,
) -> Result<Checkpoint, WorkspaceError> {
    // a directory that is not a repository, or has no commits yet, cannot be checkpointed.
    let base_commit = git(workspace, &["rev-parse", "HEAD"])
        .map_err(|_| WorkspaceError::NotARepository(workspace.display().to_string()))?;

    // unique per call: two sessions can share a session id prefix across repositories,
    // and tests run in parallel.
    let index = std::env::temp_dir().join(format!(
        "sa-index-{session_id}-{seq}-{}",
        uuid::Uuid::new_v4()
    ));
    let _ = std::fs::remove_file(&index);

    git_env(workspace, &["add", "-A"], Some(("GIT_INDEX_FILE", &index)))?;
    let tree = git_env(workspace, &["write-tree"], Some(("GIT_INDEX_FILE", &index)))?;
    let _ = std::fs::remove_file(&index);

    let message = format!("sa checkpoint {session_id} {seq}");
    let commit = git(
        workspace,
        &["commit-tree", &tree, "-p", &base_commit, "-m", &message],
    )?;

    let reference = format!("refs/sa/checkpoints/{session_id}/{seq}");
    git(workspace, &["update-ref", &reference, &commit])?;

    Ok(Checkpoint {
        reference,
        base_commit,
        tree,
    })
}

/// keeps only the `keep` most recent checkpoint refs for a session (spec 8.4).
pub fn prune_checkpoints(
    workspace: &Path,
    session_id: &str,
    keep: usize,
) -> Result<(), WorkspaceError> {
    let prefix = format!("refs/sa/checkpoints/{session_id}/");
    let raw = git(workspace, &["for-each-ref", "--format=%(refname)", &prefix])?;
    let mut refs: Vec<&str> = raw.lines().filter(|l| !l.is_empty()).collect();
    refs.sort_unstable();
    if refs.len() <= keep {
        return Ok(());
    }
    for stale in &refs[..refs.len() - keep] {
        git(workspace, &["update-ref", "-d", stale])?;
    }
    Ok(())
}

#[must_use]
pub fn checkpoint_payload(checkpoint: &Checkpoint) -> Value {
    json!({
        "ref": checkpoint.reference,
        "base_commit": checkpoint.base_commit,
        "tree": checkpoint.tree,
        "includes_untracked": true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        let run = |args: &[&str]| {
            std::process::Command::new("git")
                .arg("-C")
                .arg(dir.path())
                .args(args)
                .output()
                .expect("git");
        };
        run(&["init", "-q"]);
        run(&["config", "user.email", "test@example.com"]);
        run(&["config", "user.name", "test"]);
        std::fs::write(dir.path().join("a.txt"), "one").expect("write");
        run(&["add", "-A"]);
        run(&["commit", "-qm", "init"]);
        dir
    }

    #[test]
    fn checkpoint_captures_untracked_files_without_touching_the_worktree() {
        let dir = repo();
        std::fs::write(dir.path().join("new.txt"), "untracked").expect("write");

        let checkpoint = checkpoint(dir.path(), "s1", 1).expect("checkpoint");

        assert!(
            dir.path().join("new.txt").exists(),
            "the worktree must be untouched"
        );
        assert!(!checkpoint.tree.is_empty());

        // the index must still be clean: `git add -A` went to the temporary index only.
        let staged = git(dir.path(), &["diff", "--cached", "--name-only"]).expect("diff");
        assert!(staged.is_empty(), "real index was modified: {staged}");
    }

    #[test]
    fn checkpoint_ref_survives_and_prunes() {
        let dir = repo();
        for seq in 1..=3 {
            checkpoint(dir.path(), "s1", seq).expect("checkpoint");
        }
        prune_checkpoints(dir.path(), "s1", 2).expect("prune");
        let refs = git(
            dir.path(),
            &[
                "for-each-ref",
                "--format=%(refname)",
                "refs/sa/checkpoints/s1/",
            ],
        )
        .expect("refs");
        assert_eq!(refs.lines().count(), 2, "{refs}");
    }

    #[test]
    fn lock_is_exclusive_per_repository() {
        let dir = repo();
        let locks = tempfile::tempdir().expect("locks");
        let first = lock(dir.path(), locks.path()).expect("first lock");
        drop(first);
        let second = lock(dir.path(), locks.path()).expect("relock after release");
        drop(second);
    }

    #[test]
    fn a_non_repository_is_refused() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(matches!(
            git_common_dir(dir.path()),
            Err(WorkspaceError::NotARepository(_))
        ));
    }
}
