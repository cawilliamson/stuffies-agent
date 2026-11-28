# Spec Delta

## Purpose

Moving a session, with its history and its workspace state, between two hosts, without two
writers and without silent divergence.

## ADDED Requirements

### Requirement: Ownership and fencing

A session SHALL record an owner of `{host_fingerprint, epoch}`. A daemon SHALL accept a write
only when the session's epoch equals the one it holds, and SHALL otherwise refuse with
`-32006` naming both epochs. Fencing SHALL use integer comparison and SHALL NOT use
wall-clock time.

#### Scenario: A stale host is refused

- **WHEN** a daemon holding an older epoch starts a turn
- **THEN** it is refused with `-32006` and both epochs are named

#### Scenario: Clock skew is irrelevant

- **WHEN** two hosts disagree about the time of day
- **THEN** fencing is unaffected

#### Scenario: The host fingerprint is stable and private

- **WHEN** the fingerprint is derived
- **THEN** it is a hash of the machine id or a generated UUID, not a hostname or an address

### Requirement: Move

`handoff.export` with mode `move` SHALL quiesce the session at a turn boundary, write a
checkpoint, export the bundle, increment the epoch and move the session to `relocated`, which
refuses further turns. `handoff.import` SHALL take ownership at the new epoch and
`handoff.accept` SHALL complete the move. History SHALL stay linear.

#### Scenario: The source stops writing

- **WHEN** a move has been accepted on the target
- **THEN** the source refuses a turn with `-32006`

#### Scenario: A relocated session is still visible

- **WHEN** a session has been moved away
- **THEN** it is listed on the source as `relocated` and can be handed back

#### Scenario: History stays linear across the move

- **WHEN** the moved session is replayed on the target
- **THEN** its events are the source's events with no gap and no rewrite

### Requirement: Fork

`session.fork` SHALL copy a session into a new workspace, set `parent_id` to the source and
give the copy its own epoch lineage. Both sessions SHALL remain writable and SHALL diverge
with no merge. The target workspace SHALL be required and SHALL NOT exist or SHALL be empty.

#### Scenario: A fork needs its own tree

- **WHEN** `session.fork` names a workspace that exists and is not empty
- **THEN** it is refused

#### Scenario: A fork leaves the source untouched

- **WHEN** a session is forked
- **THEN** the source workspace's files are unchanged

#### Scenario: The copy is identifiable

- **WHEN** a forked session is listed
- **THEN** it carries `parent_id` naming the source

### Requirement: Canonical bundle

A handoff SHALL be a single container file holding `manifest.json`, `events.jsonl`,
`workspace.patch` and `untracked/`, written atomically as a temp file plus rename. The
manifest SHALL carry `format_version`, `session_id`, `owner_epoch`, `seq_range`,
`content_hash`, `created_at`, `source_host`, `workspace` and `mode`.

`content_hash` SHALL be sha256 over the concatenation of the event log and the patch in that
order, with the untracked tree hashed as a sorted list of `path\0sha256` lines.

#### Scenario: A partial bundle is impossible

- **WHEN** the process dies mid-export
- **THEN** no partial bundle is visible under the target name

#### Scenario: The bundle is readable without the schema

- **WHEN** the bundle is read by a process that has never seen sqlite
- **THEN** every event and its payload are readable as JSON lines

### Requirement: Blobs are a storage detail

Oversized payload fields SHALL be flattened back into `events.jsonl` on export and re-split
into the blob table on import, so the bundle format never exposes the storage layout.

#### Scenario: A spilled payload travels whole

- **WHEN** a session with a large tool output is exported
- **THEN** the event log carries the output itself, not a reference

### Requirement: Export preconditions

Export SHALL refuse, with a specific error, when the tree holds state the descriptor cannot
capture: submodules; any file in the transfer set above 8 MiB, or the set above 64 MiB; an
untracked file that is ignored by `.gitignore` but referenced by the patch; or a dirty index
that cannot be represented as a patch.

#### Scenario: A submodule is refused by name

- **WHEN** the workspace has a `.gitmodules`
- **THEN** export is refused and names the submodule

#### Scenario: An oversized transfer is refused

- **WHEN** one file in the transfer set exceeds 8 MiB
- **THEN** export is refused and names the file and the limit

