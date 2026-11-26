# Proposal

## Why

The daemon is meant to serve two workloads: coding, which is built, and knowledge, which is
not. Knowledge is the durable memory the daemon keeps across sessions — what the agent has
learned, in files the user can read, edit, diff and back up.

Today `session.created` records a hardcoded `knowledge_namespaces: ["shared"]` and no
knowledge method exists on the wire. Nothing is stored, indexed or searchable.

## What Changes

- A knowledge root under the state directory, holding markdown files in `shared/`,
  `user/<id>/` and `workspace/<id>/` namespaces, with optional frontmatter.
- A derived sqlite FTS5 index over path, namespace, title and body, rebuildable at will, with
  the files authoritative in every disagreement.
- Namespace resolution at session creation from `knowledge.toml`, recorded in
  `session.created`, and an ACL enforced in the dispatcher.
- The knowledge tools the agent uses: `knowledge.list`, `knowledge.read`,
  `knowledge.remove`, `knowledge.search`, `knowledge.write`.
- Wire methods for the web surface and the CLI to browse the same store without a turn.
- An opt-in, budgeted background review that writes through the same tools.
- A knowledge browser in the web surface.

Out of scope: embeddings (FTS5 only until keyword recall demonstrably fails); shipping the
knowledge root inside a handoff bundle (a bundle carries a session, not the whole brain);
per-tenant processes (one daemon, isolation by namespace).

## Capabilities

### New Capabilities

- `knowledge-store`: the on-disk layout, the derived index, namespace resolution and the
  ACL that the dispatcher enforces, the knowledge tools and the optional background review.

### Modified Capabilities

- `session-lifecycle`: `session.created` records the resolved namespace set rather than a
  fixed list.
- `protocol`: the `knowledge.*` methods join the method table.
- `tools`: the knowledge tools join the built-in set.
- `model-provider`: a background review is a second model call whose usage is recorded.

## Impact

- New `knowledge.db` and `knowledge/` under the state directory.
- New `knowledge.toml` under the config directory, optional and permissive by default.
- `knowledge_denied` (`-32012`) and `knowledge_unavailable` (`-32013`) become reachable.
- `review.completed` becomes the second event kind added since v1.0.
- `sa knowledge` joins the CLI, and `session.created` changes shape for every existing log
  reader that assumed the fixed list.
