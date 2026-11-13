# sa — specification

Status: design, pre-implementation. This document supersedes `PLAN.md` (deleted with the
v0.3 codebase). Nothing here is implemented yet.

British English throughout. `DECIDED` marks a settled decision; `OPEN` marks something that
must be answered before the milestone that depends on it.

---

## 1. Purpose

`sa` is a Rust-native coding agent whose runtime is a **persistent daemon**. Every
interactive surface — terminal UI, one-shot CLI, scripted automation — is a disposable
client that attaches to the daemon, subscribes to its event stream, and can detach at any
time without interrupting work.

The daemon runs either on the workstation or on a remote host. A session started on a
laptop can be handed off to a remote daemon, run unattended overnight, and handed back the
following morning with its history intact.

### 1.1 Motivating workflow

The workflow this design exists to serve:

1. Start work on the laptop with a local daemon.
2. Hand the session to a remote daemon before shutting the laptop down.
3. The remote daemon continues the work unattended, under a restricted autonomy policy.
4. In the morning, attach to the remote daemon and read what happened.
5. Hand the session back to the laptop and continue locally.

Everything in this specification is justified by that workflow. Features that do not serve
it are deferred.

### 1.2 Non-goals

- A plugin/extension host. `sa` is native code; there is no third-party extension API.
- Compatibility with pi's config layout. `sa` keeps its own naming (`SA_*`, `.sa/`,
  `SOUL.md`/`AGENTS.md`).
- Multi-user operation. The daemon is single-user and trusts its own unix user.
- Subagents, MCP, browser tooling, telemetry. Out of scope until the ladder in §13 is
  complete.

---

## 2. Architecture

### 2.1 Crates

```text
sa-proto    wire types and protocol constants. serde only; no core, no tokio, no terminal.
sa-core     sessions, event log, llm client, tools, policy. no terminal, no stdout.
sa-daemon   unix socket server, session manager, supervision, dispatcher host.
sa-client   typed client library: attach, subscribe, replay, send, approve.
sa-tui      ratatui front end. view only; holds no authoritative state.
sa          binary: `sa attach`, `sa run`, `sa rpc --stdio`, `sa daemon`, `sa handoff`.
```

### 2.2 Dependency rules

These are enforced by a CI check, not by convention.

- `sa-proto` depends on `serde`/`serde_json` and nothing else in the workspace.
- `sa-core` must not depend on `crossterm`, `ratatui`, or write to stdout. The test is
  `cargo tree -p sa-core` containing no terminal crate.
- `sa-tui` must not depend on `sa-daemon`. It talks to `sa-client`.
- `sa-daemon` must not depend on `sa-tui`.
- The agent loop must be runnable in-process for tests without a socket, a terminal, or a
  real model endpoint.

### 2.3 Dispatcher

One `Dispatcher` in `sa-core` implements every protocol method. Transports are carriers
around it and contain no protocol logic. Adding a transport must not add a code path in the
dispatcher; adding a method must not require touching any transport.

### 2.4 State ownership

The daemon is the only writer of session state. Clients hold a cache and a cursor, never
authority. A client that has been disconnected for a week resumes by requesting events
after its last acknowledged `seq`.

---

## 3. Protocol

### 3.1 Framing

JSON-RPC 2.0, newline-delimited UTF-8 JSON, one message per line. Requests carry an `id`;
notifications do not. Maximum line length is bounded (1 MiB) and oversized lines are a
protocol error, not a truncated read.

The first message on every connection is a `hello` handshake carrying the protocol
version. Version mismatch is a hard failure with an explicit error naming both versions.
There is no negotiation and no best-effort compatibility.

`DECIDED`: protocol version is a single integer, bumped on any breaking change.

### 3.2 Methods

Session lifecycle:

```text
session.create   {cwd, autonomy, title?}          -> {session_id, seq}
session.list     {}                               -> [{id, title, state, owner, updated_at}]
session.attach   {session_id, from_seq}           -> {session, events[], more}
session.detach   {session_id}                     -> {}
session.rename   {session_id, title}              -> {}
session.fork     {session_id, from_seq, title?}   -> {session_id}
session.delete   {session_id}                     -> {}
```

