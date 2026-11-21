//! workspace locking and the unattended checkpoint (spec 7.4, 8.4).

use std::fmt::Write as _;
use std::fs::File;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

/// the config root. everything sa reads about a workspace lives here, never in the
/// workspace itself (see spec 6.1).
#[must_use]
pub fn config_dir() -> PathBuf {
    directories::BaseDirs::new()
        .map_or_else(
            || PathBuf::from(".config"),
            |dirs| dirs.config_dir().to_path_buf(),
        )
        .join("stuffies-agent")
}

fn home() -> PathBuf {
    directories::BaseDirs::new()
        .map_or_else(|| PathBuf::from("."), |dirs| dirs.home_dir().to_path_buf())
}

/// a stable directory name for a workspace path.
///
/// matches the scheme pi-contexts uses for `~/.pi/agent/projects/`, so one dotfiles module
/// can deploy context for both agents from the same source tree.
#[must_use]
pub fn slug(path: &Path) -> String {
    let raw = path.display().to_string();
    raw.trim_start_matches('/').replace('/', "-")
}

/// per-workspace config directory, keyed by the workspace path.
#[must_use]
pub fn workspace_config_dir(workspace: &Path) -> PathBuf {
    config_dir().join("workspaces").join(slug(workspace))
}

/// the context files pi-contexts deploys, in injection order.
pub const CONTEXT_FILES: [&str; 4] = [
    "PERSONALITY.md",
    "AGENTS.md",
    "RULES.md",
    "APPEND_SYSTEM.md",
];

/// one context file discovered on disk.
#[derive(Debug, Clone)]
pub struct ContextFile {
    pub name: String,
    pub path: PathBuf,
    pub body: String,
}

/// the directories context applies to, outermost first.
///
/// every ancestor from home down to the workspace itself: a file in a parent directory
/// applies to everything beneath it, the way pi-contexts layers a home context over a
/// project one.
#[must_use]
pub fn context_scopes(workspace: &Path) -> Vec<PathBuf> {
    context_scopes_in(workspace, &home())
}

/// the same, against an explicit home. keeps tests free of process-global environment.
#[must_use]
pub fn context_scopes_in(workspace: &Path, home: &Path) -> Vec<PathBuf> {
    let workspace = std::fs::canonicalize(workspace).unwrap_or_else(|_| workspace.to_path_buf());
    let home = std::fs::canonicalize(home).unwrap_or_else(|_| home.to_path_buf());
    let mut chain: Vec<PathBuf> = workspace.ancestors().map(Path::to_path_buf).collect();
    chain.reverse();
    // a file above home belongs to the machine, not to this operator, so the chain starts
    // at home when the workspace is inside it.
    chain
        .iter()
        .position(|step| *step == home)
        .map_or_else(|| vec![workspace], |at| chain.split_off(at))
}

/// the roots one scope is read from, highest precedence first: the deployed config for that
/// directory, then the directory itself, where a repository may already carry its own.
#[must_use]
pub fn scope_roots(scope: &Path) -> Vec<PathBuf> {
    vec![workspace_config_dir(scope), scope.to_path_buf()]
}

/// the roots context is read from, highest precedence first, for every scope in order.
///
/// nothing here is ever written to: the deployed config dir belongs to the operator, and
/// the workspace and daemon directories are read-only as far as `sa` is concerned.
#[must_use]
pub fn context_roots(workspace: &Path, cwd: Option<&Path>) -> Vec<PathBuf> {
    scopes_with(workspace, cwd)
        .iter()
        .flat_map(|scope| scope_roots(scope))
        .collect()
}

/// the scopes that apply, with the client's own directory appended when it is elsewhere.
#[must_use]
fn scopes_with(workspace: &Path, cwd: Option<&Path>) -> Vec<PathBuf> {
    scopes_with_in(workspace, cwd, &home())
}

fn scopes_with_in(workspace: &Path, cwd: Option<&Path>, home: &Path) -> Vec<PathBuf> {
    let mut scopes = context_scopes_in(workspace, home);
    if let Some(cwd) = cwd {
        if cwd != workspace {
            scopes.extend(context_scopes_in(cwd, home));
        }
    }
    scopes.dedup();
    scopes
}

/// every context file that applies to a workspace, outermost scope first, so a parent's
/// context is followed by the child's rather than replaced by it.
#[must_use]
pub fn context_files(workspace: &Path, cwd: Option<&Path>) -> Vec<ContextFile> {
    scopes_with(workspace, cwd)
        .iter()
        .flat_map(|scope| context_files_in(&scope_roots(scope)))
        .collect()
}

