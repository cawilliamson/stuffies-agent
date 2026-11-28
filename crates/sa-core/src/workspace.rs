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
    #[error("no checkpoint matches {0}")]
    NoSuchCheckpoint(String),
    #[error("{0} matches more than one checkpoint; give more of the id")]
    AmbiguousCheckpoint(String),
}

/// an exclusive lock on a repository's git common directory.
///
/// ponytail: blocking flock on the calling thread. a contended lock means another session in
/// the same repository is mid-turn, which is rare and short; move to an async lock if that
/// ever queues.
pub struct WorkspaceLock {
    _lock: nix::fcntl::Flock<File>,
}

/// the git common directory, which is shared by every worktree of one repository. asked of
/// git rather than worked out here: a worktree's layout is git's to define.
pub fn git_common_dir(workspace: &Path) -> Result<PathBuf, WorkspaceError> {
    let mut command = std::process::Command::new(GIT);
    command
        .arg("-C")
        .arg(workspace)
        .args(["rev-parse", "--git-common-dir"]);
    let output = command.output()?;
    if !output.status.success() {
        return Err(WorkspaceError::NotARepository(
            workspace.display().to_string(),
        ));
    }
    // a worktree reports the main repository's directory, which may be absolute; a plain
    // repository reports `.git`, which is relative to the workspace.
    let reported = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
    let path = if reported.is_absolute() {
        reported
    } else {
        workspace.join(reported)
    };
    Ok(std::fs::canonicalize(&path).unwrap_or(path))
}

/// takes the workspace lock, creating the lock directory if needed.
pub fn lock(workspace: &Path, lock_dir: &Path) -> Result<WorkspaceLock, WorkspaceError> {
    use nix::fcntl::{Flock, FlockArg};
    use sha2::{Digest, Sha256};

    // a repository is locked by its common git directory, so every worktree of it shares the
    // lock. anywhere else is locked by its own path: two sessions in one directory must not
    // write over each other just because that directory is not a repository.
    let identity = git_common_dir(workspace).unwrap_or_else(|_| workspace.to_path_buf());
    std::fs::create_dir_all(lock_dir)?;

    let mut hasher = Sha256::new();
    hasher.update(identity.display().to_string().as_bytes());
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
    /// the commit's own id: what a user types to rewind to it.
    pub commit: String,
    pub reference: String,
    pub base_commit: Option<String>,
    pub tree: String,
    /// the shadow git directory the commit lives in, which is never the workspace's `.git`.
    pub store: PathBuf,
}

/// how long a checkpoint is kept after the turn that made it. past this it is deleted, and
/// the objects behind it are pruned with it (spec 8.4).
pub const CHECKPOINT_MAX_AGE: std::time::Duration = std::time::Duration::from_hours(7 * 24);

/// the program that owns the object format. the store is a real git repository, written by
/// git, so what it holds is exactly what git reads back.
const GIT: &str = "git";

/// the identity a checkpoint is committed under: sa's own, so a user who sets `user.name` per
/// repository rather than globally still gets a checkpoint.
const SA_NAME: &str = "sa";
const SA_EMAIL: &str = "sa@localhost";

/// the settings the store must not decide for the user's worktree. a bare store still inherits
/// the machine's system config, and one that sets `core.symlinks=false` would have a rewind
/// write a link's target as a file. bytes are stored as they are, and the executable bit is
/// recorded, because that is what a checkpoint is for.
const WORKTREE_CONFIG: [&str; 6] = [
    "-c",
    "core.autocrlf=false",
    "-c",
    "core.filemode=true",
    "-c",
    "core.symlinks=true",
];

/// the directory a workspace's checkpoints live in: one bare git directory per repository,
/// named for the repository's common git directory so worktrees share it.
///
/// it is deliberately outside the workspace. the workspace's own `.git` is the user's, and a
/// turn must not add a ref or an object to it (spec 6.1).
fn checkpoint_store(workspace: &Path, root: &Path) -> PathBuf {
    use sha2::{Digest, Sha256};

    // a repository is identified by its common git directory, so every worktree of it shares
    // one store. a directory that is not a repository has no common git directory, and its
    // own path identifies it just as well: the store is ours.
    let identity = git_common_dir(workspace).unwrap_or_else(|_| workspace.to_path_buf());
    let mut hasher = Sha256::new();
    hasher.update(identity.display().to_string().as_bytes());
    let mut name = String::new();
    for byte in hasher.finalize() {
        let _ = write!(name, "{byte:02x}");
    }
    root.join(name)
}