Turns:

```text
turn.start       {session_id, prompt, detach?}    -> {turn_id, seq}
turn.cancel      {session_id, turn_id}            -> {}
turn.status      {session_id}                     -> {turn_id?, state}
```

Events:

```text
events.subscribe {session_id, from_seq, filter}   -> {subscription_id}
events.unsubscribe {subscription_id}              -> {}
events.replay    {session_id, from_seq, to_seq}   -> {events[]}
```

Approvals:

```text
approval.list    {session_id}                     -> [{approval_id, class, detail, requested_at}]
approval.respond {approval_id, decision, scope}   -> {}
```

Handoff:

```text
handoff.export   {session_id, mode}               -> {bundle_path, bytes, epoch}
handoff.import   {bundle_path, verify_only?}      -> {session_id, epoch}
handoff.accept   {session_id, epoch}              -> {session_id}
```

Administration:

```text
ping             {}                               -> {version, uptime, sessions}
model.list       {}                               -> [{id, context_limit, capabilities}]
model.select     {session_id, model}              -> {}
```

`DECIDED`: method names are stable API. Renaming one is a protocol version bump.

### 3.3 Notifications

Server to client, never acknowledged:

```text
event            {session_id, seq, kind, payload}
gap              {session_id, from_seq, to_seq}   -- client fell behind, re-sync required
approval.request {approval_id, session_id, class, detail}
turn.state       {session_id, turn_id, state}
log              {level, message}                 -- daemon diagnostics
```

`DECIDED`: a client that cannot keep up receives `gap` and is dropped from the live
subscription; it re-syncs with `events.replay`. A slow client must never apply backpressure
to the agent loop.

### 3.4 Per-client filtering

Subscriptions carry a filter so that reasoning traces, tool output verbosity, and cost
events can differ per client without changing what is persisted. Filtering is applied
server-side at send time. The log is always complete; the filter only affects delivery.

---

## 4. Transports

### 4.1 Unix socket (local, primary)

`$XDG_RUNTIME_DIR/sa/daemon.sock`, socket mode `0600`, containing directory `0700`. Peer
credentials are checked with `SO_PEERCRED` and must match the daemon's own uid.

### 4.2 SSH stdio (remote)

The remote daemon is reached as:

```text
ssh <host> sa rpc --stdio
```

`sa rpc --stdio` on the remote host is a **proxy** that connects to the long-lived daemon
over its unix socket and relays frames to stdin/stdout. It is not the daemon. If the daemon
were a child of the SSH session, closing the laptop would kill overnight work.

Consequences, all of which are requirements:

- The SSH channel is a disposable carrier. Dropping it must not affect a running turn.
- No listening port, no TLS, no token store. SSH authentication and host keys are the
  trust boundary.
- The daemon's lifetime is owned by systemd (§11), never by a client connection.

`DECIDED`: SSH stdio is the remote transport. HTTP is opt-in and not required for the
motivating workflow.

### 4.3 HTTP (opt-in, later)

If a browser or non-Rust client is ever needed: `POST /rpc` and `GET /events` (SSE) over
the same dispatcher, bound to loopback only, with a token file at mode `0600`. This is a
milestone-8 concern and may be dropped entirely if no client needs it.

---

## 5. Session model

### 5.1 Event sourcing

A session is an append-only, ordered log of events with a per-session monotonic `seq`
starting at 1. All state is derived from the log. There is no mutable session state that is
not reconstructible from events.

Initial event kinds:

```text
session.created     session.renamed      session.state
turn.started        turn.completed       turn.interrupted
llm.delta           llm.reasoning        llm.completed
tool.requested      tool.completed       tool.failed
approval.requested  approval.resolved
model.changed       checkpoint
handoff.exported    handoff.imported
compaction.applied  error
```

Event payloads are versioned and must not encode storage-specific details. A payload must
be readable by an importer that has never seen the sqlite schema.

`DECIDED`: adding an event kind is a minor version bump; changing an existing payload's
shape is a major one.

### 5.2 Ownership and fencing