/// the same, against explicit roots. keeps tests free of process-global environment.
#[must_use]
pub fn context_files_in(roots: &[PathBuf]) -> Vec<ContextFile> {
    // within one scope this is precedence: the first root that provides a file wins, so a
    // deployed copy is used instead of the repository's rather than both.
    let mut out = Vec::new();
    for name in CONTEXT_FILES {
        for root in roots {
            let path = root.join(name);
            // the logical path is what the operator is shown: a deployed file is a symlink
            // into the store, and the store path says nothing about where it came from.
            let Ok(canonical) = std::fs::canonicalize(&path) else {
                continue;
            };
            let Ok(body) = std::fs::read_to_string(&canonical) else {
                continue;
            };
            if body.trim().is_empty() {
                continue;
            }
            out.push(ContextFile {
                name: name.to_owned(),
                path,
                body,
            });
            break;
        }
    }
    out
}

/// one file in a context listing, with what it adds to the prompt.
#[derive(Debug, Clone)]
pub struct ContextListing {
    pub path: String,
    /// the path below the root that provided it, which is what the tree is drawn from.
    pub relative: String,
    /// the root that provided it, so a file deployed elsewhere is not shown inside a
    /// directory it does not live in.
    pub provider: String,
    pub tokens: u64,
}

/// a root that provides context, and the files under it, sorted.
#[derive(Debug, Clone)]
pub struct ContextGroup {
    pub root: String,
    pub files: Vec<ContextListing>,
}

/// a skill directory discovered on disk.
#[derive(Debug, Clone)]
pub struct SkillRef {
    pub name: String,
    pub path: PathBuf,
    pub summary: String,
}

/// the context a directory provides, grouped by the directory it applies to.
///
/// this is the shape the operator is shown at the start of a session. every scope
/// contributes, so a parent's context is listed alongside the workspace's.
#[must_use]
pub fn context_listing(workspace: &Path, cwd: Option<&Path>) -> Vec<ContextGroup> {
    context_listing_in(workspace, cwd, &home())
}

/// the same, against an explicit home. keeps tests free of process-global environment.
#[must_use]
pub fn context_listing_in(workspace: &Path, cwd: Option<&Path>, home: &Path) -> Vec<ContextGroup> {
    let mut groups = Vec::new();
    for scope in scopes_with_in(workspace, cwd, home) {
        let roots = scope_roots(&scope);
        let mut files: Vec<ContextListing> = Vec::new();
        for file in context_files_in(&roots) {
            files.push(ContextListing {
                relative: relative_to(&roots, &file.path),
                provider: provider_of(&roots, &file.path),
                path: file.path.display().to_string(),
                tokens: crate::tokens::estimate_rounded(&file.body),
            });
        }
        for skill in skills_in(&roots) {
            let body = std::fs::read_to_string(&skill.path).unwrap_or_default();
            files.push(ContextListing {
                relative: relative_to(&roots, &skill.path),
                provider: provider_of(&roots, &skill.path),
                path: skill.path.display().to_string(),
                tokens: crate::tokens::estimate_rounded(&body),
            });
        }
        if files.is_empty() {
            continue;
        }
        files.sort_by(|left, right| left.path.cmp(&right.path));
        files.dedup_by(|left, right| left.path == right.path);
        groups.push(ContextGroup {
            root: scope.display().to_string(),
            files,
        });
    }
    groups.sort_by(|left, right| left.root.cmp(&right.root));
    groups
}

/// the path of a file below the root that provided it. a file outside every root keeps its
/// own name, so it is never given a directory it does not live in.
#[must_use]
fn relative_to(roots: &[PathBuf], path: &Path) -> String {
    roots
        .iter()
        .find_map(|root| path.strip_prefix(root).ok())
        .map_or_else(
            || {
                path.file_name()
                    .map_or_else(String::new, |name| name.to_string_lossy().into_owned())
            },
            |rest| rest.display().to_string(),
        )
}

/// the root a file was found under, as a path.
#[must_use]
fn provider_of(roots: &[PathBuf], path: &Path) -> String {
    roots
        .iter()
        .find(|root| path.starts_with(root))
        .map_or_else(String::new, |root| root.display().to_string())
}

/// a one-line summary for a skill file, from its frontmatter or first heading.
fn skill_summary(body: &str) -> String {
    for line in body.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("description:") {
            return rest.trim().trim_matches('"').to_owned();
        }
    }
    body.lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with("---") && !line.starts_with('#'))
        .unwrap_or_default()
        .chars()
        .take(120)
        .collect()
}

/// skill directories that apply to a workspace, in the same precedence order.
#[must_use]
pub fn skills(workspace: &Path, cwd: Option<&Path>) -> Vec<SkillRef> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for scope in scopes_with(workspace, cwd) {
        for skill in skills_in(&scope_roots(&scope)) {
            if seen.insert(skill.name.clone()) {
                out.push(skill);
            }
        }
    }
    out
}