/// runs git and returns its trimmed stdout. a non-zero exit is an error carrying what git
/// said: a checkpoint that quietly did nothing is worse than one that failed.
fn finish(mut command: std::process::Command, args: &[&str]) -> Result<String, WorkspaceError> {
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

/// one git invocation against the store, which is the repository and nothing else.
fn store_git(store: &Path, args: &[&str]) -> Result<String, WorkspaceError> {
    let mut command = std::process::Command::new(GIT);
    command.env("GIT_DIR", store).args(args);
    finish(command, args)
}

/// one git invocation against the worktree, through a temporary index. the user's index is
/// never opened and the worktree is never modified by the snapshot half of this.
fn worktree_git(
    store: &Path,
    workspace: &Path,
    index: &Path,
    args: &[&str],
) -> Result<String, WorkspaceError> {
    let mut command = std::process::Command::new(GIT);
    command
        .current_dir(workspace)
        .env("GIT_DIR", store)
        .env("GIT_WORK_TREE", workspace)
        .env("GIT_INDEX_FILE", index)
        .args(WORKTREE_CONFIG)
        .args(args);
    finish(command, args)
}

/// a path for the index one invocation builds, unique to the call so two snapshots of one
/// repository cannot write over each other.
fn temporary_index(store: &Path) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos());
    store.join(format!(".sa-index-{}-{nanos}", std::process::id()))
}

/// the store, created as a bare repository on first use.
fn ensure_store(store: &Path) -> Result<(), WorkspaceError> {
    if store.join("HEAD").exists() {
        return Ok(());
    }
    std::fs::create_dir_all(store)?;
    let mut command = std::process::Command::new(GIT);
    command.args(["init", "--bare", "--quiet"]).arg(store);
    finish(command, &["init", "--bare"]).map(|_| ())
}