Each session records an owner:

```text
owner = { host_fingerprint, epoch }
```

`epoch` increments on every successful handoff. Fencing rules:

- A daemon accepts `turn.start` only if the session's `owner_epoch` equals the epoch it
  holds. Otherwise it rejects with a stale-owner error naming both epochs.
- Fencing uses the epoch counter, never wall-clock time. Clock skew between laptop and
  server must not be load-bearing.
- `host_fingerprint` is a stable local identifier, not a hostname and not a network
  address.

### 5.3 Session states

```text
active        writable by its owner
quiescing     draining an in-flight turn before handoff
relocating    exported, awaiting acceptance on the target
relocated     moved away; read-only tombstone on the source host
interrupted   a turn died with the daemon; resumable, no automatic retry
```

`relocated` sessions remain listed on the source host so the user can see where a session
went, and can be revived by handing them back.

### 5.4 Handoff: move versus fork

Two explicit modes, chosen at export time. There is no implicit behaviour.

- **move** — the intended overnight workflow. Source quiesces at a turn boundary, writes a
  `checkpoint`, exports the bundle, increments the epoch, transitions to `relocated`, and
  refuses further turns. Target imports and becomes owner at the new epoch. History stays
  linear. The source cannot write even if it wakes up believing it is still the owner.
- **fork** — the target receives a copy with `parent_id` set to the source session and its
  own epoch lineage. Both remain writable and diverge deliberately. There is no merge.

`DECIDED`: transfer is a move by default. Divergence requires asking for a fork.

### 5.5 Crash recovery

On startup the daemon scans for sessions with an open turn and appends `turn.interrupted`.
It does not retry tool calls, because tool side effects are not idempotent. A client
attaching to an interrupted session sees the interruption and may re-drive the turn
explicitly.

---

## 6. Persistence

### 6.1 Live store

`DECIDED`: sqlite in WAL mode at `.sa/sa.db` is the sole source of truth for live sessions.

```text
events(id INTEGER PRIMARY KEY, session_id TEXT, seq INTEGER, kind TEXT,
       payload TEXT, created_at INTEGER, UNIQUE(session_id, seq))
sessions(id TEXT PRIMARY KEY, title TEXT, parent_id TEXT, state TEXT,
         owner_host TEXT, owner_epoch INTEGER, cwd TEXT, repo_remote TEXT,
         autonomy TEXT, created_at INTEGER, updated_at INTEGER)
approvals(id TEXT PRIMARY KEY, session_id TEXT, seq INTEGER, state TEXT,
          request TEXT, resolution TEXT, requested_at INTEGER, resolved_at INTEGER)
schema_version(version INTEGER)
```

Rationale: a daemon with several sessions writing concurrently needs atomic commit and
crash safety. An append-only text file needs hand-rolled locking and torn-tail repair; a
`kill -9` mid-tool-call must not require a repair pass.

`OPEN`: whether `.sa/sa.db*` is committed to the repository or ignored. Default assumption
is ignored, with `export` producing the durable, shareable artefact.

### 6.2 Canonical bundle (handoff format)

A handoff is a single container file, written atomically (temp file plus rename):

```text
manifest.json     format_version, session_id, owner_epoch, seq_range,
                  content_hash, created_at, source_host, workspace, autonomy
events.jsonl      the full event log, one JSON object per line, in seq order
workspace.patch   unified diff of the dirty worktree
untracked/        contents of untracked files, size-capped
```

`DECIDED`: sqlite is authoritative at rest; the bundle is authoritative in transit. There is
exactly one write path for each, and no format is a second live store.

### 6.3 Import verification

Import is verified before any mutation. Rejection is atomic and leaves the target daemon
unchanged. Reject when:

- `format_version` is unsupported;
- `content_hash` does not match;
- `seq` values are not contiguous within `seq_range`;
- an event kind is unknown (hard error, never silently dropped);
- the session id already exists locally with an `owner_epoch` greater than or equal to the
  bundle's (stale bundle);
- the workspace descriptor cannot be applied (§7).

### 6.4 Derived exports

