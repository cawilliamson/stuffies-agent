# Design

## Context

The daemon already has one store (sqlite, authoritative) and one rule about the workspace:
nothing of ours is written into it. Knowledge is a second store, and the temptation is to
build it like the first. The reason not to is stated by the requirement that creates it: the
user must be able to read, edit, diff and back up what the agent knows.

The knowledge tools must reach the store the same way every other tool reaches the world, so
that every access is already a `tool.requested`/`tool.completed` pair in the event log.

## Goals / Non-Goals

**Goals:**

- Markdown on disk, with the path as the identity, so the store is inspectable and portable.
- An index that is derived, rebuildable, and never the source of truth.
- Isolation enforced in the dispatcher, so one daemon serves several people without several
  processes.
- A review whose cost is bounded and chosen by the user, not by surprise.

**Non-Goals:**

- Embeddings. FTS5 costs no model call and keyword recall over agent-written headings is
  adequate until it demonstrably is not.
- A database of rows instead of files.
- Transferring knowledge with a session.
- A second tool registry: knowledge tools are built-in tools.

## Decisions

**Files are authoritative; the index is derived.** `knowledge.reindex` walks the root and
rebuilds `knowledge.db`. A missing or corrupt index is never an error, only a reindex. Files
win every disagreement. Alternative considered: rows in the session database. Rejected
because an opaque store becomes a liability nobody prunes.

**The path is the identity.** `knowledge/<namespace>/<name>.md`, with optional YAML
frontmatter carrying `title`, `tags` and `updated`. The root may itself be a git repository,
and `sa knowledge` commits on write when it is.

**Namespaces are resolved at session creation and recorded in the log.** A session carries
`knowledge_namespaces`, resolved from `knowledge.toml`. The daemon refuses any read or write
outside that set with `knowledge_denied`, naming the namespace. This is what replaces a
container per tenant: one daemon, one supervision surface, isolation in the dispatcher. A
single-user deployment leaves `[default]` permissive and never notices.

**Writes are recorded, not silently overwritten.** `knowledge.write` refuses a path outside
the session's namespaces, and refuses to overwrite a file whose `updated` frontmatter is newer
than the one it read.

**Background review is off by default and opt-in per session.** It is a second model call per
batch of turns and the budget must be the user's choice. When enabled it runs after every 10
completed turns, bounded to 50 000 input tokens, low effort, 300 seconds and 8 files per
review. The bounds come from the measured configuration this replaces, where an unbounded
review parked a session behind a multi-minute decode.

**`review.completed` is the only new event kind.** Searches and reads are already
`tool.requested`/`tool.completed` pairs, so nothing else needs a kind.

**Knowledge is not in the bundle.** A handoff records the namespaces and a per-file hash;
import warns when a namespace is absent on the target and lists the missing paths. Whether a
`--with-knowledge` flag is wanted remains open.

## Risks / Trade-offs

- **A namespace ACL is a check in one place.** A bug there is a leak, so the check lives in
  the dispatcher and every tool path goes through it.
- **FTS5 recall is keyword-based.** A question phrased unlike the file's headings will miss.
  The answer is better headings and, later, embeddings behind the same signature.
- **A review is a second call.** It is bounded and opt-in precisely because an unbounded one
  is what made the previous implementation unusable.
- **The root is user-writable.** An edit on disk wins over the index, which is the intended
  behaviour and also means a half-written file is briefly what search returns.
