# design

Why `sa` is shaped the way it is. Behaviour lives in `specs/`, and its scenarios are executed
by `cargo test`; this file holds the motivation, the alternatives that were rejected, and the
decisions already settled. Where the two disagree, the specs are right.

## the build goal: lean on the modules we already depend on

This is a defining goal of the project, not a style preference. The project is pre-release
and in early testing, so nothing is kept for compatibility, nothing is carried "for later",
and no shim exists to protect a decision that has not shipped.

- **lean into the dependencies.** `rig-core` and `rig-rmcp` are in the tree; the agent loop,
  message model, conversation history, tool plumbing and provider wire should use what they
  already implement rather than a hand-rolled equivalent beside it. The same holds for
  `ratatui`, `crossterm`, `edtui` and every other dependency: if it ships the behaviour, our
  version of that behaviour is a bug waiting to happen.
- **our code is the difference, not the plumbing.** Anything a dependency can do correctly
  is not our job. What stays ours is what nothing else knows: the event log, the daemon, the
  turn state machine, the UI.
- **delete before adding.** A second message type, a conversion layer between two shapes we
  control, a helper that exists because a dependency's type was inconvenient: remove it
  rather than maintain it.
- **no legacy accommodation.** No dual code paths, no deprecated aliases, no migration
  shims. Change the thing and fix the call sites.

## the workflow this exists for

1. Start work on the laptop with a local daemon.
2. Hand the session to a remote daemon before shutting the laptop down.
3. The remote daemon continues the work unattended.
4. In the morning, attach to the remote daemon and read what happened.
5. Hand the session back to the laptop and continue locally.

Every feature is justified by that loop. A feature that does not serve it is deferred, and
the ones that serve it are the ones in the specs.

There are two workloads and they are served by different surfaces. Coding is the local,
interactive, high-bandwidth loop. Knowledge is the low-bandwidth, asynchronous loop. They
share one daemon because both need the same things: a durable event log, tool execution, and
the ability to keep working after the client goes away.

## what is deliberately not built

- A plugin host. `sa` is native code and there is no third-party extension API.
- Writing anything into the workspace. See the trace boundary below; this is absolute.
- Compatibility with another agent's config layout. `sa` keeps its own root, though it
  deploys per-project context with the same directory-naming scheme so one dotfiles module
  serves both.
- Multi-tenant processes. One daemon serves one unix user; several people are isolated by
  knowledge namespace, not by container.
- A permission system.

## the trace boundary

A turn leaves no trace in the directory it works on. Everything the agent writes for its own
bookkeeping lives under `$XDG_STATE_HOME/stuffies-agent` and `$XDG_RUNTIME_DIR`. The only
things that appear in a workspace are the changes the turn was asked to make.

The line is between what the agent writes and what the user's own commands write. Build
output from a command a turn runs belongs to the toolchain that produced it, and a project's
ignore file already covers it. What the agent must never do is add a cache, a marker or a
directory of its own.

This is why there is no permission system and why the one guard that remains is a checkpoint:
the user's defence against a bad turn is that their directory is exactly as they left it,
plus the edits they wanted. A recovery point that lives in their `.git` is not that.

### why not the workspace's `.git`

A ref and its objects there are a trace in the user's repository: they survive pruning until
a `git gc`, they grow the object store on every detached turn, and they make `git fsck` and
`git log --all` say things about `sa`. A shadow store is one directory, removed with `rm -rf`.

### why not `git stash`

`git stash push` cleans the worktree, which is the opposite of what a checkpoint wants, and
it mutates a shared list. `git stash create` avoids the list but silently ignores untracked
files, which is exactly what an agent writes. A commit over a tree built from the worktree
captures tracked and untracked files, changes nothing in place, and is removed by deleting a
ref.

### why `gix` and not the git binary

This was the git binary, for one reason: git decides what a worktree holds — ignore rules,
file modes, symlinks — and a checkpoint's only job is to record exactly that, so a second
implementation of those rules risks a checkpoint that does not restore what the user had.

Two things changed the answer. The binary is a runtime dependency the daemon is not supposed
to have, and it costs a process per snapshot. And the risk that motivated it is now covered by
the tests rather than by the choice of implementation: the scenarios read every checkpoint back
with real git (`git log`, `git ls-tree`, `git rev-list`), so a store gix writes and git cannot
read is a red build rather than a surprise at restore time.

The rules are not re-derived. The walk uses the `ignore` crate, which is git's ignore
semantics as a library, and the tree is written in git's own object format and sorted the way
git sorts it. What stays ours is the snapshot walk, the commit, the refs and the store's
bookkeeping — gix does not collect, so unreachable objects are pruned by walking `objects/`,
which is safe because the store is ours and is never packed.