`sa export --format jsonl|markdown` produces human-readable artefacts for review and
diffing. Derived only. Never read back as authoritative state.

---

## 7. Workspace handling

A session is meaningless without the tree it was editing. The bundle therefore carries a
workspace descriptor, not a filesystem image:

```text
repo_remote       origin URL
head_commit       commit the session was based on
dirty_patch       unified diff of tracked modifications
untracked         file list with contents, size-capped
toolchain_hint    presence of flake.nix / shell.nix / rust-toolchain.toml
```

`OPEN`: this assumes a single git repository with a reachable remote as the contract for
workspace identity. Multi-repo workspaces, and repositories with no remote, need an answer
before milestone 7.

Hard rules:

- Refuse the handoff loudly if the tree contains state the descriptor cannot capture —
  submodules, oversized binaries, or referenced files under `.gitignore`. Silent divergence
  costs a day; a refusal costs a minute.
- Secrets never enter the bundle. Credentials are daemon-local (§10).
- A bundle is data, never code. Import never executes anything from it.

---

## 8. Autonomy and approvals

### 8.1 Levels

Per session, recorded in the session row and in the bundle:

```text
read-only     inspect and report; no writes, no commands
allowlisted   writes inside the workspace, allowlisted commands, no network
full          unrestricted, including network and destructive commands
```

`DECIDED`: unattended handoff defaults to `allowlisted`. `full` while detached requires
explicit opt-in at handoff time.

### 8.2 Approval routing

`PermissionClass` is enforced by the daemon, not the UI. When an operation needs approval:

- attached client(s) receive `approval.request` and may answer;
- with no client attached, the request is queued and the turn blocks until the configured
  timeout, then resolves as denied and the turn fails with an explicit reason;
- queued approvals surface in `approval.list` on the next attach.

`DECIDED`: deny on timeout. Never auto-approve because nobody was watching.

### 8.3 Unattended safety net

Before any unattended turn, the daemon creates a snapshot commit (or a named stash) in the
workspace and records its id in a `checkpoint` event. Recovery is `git reset` to that
checkpoint. This is what makes `allowlisted` a real guarantee rather than a promise.

---

## 9. Tools

Initial set, all executed in the daemon process with the session's `cwd`:

```text
read  write  edit  bash  grep  find  ls
```

Rules:

- Paths are canonicalised and must resolve inside the session workspace. Escapes are
  errors, not clamps.
- Every tool declares a `PermissionClass`; the daemon enforces it per §8.
- Tool output is bounded; truncation is recorded in the event so a client knows it saw a
  partial result.
- Tools are async and cancellable. `turn.cancel` drops in-flight tool futures.

`OPEN`: whether tool calls run in parallel within a turn. Default assumption is sequential
until the loop is stable.

---

## 10. Credentials

- The daemon owns model credentials. Clients never see a key.
- Configuration is daemon-local: environment (`OPENAI_API_TOKEN`, `OPENAI_API_ENDPOINT`) and
  a credential file under `.sa/` at mode `0600`.
- Bundles carry no credentials. A handoff to a host without credentials fails at import
  verification, not mid-turn.
- Model endpoints are OpenAI-compatible. Native Anthropic/Google wire protocols are `OPEN`.

---

## 11. Lifecycle and supervision

- systemd user unit `sa-daemon.service`, `Restart=on-failure`, `RuntimeDirectory=sa` with
  mode `0700`.
- `sa` auto-spawns the daemon when the socket is absent, guarded by an exclusive bind lock
  so auto-spawn and systemd can never both start a daemon.
- The daemon never exits because its last client disconnected.
- Graceful shutdown drains in-flight turns or marks them `interrupted`; it never truncates
  the log.

---

## 12. Failure modes

Each of these has a required behaviour and a test in the milestone that introduces it.

