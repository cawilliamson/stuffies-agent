# workspace-handling

## Purpose

How sa treats the directory a session works in: what it reads from it, what it locks, the
checkpoints it takes before unattended work, and the rule that it writes nothing of its own
there.

## Requirements

### Requirement: Nothing is written into the workspace

sa SHALL never create, modify or delete a file inside a workspace for its own bookkeeping: no
directory, no ignore line, no lock file, no cache, no ref in the workspace's `.git`. After
any operation, `git status` SHALL be byte-identical to before it. The only files that appear
are the edits a turn was asked to make and the files its tools created on purpose.

#### Scenario: A daemon start writes nothing

- **WHEN** the daemon starts and a session is created against a repository
- **THEN** `git status` in that repository is unchanged

#### Scenario: A checkpoint adds nothing to the user's git

- **WHEN** an unattended turn takes a checkpoint
- **THEN** the workspace's `.git` gains no ref and no object, and `git log --all` is unchanged

#### Scenario: Build output is the toolchain's, not ours

- **WHEN** a turn runs a project's tests
- **THEN** whatever they write belongs to that toolchain and sa adds nothing beside it

### Requirement: Runtime state lives in the XDG roots

Everything sa needs SHALL live under `$XDG_CONFIG_HOME/stuffies-agent` for user-authored
config and `$XDG_STATE_HOME/stuffies-agent` for runtime state, or under `$XDG_RUNTIME_DIR` for
locks and the socket.

#### Scenario: A session's state is outside the tree it edits

- **WHEN** a session runs against a workspace
- **THEN** its database, checkpoints and knowledge live under the state directory

### Requirement: Context is read, never written

sa SHALL read `PERSONALITY.md`, `AGENTS.md`, `RULES.md` and `APPEND_SYSTEM.md` from the
deployed per-workspace config directory and from the workspace itself, layering every
ancestor scope from the home directory down. It SHALL read skill directories the same way. It
SHALL NOT create any of them.

#### Scenario: A workspace's own AGENTS.md is used

- **WHEN** a repository already carries an `AGENTS.md`
- **THEN** it is injected and left untouched

#### Scenario: An absent file is not created

- **WHEN** a directory has no `AGENTS.md`
- **THEN** nothing is written and the prompt simply carries no project context

#### Scenario: A parent scope applies to a child

- **WHEN** a home-level context file and a project-level one both exist
- **THEN** both are injected, outermost first

### Requirement: The injected context is announced

Creating a session or changing its directory SHALL append `context.loaded` naming the
directories that contribute context and, for each file, its path, its path relative to the
root that provided it, its provider and its token estimate.

#### Scenario: The operator can see what the model was given

- **WHEN** a session is created in a directory with context files
- **THEN** `context.loaded` lists them with their provider and token counts

### Requirement: Workspace lock

The daemon SHALL hold an exclusive lock for the duration of any turn that can write. The lock
SHALL be keyed on the repository's git common directory, so every worktree of one repository
shares it, and on the workspace path when the directory is not a repository. The lock file
SHALL live under `$XDG_RUNTIME_DIR/stuffies-agent/locks`.

#### Scenario: Two sessions in one repository serialise

- **WHEN** a second session starts a turn in a repository already locked
- **THEN** it waits for the lock rather than writing concurrently

#### Scenario: Worktrees share the lock

- **WHEN** two worktrees of one repository run turns
- **THEN** they share one lock because they share the git common directory

### Requirement: Checkpoint before unattended work

Before any turn started with `detach: true` the daemon SHALL create a checkpoint and record
it in a `checkpoint` event. A checkpoint SHALL be written to a shadow store under
`$XDG_STATE_HOME/stuffies-agent/checkpoints/<id>/`, never to the workspace's `.git`. A
checkpoint failure SHALL abort the turn before any tool runs.

#### Scenario: An unattended turn never starts without a checkpoint

- **WHEN** a checkpoint fails before a detached turn
- **THEN** the turn is interrupted, the failure is recorded, and no tool runs

#### Scenario: A non-repository directory is still checkpointed

- **WHEN** a detached turn runs in a directory that is not a git repository
- **THEN** the store is named for the directory's path and the checkpoint succeeds

### Requirement: Checkpoints are written with gix

The store SHALL be written by `gix` rather than by running `git`, so the daemon needs no git
binary and inherits none of the user's git configuration. A store sa creates SHALL be readable
by `git`.

#### Scenario: The daemon works without git installed

- **WHEN** no git binary is on the path
- **THEN** checkpoints are still created

#### Scenario: A checkpoint is readable by git

- **WHEN** the shadow store is opened with the git binary
- **THEN** the checkpoint commit and its tree are readable

### Requirement: Checkpoint shape

A checkpoint SHALL be a commit with no parent over a tree of every file the worktree holds,
honouring the project's ignore files and excluding `.git`, written to
`refs/sa/checkpoints/<session_id>/<seq>`. `base_commit` SHALL be recorded when the workspace
has a HEAD and omitted when it does not.

#### Scenario: A checkpoint is a tree to restore, not a history

- **WHEN** a checkpoint commit is inspected
- **THEN** it has no parent and its tree holds the worktree's files

#### Scenario: Ignored files are not checkpointed

- **WHEN** the worktree holds files excluded by its ignore files
- **THEN** they are absent from the checkpoint tree

### Requirement: Checkpoint retention

A session SHALL keep its 10 most recent checkpoints, and a checkpoint older than seven days
SHALL be deleted with its objects. A store with no checkpoints left SHALL be removed.
Nothing outside `refs/sa/` SHALL be touched.

#### Scenario: Old checkpoints are pruned by count

- **WHEN** an eleventh checkpoint is taken for a session
- **THEN** the oldest is removed

#### Scenario: Old checkpoints are pruned by age

- **WHEN** a checkpoint is older than seven days at daemon start
- **THEN** it is deleted and an emptied store is removed

### Requirement: Listing and rewinding

`checkpoint.list` SHALL return each checkpoint's id, session id, sequence and time, newest
first. `checkpoint.rewind` SHALL accept a commit id or a unique prefix of it, restore the
files the checkpoint holds, and leave files created after it alone. An unknown or ambiguous
prefix SHALL be refused with a message saying which.

#### Scenario: A rewind restores what was there

- **WHEN** a checkpoint is rewound
- **THEN** the files it holds are restored to their checkpointed contents

#### Scenario: A rewind does not delete later work

- **WHEN** a file was created after the checkpoint
- **THEN** the rewind leaves it in place

#### Scenario: A prefix that matches two checkpoints is refused

- **WHEN** a rewind id matches more than one checkpoint
- **THEN** it is refused and the message asks for more of the id

#### Scenario: A rewind is recorded

- **WHEN** a rewind succeeds
- **THEN** a `checkpoint` event carrying the id and the number of files restored is appended