## why no permission system

No autonomy levels, no allowlist, no policy file, no approval prompts. A tool runs, or it
fails because the path escaped the workspace. The earlier design had an autonomy field, an
allowlist and an interactive approval step; all three were removed because each was a second
place to state what the checkpoint already protects.

## why one dispatcher

One `Dispatcher` implements every protocol method. Transports are carriers around it and
contain no protocol logic. Adding a transport must not add a code path in the dispatcher;
adding a method must not require touching a transport. That is what keeps a unix socket, a
stdio proxy, an in-process pair and an HTTP carrier from drifting apart.

## why sqlite at rest and a bundle in transit

A daemon with several sessions writing concurrently needs atomic commit and crash safety. An
append-only text file needs hand-rolled locking and torn-tail repair, and a `kill -9`
mid-tool-call must not require a repair pass. The bundle is the transit format and is written
atomically, so there is exactly one write path for each and neither is a second live store.

## why the epoch and not a clock

Fencing uses an integer counter, never wall-clock time. Clock skew between a laptop and a
server must not be load-bearing, and a lease that expires is a lease that can expire early.

## why lean on the dependencies

`rig-core`, `rig-rmcp`, `lspf`, `ratatui` and the rest already implement the protocol, the
process supervision and the layout, and where a program owns a format — `git` for the
checkpoint store — the program is what we call rather than
reimplement. Anything a dependency does correctly is not our job. What stays ours is what
nothing else knows: the event log, the daemon, the turn state machine, the checkpoint store's
placement, and the UI.

## rejected alternatives, by decision

| decision | alternative considered | why rejected |
| --- | --- | --- |
| checkpoint in a shadow store | private refs in the workspace's `.git` | a trace in the user's repository |
| checkpoint by tree snapshot | `git stash push` / `git stash create` | cleans the worktree; ignores untracked files |
| checkpoints written by `git` | `gix` writing the object format ourselves | a second implementation of what a worktree holds |
| move by default, fork on request | one mode with a `--fork` flag | an accidental fork diverges silently |
| epoch counter for fencing | lease timestamps | clock skew must not be load-bearing |
| bundle as one file | tar stream over the JSON-RPC channel | would need base64 and a framing exception |
| dedicated `ssh ... sa handoff --receive` | binary frames on the RPC channel | keeps one framing rule for the channel |
| SSH stdio for remote | a listening port with TLS and a token | SSH is already the trust boundary |
| HTTP served by the daemon | a separate gateway process | HTTP is a passive carrier; a gateway widens the failure domain |
| SSE with `Last-Event-ID` | websockets | would duplicate a reconnect protocol the daemon has |
| knowledge as markdown files | rows in a database | an opaque store nobody can diff or prune |
| FTS5 only | embeddings | no model call, adequate until it demonstrably is not |
| namespace ACL in the dispatcher | a container per tenant | one process to supervise instead of three |
| background review opt-in and bounded | on by default, unbounded | an unbounded review parked a session behind a decode |
| single-line composer plus `$EDITOR` | an in-terminal editor | an editor is a project of its own |
| sequential tool calls | parallel calls | revisited only if a measured turn is latency-bound |
| no theming layer | a pluggable theme registry | layout is code until there is a reason for it not to be |

## settled questions

- **Remote toolchain.** A pre-provisioned host is the contract. `nix develop` integration is
  deferred past v5.0; `toolchain_hint` is recorded for diagnostics only, and import warns when
  the hint names a tool the target lacks. The daemon is not a build orchestrator.
- **Checkpoint retention.** Ten per session, pruned automatically, and anything older than
  seven days deleted with its objects. A checkpoint nothing will restore is a leak.
- **Bundle transport.** A separate `ssh <host> sa handoff --receive` invocation streaming raw
  bytes, not the JSON-RPC channel.
- **Web surface.** Served by the daemon over loopback HTTP with token plus header
  authentication, TLS terminated by a reverse proxy.
- **Knowledge isolation.** A namespace ACL enforced in the dispatcher.

## still open

- Whether a `--with-knowledge` flag that ships the referenced knowledge files in a bundle is
  wanted. Not required for the motivating workflow.

## release order

The specs describe behaviour; the order the surfaces are wanted is history rather than a
requirement. v1.0 through v1.3 are built: the daemon, the event log, the model client, the
tools, the checkpoint store, goal mode, the line client and the terminal front end. v2.0 (web
surface), v3.0 (knowledge) and v4.0 (remote handoff) are the open changes under
`docs/changes/`. v5.0 is packaging: the systemd unit, a Nix module, auto-spawn under a
bind lock, blob garbage collection and an optional `session.purge`.