| Failure | Required behaviour |
| --- | --- |
| daemon killed mid-turn | on restart, append `turn.interrupted`; no tool retry; client sees the interruption |
| SSH carrier drops | daemon and turn unaffected; client reconnects and replays from its cursor |
| client stalls | bounded queue, `gap` notification, client dropped from live subscription; agent unaffected |
| stale bundle (old epoch) | import rejected, target unchanged |
| two hosts claim the session | lower epoch loses; its writes are rejected, not merged |
| disk full | turn aborts inside a transaction; log stays consistent |
| approval timeout while detached | denied, recorded, turn fails with reason |
| model endpoint unavailable | turn fails, recorded, session remains resumable |
| partial bundle write | impossible: temp file plus rename, verified by hash before acceptance |
| unknown event kind on import | hard rejection, no partial import |

---

## 13. Milestones

Each milestone is independently testable and leaves the tree green.

**M1 — daemon skeleton.** `sa-proto` plus a daemon serving `ping` and `hello` over the unix
socket, with `sa rpc --stdio` proxying to it. No LLM, no database.
Acceptance: an integration test drives the socket with a raw client; a second test drives
the same methods through the stdio proxy; socket permissions are asserted.

**M2 — event log and replay.** sqlite schema, event append, `events.replay`, session
create/list, `kill -9` recovery.
Acceptance: crash test kills the daemon mid-append and asserts a consistent log on restart;
replay from an arbitrary cursor returns exactly the later events.

**M3 — llm and one-shot turns.** OpenAI-compatible client, streaming deltas into the log,
`sa run -p` one-shot client.
Acceptance: a recorded-cassette test produces a completed turn with a contiguous event log;
no network is touched in tests.

**M4 — tools, policy, detached turns.** Tool execution, approval events, autonomy levels,
`turn.start {detach:true}` surviving client disconnect.
Acceptance: a detached turn completes with no client attached; a `read-only` session
performs zero writes; a denied approval is recorded and fails the turn.

**M5 — TUI.** Attach, live subscription, reconnect and replay, markdown rendering, editor.
Acceptance: a render smoke test; a test that detaching and reattaching mid-turn reproduces
the transcript from the cursor.

**M6 — handoff.** Bundle export/import, verification, epoch fencing, `move` and `fork`.
Acceptance: round-trip a session between two in-process daemons; assert the source refuses
writes after `move`; assert a stale bundle is rejected without mutation; assert `fork`
produces a `parent_id`.

**M7 — workspace transfer.** Descriptor, patch capture, applicability checks, refusal paths.
Acceptance: a dirty worktree transfers and applies cleanly; a worktree with a submodule is
refused with a specific error.

**M8 — packaging.** systemd unit, Nix module, auto-spawn lock, optional HTTP listener.
Acceptance: daemon survives a client kill and a service restart; auto-spawn and systemd
cannot race.

---

## 14. Engineering constraints

- Rust, edition 2021, workspace lints as configured in `Cargo.toml`: `unwrap_used` denied,
  `print_stdout`/`print_stderr` denied outside binaries, pedantic allowed.
- No language-level package managers. Dependencies are declared in `Cargo.toml` and
  provided through nixpkgs.
- Pre-commit gates are mandatory: `cargo fmt --check`, `cargo clippy --all-targets
  --all-features -- -D warnings`, `cargo test --workspace`, `cargo deny check`.
- No `unsafe`.
- Tests are behaviour-named (`kills_daemon_mid_turn_and_recovers`), not
  `test_recovery_2`.

---

## 15. Open questions

These need answers before the milestone that depends on them.

1. **Workspace contract** (M7): is a single git repo with a reachable remote the real
   contract? What about multiple repos in one workspace, or a repo with no remote?
2. **Remote toolchain** (M7): the remote needs `cargo`/`nix` for tests to be meaningful. Is
   `nix develop` integration in scope, or is a pre-provisioned host assumed?
3. **Multi-client sessions** (M5): can two clients attach to one session at once (laptop
   and phone)? Affects approval routing and presence display.
4. **Parallel tool calls** (M4): sequential is the default. Is parallel wanted?
5. **HTTP client** (M8): is there any concrete non-Rust client? If not, drop HTTP entirely.
6. **Model protocols** (M3): OpenAI-compatible only, or native Anthropic/Google messages?
7. **Session deletion** (M2): hard delete, or tombstone so a handoff can still be revived?