#### Scenario: An ambiguous ignored file is refused rather than guessed

- **WHEN** the patch touches a path the project's ignore files exclude
- **THEN** export is refused rather than choosing

### Requirement: Secrets never enter the bundle

Export SHALL refuse when any path in the transfer set matches `*.pem`, `*.key`, `*.p12`,
`id_*`, `.env`, `.env.*`, `*credentials*` or `*secret*`. Nothing under either XDG root SHALL
be transferred. `--allow-secret-paths` SHALL downgrade the refusal to a warning recorded in
`handoff.exported`.

#### Scenario: A key is refused

- **WHEN** the transfer set contains `id_rsa`
- **THEN** export is refused and names the path

#### Scenario: An override is visible in history

- **WHEN** export runs with `--allow-secret-paths`
- **THEN** `handoff.exported` records the warning and the paths

#### Scenario: A bundle carries no credential

- **WHEN** a bundle is imported on a host with no token configured
- **THEN** the import still verifies, and the turn that needs a token fails later, naming it

### Requirement: Import verification

Import SHALL verify before any mutation and SHALL reject atomically when `format_version` is
unsupported, `content_hash` does not match, `seq` values are not contiguous within
`seq_range`, an event kind is unknown, a payload version is unknown, the local session's
`owner_epoch` is greater than or equal to the bundle's, or the workspace descriptor cannot be
applied.

#### Scenario: A tampered bundle is rejected

- **WHEN** `content_hash` does not match the contents
- **THEN** the import is refused with `-32011` and the target is unchanged

#### Scenario: A gap in the log is rejected

- **WHEN** the event log skips a `seq` inside `seq_range`
- **THEN** the import is refused rather than storing a hole

#### Scenario: A stale bundle is rejected

- **WHEN** the local session's epoch is not older than the bundle's
- **THEN** the import is refused and the local session is untouched

#### Scenario: An unknown kind is never silently dropped

- **WHEN** the bundle carries an event kind this build does not know
- **THEN** the import fails rather than skipping the event

### Requirement: Workspace descriptor

A bundle SHALL carry a workspace descriptor with `kind`, `repo_id`, `head_commit`, `remote`,
`dirty_patch`, `untracked` and `toolchain_hint`. `repo_id` SHALL be sha256 of the git common
directory path and the root commit, and SHALL be used to apply a patch in place, never as
proof that two hosts hold the same repository.

#### Scenario: A matching repository is patched in place

- **WHEN** the target holds a repository whose `repo_id` matches and whose `HEAD` is the
  recorded commit or its descendant
- **THEN** the patch and untracked files are applied in place

#### Scenario: A diverged tree is refused

- **WHEN** `head_commit` is neither the target's `HEAD` nor an ancestor of it
- **THEN** the import is refused with `-32007` naming both commits

#### Scenario: An unreachable tree needs an explicit path

- **WHEN** `repo_id` does not match and `remote` is null
- **THEN** the import is refused with `-32008` unless the caller supplies a workspace whose
  root commit matches

#### Scenario: A clonable remote is cloned

- **WHEN** `repo_id` does not match and `remote` is set
- **THEN** the repository is cloned into the target directory, checked out at `head_commit`,
  and the patch and untracked files applied

#### Scenario: A patch is never fuzzed

- **WHEN** a patch does not apply cleanly
- **THEN** the import is refused rather than applied approximately

### Requirement: One repository per workspace

A workspace SHALL be exactly one git repository. A multi-repository tree SHALL be refused by
name rather than half-transferred.

#### Scenario: A multi-repo tree is refused

- **WHEN** the workspace contains more than one repository root
- **THEN** export is refused and names the trees

### Requirement: Transport is a second process

A bundle SHALL move over `ssh <host> sa handoff --receive`, streaming raw bytes on stdin,
rather than over the JSON-RPC channel. The SSH channel SHALL be a disposable carrier.

#### Scenario: The channel can drop

- **WHEN** the SSH connection drops during a transfer
- **THEN** a running turn on either host is unaffected

#### Scenario: The RPC channel stays clean

- **WHEN** a bundle is transferred
- **THEN** no binary payload is encoded into the JSON-RPC channel
