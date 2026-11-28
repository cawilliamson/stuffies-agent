# workspace-handling
#
# How sa treats the directory a session works in: what it reads from it, what it locks, the
# checkpoints it takes before unattended work, and the rule that it writes nothing of its own
# there.

Feature: workspace-handling
  How sa treats the directory a session works in: what it reads from it, what it locks, the
  checkpoints it takes before unattended work, and the rule that it writes nothing of its own
  there.

  # Requirement: Nothing is written into the workspace
  # sa SHALL never create, modify or delete a file inside a workspace for its own bookkeeping: no
  # directory, no ignore line, no lock file, no cache, no ref in the workspace's `.git`. After any
  # operation, `git status` SHALL be byte-identical to before it. The only files that appear are the
  # edits a turn was asked to make and the files its tools created on purpose.
  @req:nothing-is-written-into-the-workspace
  Scenario: A daemon start writes nothing
    When the daemon starts and a session is created against a repository
    Then `git status` in that repository is unchanged
  @req:nothing-is-written-into-the-workspace
  Scenario: A checkpoint adds nothing to the user's git
    When an unattended turn takes a checkpoint
    Then the workspace's `.git` gains no ref and no object, and `git log --all` is unchanged
  @req:nothing-is-written-into-the-workspace
  Scenario: Build output is the toolchain's, not ours
    When a turn runs a project's tests
    Then whatever they write belongs to that toolchain and sa adds nothing beside it

  # Requirement: Runtime state lives in the XDG roots
  # Everything sa needs SHALL live under `$XDG_CONFIG_HOME/stuffies-agent` for user-authored config
  # and `$XDG_STATE_HOME/stuffies-agent` for runtime state, or under `$XDG_RUNTIME_DIR` for locks
  # and the socket.
  @req:runtime-state-lives-in-the-xdg-roots
  Scenario: A session's state is outside the tree it edits
    When a session runs against a workspace
    Then its database, checkpoints and knowledge live under the state directory

  # Requirement: Context is read, never written
  # sa SHALL read `PERSONALITY.md`, `AGENTS.md`, `RULES.md` and `APPEND_SYSTEM.md` from the deployed
  # per-workspace config directory and from the workspace itself, layering every ancestor scope from
  # the home directory down. It SHALL read skill directories the same way. It SHALL NOT create any
  # of them.
  @req:context-is-read-never-written
  Scenario: A workspace's own AGENTS.md is used
    When a repository already carries an `AGENTS.md`
    Then it is injected and left untouched
  @req:context-is-read-never-written
  Scenario: An absent file is not created
    When a directory has no `AGENTS.md`
    Then nothing is written and the prompt simply carries no project context
  @req:context-is-read-never-written
  Scenario: A parent scope applies to a child
    When a home-level context file and a project-level one both exist
    Then both are injected, outermost first

  # Requirement: The injected context is announced
  # Creating a session or changing its directory SHALL append `context.loaded` naming the
  # directories that contribute context and, for each file, its path, its path relative to the root
  # that provided it, its provider and its token estimate.
  @req:the-injected-context-is-announced
  Scenario: The operator can see what the model was given
    When a session is created in a directory with context files
    Then `context.loaded` lists them with their provider and token counts

  # Requirement: Workspace lock
  # The daemon SHALL hold an exclusive lock for the duration of any turn that can write. The lock
  # SHALL be keyed on the repository's git common directory, so every worktree of one repository
  # shares it, and on the workspace path when the directory is not a repository. The lock file SHALL
  # live under `$XDG_RUNTIME_DIR/stuffies-agent/locks`.
  @req:workspace-lock
  Scenario: Two sessions in one repository serialise
    When a second session starts a turn in a repository already locked
    Then it waits for the lock rather than writing concurrently
  @req:workspace-lock
  Scenario: Worktrees share the lock
    When two worktrees of one repository run turns
    Then they share one lock because they share the git common directory

  # Requirement: Checkpoint before unattended work
  # Before any turn started with `detach: true` the daemon SHALL create a checkpoint and record it
  # in a `checkpoint` event. A checkpoint SHALL be written to a shadow store under
  # `$XDG_STATE_HOME/stuffies-agent/checkpoints/<id>/`, never to the workspace's `.git`. A
  # checkpoint failure SHALL abort the turn before any tool runs.
  @req:checkpoint-before-unattended-work
  Scenario: An unattended turn never starts without a checkpoint
    When a checkpoint fails before a detached turn
    Then the turn is interrupted, the failure is recorded, and no tool runs
  @req:checkpoint-before-unattended-work
  Scenario: A non-repository directory is still checkpointed
    When a detached turn runs in a directory that is not a git repository
    Then the store is named for the directory's path and the checkpoint succeeds

  # Requirement: Checkpoints are written by git
  # The store SHALL be a bare git repository written by the `git` program, so what sa records is
  # exactly what git reads back, and the worktree's bytes, file modes and symlinks SHALL be recorded
  # as they are whatever the machine's git configuration says. The commit SHALL be written under
  # sa's own identity rather than the user's. A machine with no `git` on the path SHALL fail the
  # checkpoint rather than skip it, so the turn it guards does not start.
  @req:checkpoints-are-written-by-git
  Scenario: A checkpoint is readable by git
    When the shadow store is opened with the git binary
    Then the checkpoint commit and its tree are readable
  @req:checkpoints-are-written-by-git
  Scenario: A machine's configuration does not decide what is stored
    When the machine's git configuration disables symlinks
    Then a symlink in the worktree is still checkpointed as a symlink and restored as one
  @req:checkpoints-are-written-by-git
  Scenario: No git means no checkpoint, and no unattended turn
    When no git binary is on the path
    Then the checkpoint fails and the detached turn is interrupted rather than run unguarded

  # Requirement: Checkpoint shape
  # A checkpoint SHALL be a commit with no parent over a tree of every file the worktree holds,
  # honouring the project's ignore files and excluding `.git`, written to
  # `refs/sa/checkpoints/<session_id>/<seq>`. `base_commit` SHALL be recorded when the workspace has
  # a HEAD and omitted when it does not.
  @req:checkpoint-shape
  Scenario: A checkpoint is a tree to restore, not a history
    When a checkpoint commit is inspected
    Then it has no parent and its tree holds the worktree's files
  @req:checkpoint-shape
  Scenario: Ignored files are not checkpointed
    When the worktree holds files excluded by its ignore files
    Then they are absent from the checkpoint tree

  # Requirement: Checkpoint retention
  # A session SHALL keep its 10 most recent checkpoints, and a checkpoint older than seven days
  # SHALL be deleted with its objects. A store with no checkpoints left SHALL be removed. Nothing
  # outside `refs/sa/` SHALL be touched.
  @req:checkpoint-retention
  Scenario: Old checkpoints are pruned by count
    When an eleventh checkpoint is taken for a session
    Then the oldest is removed
  @req:checkpoint-retention
  Scenario: Old checkpoints are pruned by age
    When a checkpoint is older than seven days at daemon start
    Then it is deleted and an emptied store is removed

  # Requirement: Listing and rewinding
  # `checkpoint.list` SHALL return each checkpoint's id, session id, sequence and time, newest
  # first. `checkpoint.rewind` SHALL accept a commit id or a unique prefix of it, restore the files
  # the checkpoint holds, and leave files created after it alone. An unknown or ambiguous prefix
  # SHALL be refused with a message saying which.
  @req:listing-and-rewinding
  Scenario: A rewind restores what was there
    When a checkpoint is rewound
    Then the files it holds are restored to their checkpointed contents
  @req:listing-and-rewinding
  Scenario: A rewind does not delete later work
    When a file was created after the checkpoint
    Then the rewind leaves it in place
  @req:listing-and-rewinding
  Scenario: A prefix that matches two checkpoints is refused
    When a rewind id matches more than one checkpoint
    Then it is refused and the message asks for more of the id
  @req:listing-and-rewinding
  Scenario: A rewind is recorded
    When a rewind succeeds
    Then a `checkpoint` event carrying the id and the number of files restored is appended