/// the same, against explicit roots.
#[must_use]
pub fn skills_in(roots: &[PathBuf]) -> Vec<SkillRef> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for root in roots {
        let dir = root.join("skills");
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut names: Vec<_> = entries.filter_map(Result::ok).collect();
        names.sort_by_key(std::fs::DirEntry::file_name);
        for entry in names {
            let file = entry.path().join("SKILL.md");
            let Ok(canonical) = std::fs::canonicalize(&file) else {
                continue;
            };
            // dedup follows the real file, so one skill linked twice is listed once; the
            // path recorded is the logical one the operator recognises.
            if !seen.insert(canonical.clone()) {
                continue;
            }
            let Ok(body) = std::fs::read_to_string(&canonical) else {
                continue;
            };
            out.push(SkillRef {
                name: entry.file_name().to_string_lossy().into_owned(),
                path: file,
                summary: skill_summary(&body),
            });
        }
    }
    out
}

/// persona and project context for a workspace, assembled for the system prompt.
///
/// read only: files are read from the config dir, the workspace and the daemon's own
/// directory, and never created or modified (spec 6.1).
#[must_use]
pub fn context(workspace: &Path, cwd: Option<&Path>) -> Option<String> {
    let mut parts: Vec<String> = context_files(workspace, cwd)
        .into_iter()
        .map(|file| file.body)
        .collect();

    let found = skills(workspace, cwd);
    if !found.is_empty() {
        let mut listing = String::from(
            "Skills available in this workspace. Read the file when the task matches it:\n",
        );
        for skill in found {
            let _ = writeln!(
                &mut listing,
                "- {} ({}): {}",
                skill.name,
                skill.path.display(),
                skill.summary
            );
        }
        parts.push(listing);
    }

    if parts.is_empty() {
        None
    } else {
        Some(parts.join("\n\n"))
    }
}

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

/// a checkpoint's temporary git index, removed however the checkpoint ends: a git command
/// that fails must not leave the index behind in the temp directory.
struct TempIndex(PathBuf);

impl Drop for TempIndex {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
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
    let index = TempIndex(std::env::temp_dir().join(format!(
        "sa-index-{session_id}-{seq}-{}",
        uuid::Uuid::new_v4()
    )));
    let _ = std::fs::remove_file(&index.0);

    git_env(
        workspace,
        &["add", "-A"],
        Some(("GIT_INDEX_FILE", &index.0)),
    )?;
    let tree = git_env(
        workspace,
        &["write-tree"],
        Some(("GIT_INDEX_FILE", &index.0)),
    )?;

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
    // the ref ends in the session's seq, so it is ordered as a number: a lexicographic
    // sort puts `/10` before `/9` and prunes the newest checkpoint instead of the oldest.
    let seq_of = |reference: &str| {
        reference
            .rsplit('/')
            .next()
            .and_then(|seq| seq.parse::<u64>().ok())
    };
    let mut refs: Vec<&str> = raw.lines().filter(|line| !line.is_empty()).collect();
    refs.sort_by_key(|reference| seq_of(reference).unwrap_or_default());
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
#[allow(clippy::expect_used)]
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
        assert_ne!(checkpoint.tree, "");

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
    fn pruning_keeps_the_newest_checkpoints_past_ten() {
        // the regression: the refs were sorted as text, so `/10` sorted before `/9` and the
        // newest checkpoint was the first thing pruned.
        let dir = repo();
        for seq in 1..=11 {
            checkpoint(dir.path(), "s1", seq).expect("checkpoint");
        }
        prune_checkpoints(dir.path(), "s1", 3).expect("prune");
        let refs = git(
            dir.path(),
            &[
                "for-each-ref",
                "--format=%(refname)",
                "refs/sa/checkpoints/s1/",
            ],
        )
        .expect("refs");
        let mut kept: Vec<&str> = refs.lines().collect();
        kept.sort_unstable();
        assert_eq!(
            kept,
            [
                "refs/sa/checkpoints/s1/10",
                "refs/sa/checkpoints/s1/11",
                "refs/sa/checkpoints/s1/9"
            ],
            "the three most recent checkpoints survive"
        );
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
    fn slug_matches_the_pi_contexts_scheme() {
        assert_eq!(
            slug(Path::new("/home/chrisaw/.dotfiles")),
            "home-chrisaw-.dotfiles"
        );
        assert_eq!(
            slug(Path::new("/home/chrisaw/Projects/treble_restlessos")),
            "home-chrisaw-Projects-treble_restlessos"
        );
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
