# Tasks

## 1. Layout

- [ ] 1.1 Create the root at `$XDG_STATE_HOME/stuffies-agent/knowledge/`, overridable in
      `knowledge.toml`
- [ ] 1.2 Support `shared/`, `user/<id>/` and `workspace/<id>/` namespaces
- [ ] 1.3 Read and write optional YAML frontmatter with `title`, `tags` and `updated`
- [ ] 1.4 Commit on write when the root is a git repository

## 2. Index

- [ ] 2.1 Create `knowledge.db` with FTS5 over `path`, `namespace`, `title` and `body`
- [ ] 2.2 Implement `knowledge.reindex` as a full rebuild from the root
- [ ] 2.3 Treat a missing or corrupt index as a reindex, never an error
- [ ] 2.4 Implement `knowledge.search` with a limit and an optional namespace filter

## 3. Namespaces and ACL

- [ ] 3.1 Resolve a session's namespace set at creation from `knowledge.toml` and record it
      in `session.created`
- [ ] 3.2 Enforce the set in the dispatcher for every read and write, refusing with `-32012`
      and naming the namespace
- [ ] 3.3 Leave a single-user `[default]` permissive so it is never noticed
- [ ] 3.4 Return `-32013` from the knowledge tools when the root is unwritable, without
      failing the turn

## 4. Tools and methods

- [ ] 4.1 Add `knowledge.list`, `knowledge.read`, `knowledge.remove`, `knowledge.search` and
      `knowledge.write` as built-in tools
- [ ] 4.2 Add the wire methods with the same names for the CLI and web surface
- [ ] 4.3 Refuse a `knowledge.write` that would overwrite a newer `updated` frontmatter
- [ ] 4.4 Record every access as `tool.requested`/`tool.completed` when it came from a turn

## 5. Background review

- [ ] 5.1 Add the per-session opt-in
- [ ] 5.2 Run after every 10 completed turns, bounded to 50 000 input tokens, low effort,
      300 seconds and 8 files
- [ ] 5.3 Write through `knowledge.write` under the session's namespaces
- [ ] 5.4 Append `review.completed` with the turns covered, files written and usage
- [ ] 5.5 Skip and log when the bounds are exceeded, leaving the turn unaffected

## 6. Surfaces

- [ ] 6.1 Add `sa knowledge` for listing, reading and reindexing
- [ ] 6.2 Add a knowledge browser to the web surface

## 7. Verification

- [ ] 7.1 Assert knowledge tools round-trip through the ACL
- [ ] 7.2 Assert a session in one namespace cannot read another's
- [ ] 7.3 Assert `knowledge.reindex` rebuilds from files after the index is deleted
- [ ] 7.4 Assert a review respects its token, time and write bounds
- [ ] 7.5 Assert a knowledge write appears in the event log
- [ ] 7.6 Assert an unwritable root fails the knowledge tools and not the turn