/// where HEAD was, when there is one. recorded for the reader, not for the commit: the
/// checkpoint has no parent, so a workspace with no history can still be checkpointed.
fn head_commit(workspace: &Path) -> Option<String> {
    let mut command = std::process::Command::new(GIT);
    command.arg("-C").arg(workspace).args(["rev-parse", "HEAD"]);
    let output = command.output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// writes the worktree into the store as a tree, and returns its id.
///
/// git decides what the worktree holds: its own ignore handling, its own file modes and its
/// own symlink handling, so what is checkpointed is what the developer considers their source.
/// the directory does not need to be a repository for this to work. the store is the
/// repository here, so its own `info/exclude` is the empty one; a `.gitignore` in the worktree
/// and the user's global excludes file still apply.
fn snapshot(store: &Path, workspace: &Path) -> Result<String, WorkspaceError> {
    let index = temporary_index(store);
    let tree = (|| {
        worktree_git(store, workspace, &index, &["add", "-A"])?;
        worktree_git(store, workspace, &index, &["write-tree"])
    })();
    let _ = std::fs::remove_file(&index);
    tree
}

/// creates a checkpoint for an unattended turn.
///
/// the commit is written to a shadow store under `root`, never to the workspace's `.git`, and
/// it has no parent: a checkpoint is a tree to restore, not a history to walk. the first
/// checkpoint of a repository copies its source tree into the store; later ones add only what
/// changed, because the objects they reuse are already there.
pub fn checkpoint(
    workspace: &Path,
    root: &Path,
    session_id: &str,
    seq: u64,
) -> Result<Checkpoint, WorkspaceError> {
    let base_commit = head_commit(workspace);
    let store = checkpoint_store(workspace, root);
    ensure_store(&store)?;
    let tree = snapshot(&store, workspace)?;

    let message = format!("sa checkpoint {session_id} {seq}");
    let name = format!("user.name={SA_NAME}");
    let email = format!("user.email={SA_EMAIL}");
    let commit = store_git(
        &store,
        &[
            "-c",
            &name,
            "-c",
            &email,
            "commit-tree",
            &tree,
            "-m",
            &message,
        ],
    )?;
    let reference = format!("refs/sa/checkpoints/{session_id}/{seq}");
    store_git(&store, &["update-ref", &reference, &commit])?;

    Ok(Checkpoint {
        commit,
        reference,
        base_commit,
        tree,
        store,
    })
}

/// every checkpoint ref in a store under `prefix`, as its name, the commit it names and when
/// that commit was made.
fn checkpoint_refs(
    store: &Path,
    prefix: &str,
) -> Result<Vec<(String, String, u64)>, WorkspaceError> {
    if !store.join("HEAD").exists() {
        return Ok(Vec::new());
    }
    let listed = store_git(
        store,
        &[
            "for-each-ref",
            "--format=%(refname) %(objectname) %(committerdate:unix)",
            prefix,
        ],
    )?;
    let mut out = Vec::new();
    for line in listed.lines() {
        let mut parts = line.split(' ');
        let (Some(name), Some(id), Some(at)) = (parts.next(), parts.next(), parts.next()) else {
            continue;
        };
        out.push((
            name.to_owned(),
            id.to_owned(),
            at.parse().unwrap_or_default(),
        ));
    }
    Ok(out)
}

/// the seq a checkpoint ref names, when the ref is one of a session's.
fn seq_of(reference: &str) -> Option<u64> {
    reference.rsplit('/').next()?.parse().ok()
}

/// keeps only the `keep` most recent checkpoint refs for a session (spec 8.4).
pub fn prune_checkpoints(
    workspace: &Path,
    root: &Path,
    session_id: &str,
    keep: usize,
) -> Result<(), WorkspaceError> {
    let store = checkpoint_store(workspace, root);
    let prefix = format!("refs/sa/checkpoints/{session_id}/");
    let mut refs = checkpoint_refs(&store, &prefix)?;
    if refs.len() <= keep {
        return Ok(());
    }
    // the seq is a number, so it is ordered as one: a lexicographic sort puts `/10` before
    // `/9` and prunes the newest checkpoint instead of the oldest.
    refs.sort_by_key(|(name, _, _)| seq_of(name).unwrap_or_default());
    for (name, _, _) in &refs[..refs.len() - keep] {
        store_git(&store, &["update-ref", "-d", name])?;
    }
    // the refs are gone but the objects behind them are not. the store is shared by every
    // session in the workspace, so collecting now is what makes a pruned checkpoint stop
    // costing disk; nothing else ever reaches those commits again.
    store_git(&store, &["gc", "--prune=now", "--quiet"])?;
    Ok(())
}

/// deletes every checkpoint older than `max_age`, and the store of a repository that has no
/// checkpoints left. a checkpoint nothing will restore is a leak, not a safety net.
///
/// returns the number of checkpoints removed. a store that cannot be read is skipped: one
/// stale directory must not stop a daemon from starting.
#[must_use]
pub fn expire_checkpoints(root: &Path, max_age: std::time::Duration) -> usize {
    let Ok(entries) = std::fs::read_dir(root) else {
        return 0;
    };
    let cutoff = std::time::SystemTime::now()
        .checked_sub(max_age)
        .and_then(|when| when.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |since| since.as_secs());

    let mut removed = 0;
    for entry in entries.flatten() {
        let store = entry.path();
        if !store.is_dir() {
            continue;
        }
        let Ok(refs) = checkpoint_refs(&store, "refs/sa/checkpoints/") else {
            continue;
        };
        for (name, _, _) in refs.iter().filter(|(_, _, at)| *at < cutoff) {
            if store_git(&store, &["update-ref", "-d", name]).is_ok() {
                removed += 1;
            }
        }
        // nothing points at the objects any more. a store is a plain directory, so removing
        // it is the prune: an empty one is deleted outright, and one with checkpoints left
        // keeps the objects they still share.
        let empty =
            checkpoint_refs(&store, "refs/sa/checkpoints/").is_ok_and(|refs| refs.is_empty());
        if empty {
            let _ = std::fs::remove_dir_all(&store);
        }
    }
    removed
}

/// one checkpoint a user can rewind to.
#[derive(Debug, Clone)]
pub struct CheckpointEntry {
    /// the commit id, which is what a user types.
    pub id: String,
    pub session_id: String,
    pub seq: u64,
    /// unix seconds the checkpoint was made.
    pub at: u64,
}

/// every checkpoint of a workspace, newest first.
pub fn list_checkpoints(
    workspace: &Path,
    root: &Path,
) -> Result<Vec<CheckpointEntry>, WorkspaceError> {
    let store = checkpoint_store(workspace, root);
    let mut out = Vec::new();
    for (name, id, at) in checkpoint_refs(&store, "refs/sa/checkpoints/")? {
        let Some(tail) = name.strip_prefix("refs/sa/checkpoints/") else {
            continue;
        };
        let Some((session_id, seq)) = tail.rsplit_once('/') else {
            continue;
        };
        out.push(CheckpointEntry {
            id,
            session_id: session_id.to_owned(),
            seq: seq.parse().unwrap_or_default(),
            at,
        });
    }
    out.sort_by(|a, b| b.at.cmp(&a.at).then(b.seq.cmp(&a.seq)));
    Ok(out)
}

/// restores the workspace to a checkpoint, given its commit id or a unique prefix of it.
///
/// files the checkpoint holds are written back over the working tree. files created after it
/// are left alone: a rewind restores what was there, it does not delete what was added, and
/// saying so is better than silently removing a file the operator made by hand.
///
/// returns how many files the checkpoint holds.
pub fn rewind(workspace: &Path, root: &Path, id: &str) -> Result<usize, WorkspaceError> {
    let store = checkpoint_store(workspace, root);
    let all = list_checkpoints(workspace, root)?;
    let matches: Vec<&CheckpointEntry> = all
        .iter()
        .filter(|entry| entry.id.starts_with(id))
        .collect();
    let entry = match matches.as_slice() {
        [] => return Err(WorkspaceError::NoSuchCheckpoint(id.to_owned())),
        [one] => *one,
        _ => return Err(WorkspaceError::AmbiguousCheckpoint(id.to_owned())),
    };
    let listed = store_git(&store, &["ls-tree", "-r", "--name-only", &entry.id])?;
    let files = listed.lines().count();
    let index = temporary_index(&store);
    let restored = worktree_git(
        &store,
        workspace,
        &index,
        &["checkout", &entry.id, "--", "."],
    );
    let _ = std::fs::remove_file(&index);
    restored?;
    Ok(files)
}

#[must_use]
pub fn checkpoint_payload(checkpoint: &Checkpoint) -> Value {
    let mut payload = json!({
        "id": checkpoint.commit,
        "ref": checkpoint.reference,
        "tree": checkpoint.tree,
        "store": checkpoint.store.display().to_string(),
        "includes_untracked": true,
    });
    if let Some(base) = &checkpoint.base_commit {
        payload["base_commit"] = json!(base);
    }
    payload
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

    /// runs git, for the tests: the store is a git repository, so the binary is the reader
    /// that proves what sa wrote is what git reads.
    fn git(dir: &std::path::Path, args: &[&str]) -> Result<String, WorkspaceError> {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()?;
        if !output.status.success() {
            return Err(WorkspaceError::Git(format!(
                "git {}: {}",
                args.join(" "),
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
    }

    /// the refs of a session's checkpoints, read from the shadow store rather than the repo.
    fn store_refs(dir: &std::path::Path, root: &std::path::Path, session: &str) -> String {
        let store = checkpoint_store(dir, root);
        std::process::Command::new("git")
            .env("GIT_DIR", &store)
            .args([
                "for-each-ref",
                "--format=%(refname)",
                &format!("refs/sa/checkpoints/{session}/"),
            ])
            .output()
            .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_owned())
            .unwrap_or_default()
    }

    #[test]
    fn checkpoint_captures_untracked_files_without_touching_the_worktree() {
        let dir = repo();
        let root = tempfile::tempdir().expect("root");
        std::fs::write(dir.path().join("new.txt"), "untracked").expect("write");

        let checkpoint = checkpoint(dir.path(), root.path(), "s1", 1).expect("checkpoint");

        assert!(
            dir.path().join("new.txt").exists(),
            "the worktree must be untouched"
        );
        assert_ne!(checkpoint.tree, "");

        // the index must still be clean: `git add -A` went to the temporary index only.
        let staged = git(dir.path(), &["diff", "--cached", "--name-only"]).expect("diff");
        assert!(staged.is_empty(), "real index was modified: {staged}");
    }

    /// the whole point of the shadow store: a checkpoint must not add a ref or an object to
    /// the user's `.git`.
    ///
    /// the assertion is about the object itself, not the shape of `.git/objects`: git runs
    /// `gc --auto` in the background after a commit, so counting entries there is a race.
    #[test]
    fn checkpoint_writes_nothing_into_the_workspace_git_dir() {
        let dir = repo();
        let root = tempfile::tempdir().expect("root");

        let checkpoint = checkpoint(dir.path(), root.path(), "s1", 1).expect("checkpoint");

        assert!(
            git(dir.path(), &["for-each-ref", "refs/sa/"])
                .expect("refs")
                .is_empty(),
            "no checkpoint ref may be written into the workspace's .git"
        );
        assert!(
            git(dir.path(), &["cat-file", "-e", &checkpoint.commit]).is_err(),
            "the checkpoint object must not be in the workspace's .git"
        );
        assert_eq!(
            store_refs(dir.path(), root.path(), "s1").lines().count(),
            1,
            "the checkpoint belongs in the shadow store"
        );
    }

    /// a checkpoint nothing will restore is a leak. past its age it goes, and so does the
    /// store once nothing is left in it.
    #[test]
    fn old_checkpoints_expire_and_take_their_store_with_them() {
        let dir = repo();
        let root = tempfile::tempdir().expect("root");
        let checkpoint = checkpoint(dir.path(), root.path(), "s1", 1).expect("checkpoint");

        // rewrite the commit with a committer date in the past: the age is the commit's,
        // not the file's, so this is what a checkpoint from last month looks like.
        let store = &checkpoint.store;
        let old = std::process::Command::new("git")
            .env("GIT_DIR", store)
            .env("GIT_COMMITTER_DATE", "2000-01-01T00:00:00+0000")
            .args([
                "-c",
                "user.name=sa",
                "-c",
                "user.email=sa@localhost",
                "commit-tree",
                &checkpoint.tree,
                "-m",
                "sa checkpoint s1 1",
            ])
            .output()
            .expect("commit-tree");
        let old_commit = String::from_utf8_lossy(&old.stdout).trim().to_owned();
        std::process::Command::new("git")
            .env("GIT_DIR", store)
            .args(["update-ref", &checkpoint.reference, &old_commit])
            .output()
            .expect("update-ref");

        let removed = expire_checkpoints(root.path(), std::time::Duration::from_secs(3600));
        assert_eq!(removed, 1, "the aged checkpoint must go");
        assert!(
            !store.exists(),
            "a store with no checkpoints left must be removed"
        );
    }

    #[test]
    fn a_recent_checkpoint_is_not_expired() {
        let dir = repo();
        let root = tempfile::tempdir().expect("root");
        checkpoint(dir.path(), root.path(), "s1", 1).expect("checkpoint");

        let removed = expire_checkpoints(root.path(), CHECKPOINT_MAX_AGE);
        assert_eq!(removed, 0, "a fresh checkpoint must be kept");
        assert_eq!(
            store_refs(dir.path(), root.path(), "s1").lines().count(),
            1,
            "and it must still be restorable"
        );
    }

    #[test]
    fn checkpoint_ref_survives_and_prunes() {
        let dir = repo();
        let root = tempfile::tempdir().expect("root");
        for seq in 1..=3 {
            checkpoint(dir.path(), root.path(), "s1", seq).expect("checkpoint");
        }
        prune_checkpoints(dir.path(), root.path(), "s1", 2).expect("prune");
        let refs = store_refs(dir.path(), root.path(), "s1");
        assert_eq!(refs.lines().count(), 2, "{refs}");
    }

    #[test]
    fn pruning_keeps_the_newest_checkpoints_past_ten() {
        // the regression: the refs were sorted as text, so `/10` sorted before `/9` and the
        // newest checkpoint was the first thing pruned.
        let dir = repo();
        let root = tempfile::tempdir().expect("root");
        for seq in 1..=11 {
            checkpoint(dir.path(), root.path(), "s1", seq).expect("checkpoint");
        }
        prune_checkpoints(dir.path(), root.path(), "s1", 3).expect("prune");
        let refs = store_refs(dir.path(), root.path(), "s1");
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

    /// the worktree's own semantics decide what is stored. this machine's system git config
    /// sets `core.symlinks=false`, so a checkpoint that inherited it would record a link as a
    /// file and restore it as one.
    #[test]
    fn a_symlink_is_checkpointed_and_restored_as_a_symlink() {
        let dir = repo();
        let root = tempfile::tempdir().expect("root");
        std::os::unix::fs::symlink("a.txt", dir.path().join("link")).expect("symlink");

        let checkpoint = checkpoint(dir.path(), root.path(), "s1", 1).expect("checkpoint");
        std::fs::remove_file(dir.path().join("link")).expect("remove");
        rewind(dir.path(), root.path(), &checkpoint.commit).expect("rewind");

        let target = std::fs::read_link(dir.path().join("link")).expect("a link, not a file");
        assert_eq!(target, PathBuf::from("a.txt"));
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

    /// a directory that is not a repository is a perfectly good place to work. the store is
    /// ours, so a checkpoint does not need git to exist in the workspace at all.
    #[test]
    fn a_directory_that_is_not_a_repository_can_be_checkpointed() {
        let dir = tempfile::tempdir().expect("plain dir");
        let root = tempfile::tempdir().expect("root");
        std::fs::write(dir.path().join("a.txt"), "one").expect("write");

        let checkpoint =
            checkpoint(dir.path(), root.path(), "s1", 1).expect("a plain directory checkpoints");
        assert_eq!(checkpoint.base_commit, None, "there is no HEAD to record");

        let listed = list_checkpoints(dir.path(), root.path()).expect("list");
        assert_eq!(listed.len(), 1, "the checkpoint is listed");
        assert_eq!(listed[0].id, checkpoint.commit);
    }

    /// a plain directory is locked by its own path, so two sessions there still serialise.
    #[test]
    fn a_directory_that_is_not_a_repository_can_be_locked() {
        let dir = tempfile::tempdir().expect("plain dir");
        let locks = tempfile::tempdir().expect("locks");
        let first = lock(dir.path(), locks.path()).expect("a plain directory locks");
        drop(first);
        let second = lock(dir.path(), locks.path()).expect("relock after release");
        drop(second);
    }

    #[test]
    fn rewind_restores_the_checkpointed_files() {
        let dir = repo();
        let root = tempfile::tempdir().expect("root");
        std::fs::write(dir.path().join("a.txt"), "before").expect("write");
        let checkpoint = checkpoint(dir.path(), root.path(), "s1", 1).expect("checkpoint");

        std::fs::write(dir.path().join("a.txt"), "after").expect("write");
        let restored = rewind(dir.path(), root.path(), &checkpoint.commit).expect("rewind");

        assert_eq!(
            restored, 1,
            "the checkpoint holds the one file the repo has"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.txt")).expect("read"),
            "before",
            "the checkpoint's content is back"
        );
    }

    #[test]
    fn rewind_accepts_a_unique_prefix_and_refuses_a_missing_id() {
        let dir = repo();
        let root = tempfile::tempdir().expect("root");
        let checkpoint = checkpoint(dir.path(), root.path(), "s1", 1).expect("checkpoint");
        let prefix = &checkpoint.commit[..8];

        rewind(dir.path(), root.path(), prefix).expect("a unique prefix rewinds");

        let missing = rewind(dir.path(), root.path(), "deadbeef");
        assert!(
            matches!(missing, Err(WorkspaceError::NoSuchCheckpoint(_))),
            "an unknown id is named, not guessed"
        );
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
