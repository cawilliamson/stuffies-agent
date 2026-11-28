# Spec Delta

## Purpose

A durable, inspectable store of what the agent has learned: markdown files in namespaced
directories, a derived search index, an ACL enforced in the dispatcher, the tools that reach
it, and an optional bounded background review.

## ADDED Requirements

### Requirement: Knowledge is markdown on disk

The knowledge root SHALL be `$XDG_STATE_HOME/stuffies-agent/knowledge/`, overridable in
`knowledge.toml`. Content SHALL be markdown files whose path is their identity, in
`shared/`, `user/<id>/` and `workspace/<id>/` namespaces, with optional YAML frontmatter
carrying `title`, `tags` and `updated`.

#### Scenario: The user can read and diff what the agent knows

- **WHEN** the root is inspected with ordinary tools
- **THEN** every entry is a markdown file with a meaningful path

#### Scenario: The root may be a repository

- **WHEN** the root is a git repository and a write happens
- **THEN** the write is committed

### Requirement: The index is derived

`knowledge.db` SHALL be a sqlite FTS5 index over `path`, `namespace`, `title` and `body`.
`knowledge.reindex` SHALL rebuild it from the files. A missing or corrupt index SHALL NOT be
an error, only a reindex, and the files SHALL win every disagreement.

#### Scenario: The index rebuilds from files

- **WHEN** the index is deleted and `knowledge.reindex` runs
- **THEN** search answers again from the files

#### Scenario: An edit on disk wins

- **WHEN** a file is edited outside the daemon
- **THEN** a reindex reflects the file's contents

#### Scenario: A corrupt index is not a failure

- **WHEN** `knowledge.db` cannot be opened
- **THEN** the store is treated as unindexed and rebuilt rather than failing

### Requirement: No embeddings

Search SHALL use FTS5 keyword recall only. Embeddings SHALL NOT be added until keyword recall
demonstrably fails, and adding them later SHALL be additive behind the same
`knowledge.search` signature.

#### Scenario: Search costs no model call

- **WHEN** `knowledge.search` runs
- **THEN** no request reaches the endpoint

### Requirement: Namespaces are resolved and recorded

A session SHALL be created with a resolved namespace set, recorded in `session.created` as
`knowledge_namespaces`, resolved from `knowledge.toml`.

#### Scenario: The set is in the log

- **WHEN** a session is created
- **THEN** `session.created` carries the namespaces the session may reach

#### Scenario: A permissive single-user deployment is not noticed

- **WHEN** `[default]` allows the shared namespace only
- **THEN** a single-user deployment works without configuration

### Requirement: Authorisation is enforced in the dispatcher

The daemon SHALL refuse any knowledge read or write outside the session's namespace set with
`-32012`, naming the namespace. The check SHALL live in the dispatcher, so every tool and
every wire method passes through it.

#### Scenario: A session cannot read another's namespace

- **WHEN** a session in namespace A reads a path in namespace B
- **THEN** it is refused with `-32012` and no content is returned

#### Scenario: The UI cannot widen access

- **WHEN** a browser requests a path outside the session's set
- **THEN** the dispatcher refuses it

### Requirement: Knowledge tools

The agent SHALL reach the store through `knowledge.list`, `knowledge.read`,
`knowledge.remove`, `knowledge.search` and `knowledge.write`, so every access it makes is a
`tool.requested`/`tool.completed` pair in the log. The same five names SHALL exist as wire
methods for the CLI and the web surface.

#### Scenario: A write is in the log

- **WHEN** the agent writes a knowledge file
- **THEN** the turn's transcript carries the tool call and its result

#### Scenario: A newer file is not overwritten

- **WHEN** `knowledge.write` targets a file whose `updated` frontmatter is newer than the one
  it read
- **THEN** the write is refused rather than silently replacing it

#### Scenario: An unwritable root does not stop a turn

- **WHEN** the root cannot be written
- **THEN** the knowledge tools fail with `-32013` and the turn continues

### Requirement: Background review is opt-in and bounded

Background review SHALL be off by default and enabled per session. When enabled it SHALL run
after every 10 completed turns and SHALL be bounded to 50 000 input tokens, low reasoning
effort, 300 seconds and 8 files written per review. It SHALL write through
`knowledge.write` under the session's namespaces and SHALL append `review.completed`.

#### Scenario: Off by default

- **WHEN** a session runs many turns without opting in
- **THEN** no review call is made

#### Scenario: The bounds are enforced

- **WHEN** a review would exceed its token, time or write bound
- **THEN** it stops at the bound and the turn is unaffected

#### Scenario: The review is recorded

- **WHEN** a review writes files
- **THEN** `review.completed` carries the turns covered, the files written and the usage

### Requirement: Knowledge is not transferred with a session

A handoff bundle SHALL carry the knowledge namespaces and a per-file hash in its manifest,
and SHALL NOT carry the knowledge files themselves. Import SHALL warn when a namespace is
absent on the target and list the missing paths, and the session SHALL continue with what the
target has.

#### Scenario: A missing namespace is reported

- **WHEN** a bundle references a namespace the target does not have
- **THEN** import warns and lists the missing paths

#### Scenario: Bundle size follows the session, not the history

- **WHEN** a session is exported from a host with a large knowledge root
- **THEN** the bundle does not grow with it
