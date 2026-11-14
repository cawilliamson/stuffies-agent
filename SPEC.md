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

There are two workloads, and they are served by different surfaces:

- **Coding.** A line-oriented CLI drives an agent against a working tree, the way `pi` does.
  This is the local, interactive, high-bandwidth loop, and it is served by `sa attach` and
  `sa run`.
- **Knowledge.** A web surface holds durable memory, skills and reference material, and
  answers questions against it. This is the low-bandwidth, asynchronous loop, and it is
  served by the embedded web interface (§4.3) over the knowledge store (§11).

Both share one daemon because both need the same things: a durable event log, tool execution
under a policy, approvals, and the ability to keep working after the client goes away.

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
- Writing anything into the workspace. See §6.1; this is absolute.
- Compatibility with pi's config layout. `sa` keeps its own root (`$XDG_CONFIG_HOME/sa`),
  though it deploys per-project context with the same directory-naming scheme so one
  dotfiles module can serve both agents.
- Multi-tenant deployment. One daemon serves one unix user. Several *people* may share it
  through the web surface, isolated by knowledge namespaces (§11.3), not by separate
  processes or containers.
- Concurrent clients attached to one session (§3.6).
- A terminal UI framework. The CLI is a line-oriented client, not a ratatui application; a
  full TUI is deferred indefinitely (§14).
- Subagents, MCP, telemetry. Out of scope until the release ladder in §14 is complete.

---

## 2. Architecture

### 2.1 Crates

```text
sa-proto    wire types and protocol constants. serde only; no core, no tokio, no terminal.
sa-core     sessions, event log, llm client, tools, policy. no terminal, no stdout.
sa-daemon   unix socket server, session manager, supervision, dispatcher host.
sa-client   typed client library: attach, subscribe, replay, send, approve.
sa-web      http carrier, auth, embedded ui assets. no daemon, no terminal.
sa          binary: `sa attach`, `sa run`, `sa rpc --stdio`, `sa daemon`, `sa handoff`,
            `sa init`, `sa export`, `sa knowledge`.
```

### 2.2 Dependency rules

These are enforced by a CI check, not by convention.

- `sa-proto` depends on `serde`/`serde_json` and nothing else in the workspace.
- `sa-core` must not depend on `crossterm`, `ratatui`, or write to stdout. The test is
  `cargo tree -p sa-core` containing no terminal crate.
- `sa-web` must not depend on `sa-daemon`. It wraps the `sa-core` dispatcher and serves
  embedded assets; it holds no state of its own.
- `sa-web` must not depend on `sa-client` or on any terminal crate.
- `sa-daemon` may depend on `sa-web` to host the listener, and must not depend on a terminal
  crate.
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
version and the client's own name and version:

```json
{"method": "hello", "params": {"protocol": 1, "client": "sa", "client_version": "1.0.0"}}
```

Version mismatch is a hard failure with an explicit error naming both versions. There is no
negotiation and no best-effort compatibility.

`DECIDED`: protocol version is a single integer, bumped on any breaking change. It starts
at 1.

### 3.2 Methods

Session lifecycle:

```text
session.create   {cwd, autonomy, title?}          -> {session_id, seq}
session.list     {include_deleted?}               -> [{id, title, state, owner, updated_at}]
session.attach   {session_id, from_seq, limit?,   -> {session, events[], more, next_seq}
                  takeover?}
session.detach   {session_id}                     -> {}
session.rename   {session_id, title}              -> {}
session.fork     {session_id, from_seq,           -> {session_id, workspace}
                  workspace, title?}
session.delete   {session_id}                     -> {}
session.usage    {session_id}                     -> {prompt_tokens, completion_tokens,
                                                     cost_micros?}
```

Turns:

```text
turn.start       {session_id, prompt, detach?}    -> {turn_id, seq}
turn.resume      {session_id, turn_id}            -> {seq}
turn.cancel      {session_id, turn_id}            -> {}
turn.status      {session_id}                     -> {turn_id?, state, since}
```

Events:

```text
events.subscribe {session_id, from_seq, filter?}  -> {subscription_id}
events.unsubscribe {subscription_id}              -> {}
events.replay    {session_id, from_seq, to_seq?,  -> {events[], more, next_seq}
                  limit?}
```

Approvals:

```text
approval.list    {session_id}                     -> [{approval_id, class, detail,
                                                      requested_at, expires_at}]
approval.respond {approval_id, decision, scope?}  -> {resolved_as}
```

Handoff:

```text
handoff.export   {session_id, mode}               -> {bundle_path, bytes, epoch, sha256}
handoff.import   {bundle_path, verify_only?,      -> {session_id, epoch, workspace}
                  workspace?}
handoff.accept   {session_id, epoch}              -> {session_id}
```

Administration:

```text
ping             {}                               -> {version, uptime_seconds, sessions}
model.list       {}                               -> [{id, context_limit, capabilities}]
model.select     {session_id, model}              -> {}
```

Knowledge:

```text
knowledge.list    {namespace}                     -> [{path, title, updated, bytes}]
knowledge.read    {path}                          -> {path, body, updated}
knowledge.search  {q, namespaces?, limit?}        -> [{path, title, snippet, score}]
knowledge.reindex {}                              -> {files, indexed}
```

The agent reaches the same store through tools (§11.4), so every write it makes is already a
`tool.completed` event. These methods exist for the web surface and the CLI, which browse
knowledge without starting a turn.

`DECIDED`: method names are stable API. Renaming one is a protocol version bump.

### 3.3 Notifications

Server to client, never acknowledged:

```text
event            {session_id, seq, kind, payload}
gap              {session_id, from_seq, to_seq}   -- client fell behind, re-sync required
approval.request {approval_id, session_id, class, detail, expires_at}
turn.state       {session_id, turn_id, state}
evicted          {session_id, reason}             -- another client took over (§3.6)
log              {level, message}                 -- daemon diagnostics
```

`DECIDED`: a client that cannot keep up receives `gap` and is dropped from the live
subscription; it re-syncs with `events.replay`. A slow client must never apply backpressure
to the agent loop. The bounded queue is 4096 events or 8 MiB, whichever is hit first.

### 3.4 Per-client filtering

Subscriptions carry a filter so that reasoning traces, tool output verbosity, and cost
events can differ per client without changing what is persisted. Filtering is applied
server-side at send time. The log is always complete; the filter only affects delivery.

Filter shape:

```json
{"exclude_kinds": ["llm.delta", "llm.reasoning"], "deltas": "off"}
```

- `exclude_kinds` is a list of event kind names, matched exactly. Default is empty.
- `deltas` is `raw`, `coalesced`, or `off`. `coalesced` batches `llm.delta` payloads into
  100 ms windows. Default is `raw`.
- There is no include-list. A filter can only remove, never add.

### 3.5 Pagination

`session.attach` and `events.replay` accept `limit`, default 500, maximum 5000. When
`more` is true the caller continues from `next_seq`. A replay that would cross a `gap`
boundary returns `more: true` rather than a partial window.

### 3.6 Client exclusivity

`DECIDED`: at most one client is attached to a session at a time. The daemon refuses a
second `session.attach` with `session_busy`, naming the holder's client name and the time
of attachment.

- `takeover: true` overrides the refusal. The incumbent receives `evicted`, loses write
  authority, and may re-attach to regain it. Takeover does not interrupt a running turn.
- Detached turns are unaffected. `turn.start {detach: true}` runs with no client attached;
  that is the point of the design, not an exception to this rule.
- Approvals with no attached client queue per §8.2.

Presence is therefore a single field, not a set: a session is either attended or not.

`DECIDED`: the web surface (§4.3) drives detached turns and subscribes; it never attaches.
Several browser tabs may therefore watch one session, and §3.6 constrains only interactive
control, which is the CLI's job.

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
- The daemon's lifetime is owned by systemd (§12), never by a client connection.

`DECIDED`: SSH stdio is the remote transport. No other remote transport is scheduled.

### 4.3 HTTP (web surface)

`DECIDED`: the web surface is the knowledge and memory front end (§1), served by the daemon
itself over loopback HTTP. Unlike a chat gateway, HTTP is a passive carrier: it holds no
outbound connection, no reconnect state and no rate-limit budget, so it satisfies §2.3
without widening the daemon's failure domain.

```text
POST /rpc          one json-rpc request, one json-rpc response
GET  /events       server-sent events, one stream per subscription
GET  /             embedded application shell
GET  /assets/*     embedded static assets
POST /login        token exchange, sets the session cookie
POST /logout       clears it
```

Server-sent events rather than websockets: the browser needs one direction for events and a
plain request for everything else, and SSE reconnects with `Last-Event-ID` mapped directly
onto `from_seq`. A websocket would duplicate a reconnect protocol the daemon already has.

#### Binding

Loopback only, `127.0.0.1:7420` by default, overridable in `$XDG_CONFIG_HOME/sa/web.toml`.
The listener is
disabled unless that file exists. TLS is terminated by a reverse proxy, never by the daemon.
`X-Forwarded-For` is honoured only when the immediate peer is loopback.

#### Authentication

On first start with the listener enabled, the daemon generates `$XDG_STATE_HOME/sa/web.token`,
32 bytes hex, mode `0600`.

- `POST /login {token}` compares in constant time and, on success, sets `sa_session`: 32 bytes
  of randomness, `HttpOnly`, `SameSite=Strict`, `Secure`, 30-day expiry, held server-side in
  `web_sessions` (§6.1).
- Every mutating request must additionally carry `X-SA-Token: <session value>`. A custom
  header cannot be set by a cross-origin form post, so this is the CSRF defence and it needs
  no per-form token machinery.
- `Origin` and `Referer`, when present, must match the configured allowlist.
- Login attempts are rate limited to 5 per minute per peer address.

`DECIDED`: the URL token is exchanged once and is not a bearer credential for `/rpc`. A
leaked URL is not a leak.

#### Trust boundary

The web surface does not weaken §1.2. A browser is an untrusted client speaking JSON-RPC over
a different carrier; it has exactly the authority any other client has, which is none.
Authorisation — autonomy, knowledge namespaces, approvals — is decided in the dispatcher,
never in the UI.

#### Assets

Hand-written ES modules and CSS, embedded with `rust-embed`, served from the binary. No npm,
no bundler, no build step (§15).

---

## 5. Session model

### 5.1 Event sourcing

A session is an append-only, ordered log of events with a per-session monotonic `seq`
starting at 1. All state is derived from the log. There is no mutable session state that is
not reconstructible from events.

Event kinds:

```text
session.created     session.renamed      session.state
turn.started        turn.completed       turn.interrupted
llm.delta           llm.reasoning        llm.completed
tool.requested      tool.completed       tool.failed
approval.requested  approval.resolved
model.changed       checkpoint
handoff.exported    handoff.imported
compaction.applied  review.completed     error
```

`DECIDED`: adding an event kind is a minor version bump; changing an existing payload's
shape is a major one.

Event payloads are versioned and must not encode storage-specific details. A payload must
be readable by an importer that has never seen the sqlite schema.

### 5.2 Event envelope and payloads

Every event row stores the payload as JSON with a mandatory `v` field carrying the payload
version. Payload version starts at 1 for every kind. A payload with an unknown `v` is a hard
error on import, never a best-effort decode.

Common envelope, not repeated below: `seq`, `kind`, `at` (unix seconds).

```text
session.created     {v, title, cwd, repo_id, autonomy,
                     owner:{host_fingerprint, epoch}}
session.renamed     {v, title}
session.state       {v, state, reason?}
turn.started        {v, turn_id, prompt, model, autonomy, detach, resumed_from?}
turn.completed      {v, turn_id, finish_reason, usage:{prompt_tokens,
                     completion_tokens, cost_micros?}}
turn.interrupted    {v, turn_id, cause, pending_call_id?}
llm.delta           {v, turn_id, text}
llm.reasoning       {v, turn_id, text}
llm.completed       {v, turn_id, stop_reason, usage:{...}, purpose}
tool.requested      {v, turn_id, call_id, tool, args}
tool.completed      {v, turn_id, call_id, output, truncated, duration_ms}
tool.failed         {v, turn_id, call_id, error, class}
approval.requested  {v, approval_id, class, detail, expires_at}
approval.resolved   {v, approval_id, decision, scope, resolved_by}
model.changed       {v, from, to}
checkpoint          {v, ref, base_commit, tree, includes_untracked}
handoff.exported    {v, mode, epoch, sha256, seq_range}
handoff.imported    {v, mode, epoch, source_host}
compaction.applied  {v, from_seq, to_seq, summary, tokens_before,
                     tokens_after, trigger, usage}
review.completed    {v, turns_covered, written[], usage}
error               {v, code, message, context}
```

`purpose` on `llm.completed` is `turn` or `compaction`, so summarisation cost is visible
separately from work cost.

Rules:

- `args`, `output`, and `summary` may be replaced by a blob reference (§6.3). A payload
  field above 64 KiB is always a blob reference; the event line itself never exceeds 1 MiB.
- `turn.interrupted` records `pending_call_id` when a tool call was in flight. There must
  never be a `tool.requested` without a matching `tool.completed` or `tool.failed`; recovery
  appends the missing terminal event (§5.6).
- `cost_micros` is omitted when the model is absent from the pricing table (§10.3).

### 5.3 Ownership and fencing

Each session records an owner:

```text
owner = { host_fingerprint, epoch }
```

`host_fingerprint` is sha256 of `/etc/machine-id` when readable, otherwise a UUID generated
once at daemon start and stored at `$XDG_STATE_HOME/sa/machine-id`, mode `0600`. It is not a
hostname and not a network address.

`epoch` increments on every successful handoff. Fencing rules:

- A daemon accepts `turn.start` only if the session's `owner_epoch` equals the epoch it
  holds. Otherwise it rejects with a stale-owner error naming both epochs.
- Fencing uses the epoch counter, never wall-clock time. Clock skew between laptop and
  server must not be load-bearing.
- Epoch comparison is integer equality for writes and integer ordering for staleness
  rejection.

### 5.4 Session states

```text
active        writable by its owner
quiescing     draining an in-flight turn before handoff
relocating    exported, awaiting acceptance on the target
relocated     moved away; read-only tombstone on the source host
interrupted   a turn died with the daemon; resumable, no automatic retry
deleted       tombstoned (§5.7)
```

`relocated` sessions remain listed on the source host so the user can see where a session
went, and can be revived by handing them back.

### 5.5 Handoff: move versus fork

Two explicit modes, chosen at export time. There is no implicit behaviour.

- **move** — the intended overnight workflow. Source quiesces at a turn boundary, writes a
  `checkpoint`, exports the bundle, increments the epoch, transitions to `relocated`, and
  refuses further turns. Target imports and becomes owner at the new epoch. History stays
  linear. The source cannot write even if it wakes up believing it is still the owner.
- **fork** — the target receives a copy with `parent_id` set to the source session and its
  own epoch lineage, plus a **separate workspace** (§7.5). Both remain writable and diverge
  deliberately. There is no merge.

`DECIDED`: transfer is a move by default. Divergence requires asking for a fork, and a fork
requires an explicit target workspace path that does not exist or is empty.

### 5.6 Crash recovery

On startup the daemon scans for sessions with an open turn and appends `turn.interrupted`
with `cause: daemon_restart`. It does not retry tool calls, because tool side effects are
not idempotent.

Where a turn had an in-flight tool call, recovery also appends `tool.failed` with
`class: interrupted` for that `call_id`. The model therefore sees a definite answer rather
than a dangling call, and the log satisfies the pairing rule in §5.2.

`turn.resume {session_id, turn_id}` re-enters the loop for an interrupted turn. It appends a
new `turn.started` with `resumed_from` set to the interrupted turn id and re-issues the model
call with the log as context. It is an explicit client action; nothing resumes on its own.

### 5.7 Deletion

`DECIDED`: `session.delete` is a tombstone. The session row moves to `deleted`, the event log
is retained, and the session disappears from `session.list` unless
`include_deleted: true` is passed.

Deletion is refused with `session_busy` while a turn is in flight. A `relocated` session may
be deleted; it is not revived by the deletion. Physical purge is not in scope before v5.0 and,
when it arrives, is a separate `session.purge` method rather than a flag on `delete`.

---

## 6. Persistence

### 6.1 Live store

`DECIDED`: sqlite in WAL mode at `$XDG_STATE_HOME/sa/sa.db` is the sole source of truth for
live sessions. One daemon serves many workspaces, so the store is daemon-wide and each
session records its own `cwd`.

```text
events(id INTEGER PRIMARY KEY, session_id TEXT, seq INTEGER, kind TEXT,
       payload TEXT, created_at INTEGER, UNIQUE(session_id, seq))
sessions(id TEXT PRIMARY KEY, title TEXT, parent_id TEXT, state TEXT,
         owner_host TEXT, owner_epoch INTEGER, cwd TEXT, repo_id TEXT,
         autonomy TEXT, model TEXT, created_at INTEGER, updated_at INTEGER)
approvals(id TEXT PRIMARY KEY, session_id TEXT, seq INTEGER, state TEXT,
          request TEXT, resolution TEXT, requested_at INTEGER, resolved_at INTEGER)
blobs(hash TEXT PRIMARY KEY, bytes INTEGER, data BLOB)
web_sessions(id TEXT PRIMARY KEY, created_at INTEGER, expires_at INTEGER, peer TEXT)
schema_version(version INTEGER)
```

Rationale: a daemon with several sessions writing concurrently needs atomic commit and
crash safety. An append-only text file needs hand-rolled locking and torn-tail repair; a
`kill -9` mid-tool-call must not require a repair pass.

#### Nothing is written into the workspace

`DECIDED`: `sa` never creates, modifies or deletes a file inside a workspace. Not a
`.sa/` directory, not a `.gitignore` line, not an `AGENTS.md`, not a lock file, not a
cache. `git status` after any `sa` operation is byte-identical to before it.

Everything sa needs lives in the two XDG roots:

```text
$XDG_CONFIG_HOME/sa/                     user-authored, deployable from dotfiles
  policy.toml                            global command allowlist (spec 8.2)
  web.toml                               web listener config (spec 4.3)
  workspaces/<slug>/policy.toml          per-workspace override
  workspaces/<slug>/SOUL.md              persona, read only
  workspaces/<slug>/AGENTS.md            project context, read only

$XDG_STATE_HOME/sa/                      runtime state, never hand-edited
  sa.db  sa.db-wal  sa.db-shm            session store
  knowledge.db                           derived knowledge index (spec 11.2)
  knowledge/                             knowledge content (spec 11.1)
  machine-id                             host fingerprint fallback (spec 5.3)
```

`<slug>` is the workspace path with the leading `/` stripped and remaining `/` replaced by
`-`, exactly as `~/.pi/agent/projects/` is named. `/home/chrisaw/.dotfiles` becomes
`home-chrisaw-.dotfiles`. This is deliberate: one dotfiles module can deploy context for
both agents from a single source tree, and neither agent has to know about the other.

Read-only exceptions, both permitted because they predate sa:

- an `AGENTS.md` or `SOUL.md` that already exists in the workspace root is read and never
  written. If absent, sa does not create one.
- `refs/sa/checkpoints/` is written into `.git` (spec 8.4). It is git metadata, invisible
  to the working tree and to `git status`, and is removed with `git update-ref -d`.

There is no `sa init`. There is nothing to initialise: the daemon creates its own
directories on first use, and per-workspace config is optional.

### 6.2 Canonical bundle (handoff format)

A handoff is a single container file, written atomically (temp file plus rename):

```text
manifest.json     format_version, session_id, owner_epoch, seq_range,
                  content_hash, created_at, source_host, workspace, autonomy, mode
events.jsonl      the full event log, one JSON object per line, in seq order
workspace.patch   unified diff of the dirty worktree
untracked/        contents of untracked files, size-capped
```

`DECIDED`: sqlite is authoritative at rest; the bundle is authoritative in transit. There is
exactly one write path for each, and no format is a second live store.

`content_hash` is sha256 over the concatenation of `events.jsonl` and `workspace.patch` in
that order, with the `untracked/` tree hashed as a sorted list of `path\0sha256` lines.

### 6.3 Blobs

Oversized payload fields are stored once in `blobs` and referenced inline as
`{"$blob": "<hex sha256>", "bytes": N}`. This applies to `tool.requested.args`,
`tool.completed.output`, and `compaction.applied.summary`.

The bundle flattens blobs back into `events.jsonl` on export and re-splits them on import,
so the bundle format never exposes the storage detail. Blob garbage collection is a v5.0
concern; until then blobs are retained.

### 6.4 Import verification

Import is verified before any mutation. Rejection is atomic and leaves the target daemon
unchanged. Reject when:

- `format_version` is unsupported;
- `content_hash` does not match;
- `seq` values are not contiguous within `seq_range`;
- an event kind is unknown (hard error, never silently dropped);
- a payload `v` is unknown;
- the session id already exists locally with an `owner_epoch` greater than or equal to the
  bundle's (stale bundle);
- the workspace descriptor cannot be applied (§7.3).

Verification runs in a scratch directory. The database is opened for write only after every
check passes, and the whole import is one transaction.

### 6.5 Derived exports

`sa export --format jsonl|markdown` produces human-readable artefacts for review and
diffing. Derived only. Never read back as authoritative state.

---

## 7. Workspace handling

A session is meaningless without the tree it was editing. The bundle therefore carries a
workspace descriptor, not a filesystem image.

### 7.1 Descriptor

```text
kind             "git"
repo_id          sha256(git-common-dir path + root commit)
head_commit      commit the session was based on
remote           origin URL, or null
dirty_patch      unified diff of tracked modifications
untracked        file list with contents, size-capped
toolchain_hint   presence of flake.nix / shell.nix / rust-toolchain.toml
```

`DECIDED`: a workspace is exactly one git repository. Multi-repo trees are refused by name,
not silently half-transferred. A repository with no remote is supported.

`repo_id` is a local identity: it matches only when the git common directory path and the
root commit both match. It is used to apply a patch in place, never as proof that two hosts
have "the same" repository.

### 7.2 Export preconditions

Export refuses, with a specific error, when the tree contains state the descriptor cannot
capture:

- submodules (`.gitmodules` present);
- any file in the transfer set above 8 MiB, or the total above 64 MiB;
- untracked files that are ignored by `.gitignore` but are referenced by the patch — the
  daemon cannot know which ignored files matter, so it refuses rather than guessing;
- a dirty index that cannot be represented as a patch (conflict markers present).

Silent divergence costs a day; a refusal costs a minute.

### 7.3 Import application

Three cases, resolved in order:

1. `repo_id` matches a local repository → apply `dirty_patch` in place. Verify `head_commit`
   equals, or is an ancestor of, the current `HEAD`. If neither holds, refuse with
   `workspace_diverged`, naming both commits.
2. `repo_id` does not match, `remote` is non-null → clone into `cwd`, check out
   `head_commit`, apply the patch and untracked files.
3. `repo_id` does not match, `remote` is null → refuse with `workspace_unreachable` unless
   the caller supplies `workspace: <path>`, whose root commit must equal `head_commit`'s
   root.

Every case applies the patch with a three-way merge disabled. A patch that does not apply
cleanly is a refusal, not a fuzz.

### 7.4 Workspace lock

`DECIDED`: the daemon holds an exclusive lock on a repository's **git common directory** for
the duration of any turn that can write. The lock file is
`$XDG_RUNTIME_DIR/sa/locks/<sha256(git-common-dir)>.lock`.

This exists because §8.4 creates a checkpoint commit and because two sessions sharing an
index will otherwise corrupt each other's staging state. A second session in the same
repository waits for the lock rather than failing; `turn.status` reports
`state: waiting_workspace`.

Different worktrees of one repository share a git common directory and therefore share the
lock. That is correct: they share the index.

### 7.5 Fork workspaces

A fork needs its own tree. `session.fork` requires `workspace: <path>`; the daemon refuses
if the path exists and is non-empty. The target tree is produced by the same procedure as
§7.3 case 2 or 3, from the source's descriptor.

The source tree is untouched by a fork. Two writable sessions on one tree is exactly the
divergence the lock in §7.4 exists to prevent, so it is disallowed rather than serialised.

### 7.6 Secrets

`DECIDED`: secrets never enter the bundle, and this is enforced rather than promised.

- Nothing under either XDG root is transferred; only the workspace's own files are.
- Export refuses when any path in the transfer set matches the deny list: `*.pem`, `*.key`,
  `*.p12`, `id_*`, `.env`, `.env.*`, `*credentials*`, `*secret*`.
- `--allow-secret-paths` downgrades the refusal to a warning recorded in the
  `handoff.exported` event, so the override is visible in history.
- Credentials are daemon-local and live outside the workspace (§10).

A bundle is data, never code. Import never executes anything from it.

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

Knowledge writes are permitted under `allowlisted`. The knowledge root (§11) lives outside
the workspace, so the workspace checkpoint cannot restore it; the recovery story for
knowledge is that the root is a git repository (§11.1), not the checkpoint.

### 8.2 Allowlist

`allowlisted` needs a concrete allowlist, so it has one:

```text
$XDG_CONFIG_HOME/sa/policy.toml                     global default
$XDG_CONFIG_HOME/sa/workspaces/<slug>/policy.toml   per-workspace override

  [allowlisted]
  commands = ["cargo build", "cargo test", "git diff", "git status", "rg", "ls"]
  network  = []          # commands permitted to reach the network
```

The file lives outside the workspace (§6.1) and is never transferred. The per-workspace
override wins when it exists. Matching is by argv prefix, not by substring, and the resolved
binary must be on a fixed PATH.

Network denial under `allowlisted` is enforced with `unshare -n` when available. When it is
not, the daemon records a `session.state` event at turn start carrying
`reason: network_isolation_unavailable`, and the guarantee downgrades to allowlist-only for
that turn. The downgrade is visible in history rather than assumed away.

`DECIDED`: the checkpoint in §8.4 is the real guarantee. Network isolation is defence in
depth, not the contract.

### 8.3 Approval routing

`PermissionClass` is enforced by the daemon, not the UI. When an operation needs approval:

- the attached client receives `approval.request` and may answer;
- with no client attached, the request is queued and the turn blocks until
  `expires_at` (default 300 s), then resolves as denied and the turn fails with an explicit
  reason;
- queued approvals surface in `approval.list` on the next attach.

`approval.respond` takes `decision: allow | deny` and `scope`:

```text
once      this request only (default)
turn      every remaining request in this turn
session   every remaining request in this session
class     every future request of this PermissionClass in this session
```

`scope` is ignored when `decision` is `deny`; a denial always resolves exactly one request.

`DECIDED`: deny on timeout. Never auto-approve because nobody was watching.

### 8.4 Unattended safety net

Before any unattended turn the daemon creates a checkpoint and records it in a `checkpoint`
event. The checkpoint is a commit built against a temporary index, so neither the worktree
nor the real index is touched:

```text
GIT_INDEX_FILE=<temp> git add -A
GIT_INDEX_FILE=<temp> git write-tree
git commit-tree <tree> -p HEAD -m "sa checkpoint <session_id> <seq>"
git update-ref refs/sa/checkpoints/<session_id>/<seq> <commit>
```

The event carries `ref`, `base_commit`, `tree`, and `includes_untracked: true`.

`DECIDED`: not `git stash`. `git stash push` cleans the worktree, which is the opposite of
what a checkpoint wants, and it mutates a shared list. `git stash create` avoids the list but
silently ignores untracked files, which is exactly what an agent writes. The temporary-index
commit captures tracked and untracked files, changes nothing in place, and is removed with
`git update-ref -d`.

Recovery is documented, not inferred:

```text
git checkout <ref> -- .
```

That restores tracked files to the checkpoint state. Files created after the checkpoint are
left in place: recovery never deletes work it is unsure about.

Retention: the daemon keeps the 10 most recent checkpoint refs per session and deletes older
ones at checkpoint time. Nothing outside `refs/sa/` is ever touched.

Checkpoints are taken under the workspace lock (§7.4). A checkpoint failure aborts the turn
before any tool runs; an unattended turn never starts without one.

---

## 9. Tools

Initial set, all executed in the daemon process with the session's `cwd`:

```text
read  write  edit  bash  grep  find  ls
```

The knowledge tools (§11.4) are the second set and arrive at v3.0:

```text
knowledge.search  knowledge.read  knowledge.list  knowledge.write  knowledge.remove
```

Rules:

- Paths are canonicalised and must resolve inside the session workspace. Escapes are
  errors, not clamps. Symlinks are resolved before the check.
- Every tool declares a `PermissionClass`; the daemon enforces it per §8.
- Tool output is bounded; truncation is recorded in the event so a client knows it saw a
  partial result. The bound is 256 KiB before blob spill (§6.3).
- Tools are async and cancellable. `turn.cancel` drops in-flight tool futures and appends
  `tool.failed` with `class: cancelled` for each.
- `bash` runs with a fixed PATH, no shell profile, and a bounded timeout (default 600 s).
  Non-zero exit is `tool.completed`, not `tool.failed`; `tool.failed` means the tool itself
  failed to run.

`DECIDED`: tool calls are sequential within a turn. Parallelism is revisited only after v1.0
is green and only if a measured turn is latency-bound on tool calls.

---

## 10. Credentials and models

### 10.1 Ownership

- The daemon owns model credentials. Clients never see a key.
- Configuration is daemon-local: environment (`OPENAI_API_TOKEN`, `OPENAI_API_ENDPOINT`) and
  a credential file under `$XDG_CONFIG_HOME/sa/` at mode `0600`.
- Bundles carry no credentials. A handoff to a host without credentials fails at import
  verification, not mid-turn.

### 10.2 Wire protocol

`DECIDED`: OpenAI-compatible chat completions only, including streaming and tool calls.
Other vendors are reached through OpenAI-compatible endpoints.

Internally, a single `Provider` trait with one implementation:

```rust
trait Provider {
    async fn stream(&self, req: ChatRequest) -> impl Stream<Item = Delta>;
}
```

Adding a native Anthropic or Google implementation later is additive and touches no part of
the agent loop. Nothing else is built for it now.

### 10.3 Pricing

`cost_micros` is computed from a static table in daemon config, keyed by model id. An
unknown model omits `cost_micros` rather than guessing. `session.usage` sums
`llm.completed` usage across the session.

### 10.4 Compaction

`DECIDED`: compaction is automatic, because the motivating workflow is an unattended
overnight run and a hard stop at the context limit wastes the night.

- Trigger: 80% of the selected model's `context_limit`, checked before each turn.
- Token cost is estimated at four characters per token. No tokeniser dependency; the
  estimate only has to be conservative enough to fire early.
- The daemon summarises the oldest complete turns, retaining at least the last 4 turns and
  the current turn verbatim. The range is bounded so that a single compaction never
  summarises more than 50% of the log.
- Output is a `compaction.applied` event carrying `from_seq`, `to_seq`, the summary, token
  counts, `trigger: auto`, and the summarisation's own `usage`.
- The model's view is: summary, then every event after `to_seq`. The log retains every
  original event. A replay or a re-attach still shows the full transcript; compaction
  changes what the model sees, never what the user sees.
- Compaction is a normal event append. A crash during compaction leaves the log consistent
  and simply retries on the next call.
- If a single turn's own events exceed the context limit, compaction cannot help and the
  turn fails with `context_exhausted` naming the turn id.

Manual compaction is `trigger: manual` and lands with the web surface at v2.0, where it is a
button rather than a keystroke.

---

## 11. Knowledge

A durable, user-inspectable store of what the agent has learned, shared across sessions and
surviving restarts. It replaces the Hermes memory/skills layout with two deliberate
differences: the files are authoritative rather than a database, and the agent cannot write
outside its namespaces.

### 11.1 Layout

```text
$XDG_STATE_HOME/sa/knowledge/          default root, overridable in knowledge.toml
  shared/            read by every session
  user/<id>/         read and written by one user
  workspace/<id>/    read and written by sessions in one workspace
```

Files are markdown. The path is the identity: `knowledge/<namespace>/<name>.md`. Optional
YAML frontmatter carries `title`, `tags`, `updated`.

`DECIDED`: markdown on disk, not rows in a database. The user must be able to read, edit,
diff and back up what the agent knows; an opaque store becomes a liability nobody prunes. The
root may itself be a git repository, and `sa knowledge` commits on write when it is.

### 11.2 Index

`$XDG_STATE_HOME/sa/knowledge.db`, sqlite FTS5 over `path`, `namespace`, `title`, `body`.

The index is derived and rebuildable: `knowledge.reindex` walks the root and rebuilds it. A
missing or corrupt index is never an error, only a reindex. Files win every disagreement.

`DECIDED`: no embeddings at first. FTS5 is already a dependency, costs no model call, and
keyword recall over agent-written headings is adequate until it demonstrably is not. Adding
embeddings later is additive behind the same `knowledge.search` signature.

### 11.3 Namespaces and authorisation

A session is created with a resolved namespace set, recorded in `session.created`:

```json
{"knowledge_namespaces": ["shared", "user:chris", "workspace:stuffies"]}
```

Resolution happens at session creation from `$XDG_CONFIG_HOME/sa/knowledge.toml`:

```toml
[users.chris]
allow = ["shared", "user:chris", "workspace:stuffies"]

[default]
allow = ["shared"]
```

The daemon refuses any knowledge read or write outside the session's set with
`knowledge_denied`, naming the namespace. This is the mechanism that replaces running a
container per tenant: one daemon, one supervision surface, isolation enforced in the
dispatcher. A single-user deployment leaves `[default]` permissive and never notices.

### 11.4 Tools

The agent reaches the store through tools, so every access it makes is already in the event
log:

```text
knowledge.search  {q, namespaces?, limit?}   read
knowledge.read    {path}                     read
knowledge.list    {namespace}                read
knowledge.write   {path, body}               knowledge_write
knowledge.remove  {path}                     knowledge_write
```

`knowledge.write` refuses a path outside the session's namespaces, refuses to overwrite a
file whose `updated` frontmatter is newer than the one it read, and records the write rather
than rewriting silently.

### 11.5 Background review

`DECIDED`: off by default and opt-in per session. It is a second model call per batch of
turns, and the budget must be the user's choice, not a surprise.

When enabled, a review runs after every 10 completed turns and is bounded:

```text
max input tokens   50 000
reasoning effort   low
timeout            300 s
max writes         8 files per review
```

The bounds come from the measured Hermes configuration this replaces, where the unbounded
default was observed to park a session behind a multi-minute decode. A review writes through
`knowledge.write` under the session's namespaces and emits `review.completed`. That is the
only new event kind: searches and reads are already `tool.requested`/`tool.completed` pairs.

### 11.6 Handoff

Knowledge is not in the bundle. A bundle carries a session, not the whole brain, and shipping
the root on every handoff would make bundle size a function of history rather than of the
session.

`handoff.export` records the knowledge namespaces and a per-file hash in the manifest. Import
warns when a namespace is absent on the target and lists the missing paths in a `log`
notification; the session continues with whatever the target has.

`OPEN`: whether a `--with-knowledge` flag that ships the referenced files is wanted. Not
required for the motivating workflow.

---

## 12. Lifecycle and supervision

- systemd user unit `sa-daemon.service`, `Restart=on-failure`, `RuntimeDirectory=sa` with
  mode `0700`.
- `sa` auto-spawns the daemon when the socket is absent, guarded by an exclusive bind lock
  so auto-spawn and systemd can never both start a daemon.
- The daemon never exits because its last client disconnected.
- The web listener (§4.3) starts only when `$XDG_CONFIG_HOME/sa/web.toml` exists. It is a
  carrier, not a
  supervisor: it never owns the daemon's lifetime, and stopping it does not stop a turn.
- Graceful shutdown drains in-flight turns or marks them `interrupted`; it never truncates
  the log.

---

## 13. Failure modes

Each of these has a required behaviour and a test in the milestone that introduces it.

| Failure | Required behaviour |
| --- | --- |
| daemon killed mid-turn | on restart, append `turn.interrupted` and a terminal `tool.failed` for any in-flight call; no tool retry; client sees the interruption |
| SSH carrier drops | daemon and turn unaffected; client reconnects and replays from its cursor |
| client stalls | bounded queue (4096 events / 8 MiB), `gap` notification, client dropped from live subscription; agent unaffected |
| second client attaches | refused with `session_busy` naming the holder; `takeover` evicts the incumbent |
| stale bundle (old epoch) | import rejected, target unchanged |
| two hosts claim the session | lower epoch loses; its writes are rejected, not merged |
| two sessions, one repository | second turn waits on the workspace lock; reported as `waiting_workspace` |
| disk full | turn aborts inside a transaction; log stays consistent |
| approval timeout while detached | denied, recorded, turn fails with reason |
| model endpoint unavailable | turn fails, recorded, session remains resumable |
| partial bundle write | impossible: temp file plus rename, verified by hash before acceptance |
| unknown event kind or payload version on import | hard rejection, no partial import |
| checkpoint fails before an unattended turn | turn does not start; failure recorded |
| context limit reached mid-turn | compaction at 80%; if a single turn exceeds the limit, `context_exhausted` and the turn fails |
| secret-path match on export | refused, named; `--allow-secret-paths` records the override |
| web token rotated | all `web_sessions` invalidated; browsers re-login; CLI unaffected |
| cross-origin POST without `X-SA-Token` | 403, no state change |
| browser closes mid-turn | turn unaffected, because it is detached; reconnect with `Last-Event-ID` replays exactly the missed events |
| `X-Forwarded-For` from a non-loopback peer | header ignored, warning logged; the peer address is the truth |
| knowledge write outside the session's namespaces | `knowledge_denied`, no write, recorded |
| knowledge file edited on disk while indexed | reindex wins; files are authoritative and the index is rebuilt |
| background review over budget | skipped and logged; the turn is unaffected |
| knowledge root unwritable | knowledge tools fail with `knowledge_unavailable`; turns still run |

---

## 14. Release plan

Three releases, in the order the surfaces are wanted, then the remote workflow and packaging.
Each is independently testable and leaves the tree green.

**v1.0 — daemon and CLI coding agent.** The coding surface: a line-oriented CLI driving an
agent against a working tree, with the daemon, event log, model client, tools, policy,
approvals, detached turns and compaction underneath it.

Sub-work, in order:

1. Protocol, framing, hello gate, unix socket, `sa rpc --stdio` (Appendix A).
2. sqlite event log, blob spill, replay, session lifecycle, attach exclusivity, crash
   recovery.
3. OpenAI-compatible provider, streaming into the log, usage accounting.
4. Tools, permission classes, autonomy levels, allowlist, workspace lock, checkpoint.
5. Detached turns, `turn.resume`, automatic compaction.
6. CLI: `sa run -p`, `sa attach` line mode, `sa export`. There is no `sa init` (§6.1).

Acceptance: a crash test kills the daemon mid-append and asserts a consistent log on restart
with a terminal event for every open tool call; a recorded-cassette test produces a completed
turn with contiguous events and populated usage, touching no network; a detached turn
completes with no client attached; a `read-only` session performs zero writes; a denied
approval is recorded and fails the turn; two sessions in one repository serialise on the
workspace lock; a long session compacts and the post-compaction view is within the limit
while the log stays complete.

**v2.0 — web interface.** The general surface: sessions, live transcripts, approvals, cost
and manual compaction in a browser.

Sub-work: HTTP carrier and auth (§4.3), SSE with `Last-Event-ID`, embedded assets, session
list and creation, live transcript rendering, approval queue, usage display.

Acceptance: a wrong token is rejected and rate limited; a cross-origin POST without
`X-SA-Token` is 403 and changes nothing; an SSE reconnect with `Last-Event-ID` replays
exactly the missed events; the UI renders a running session, answers an approval and cancels
a turn; the daemon serves the UI with no network egress.

**v3.0 — knowledge and memory.** The Hermes-type surface: durable memory, skills and
retrieval, browsable and editable in the web interface.

Sub-work: knowledge root and layout (§11.1), FTS5 index (§11.2), namespace resolution and
ACL (§11.3), knowledge tools (§11.4), background review (§11.5), knowledge browser in the web
interface.

Acceptance: knowledge tools round-trip through the ACL; a session in namespace A cannot read
namespace B; `knowledge.reindex` rebuilds from files after the index is deleted; a review
respects its token, time and write bounds and writes only inside the session's namespaces; a
knowledge write appears in the event log.

At v1.0, `session.fork` and the `handoff.*` methods return `not_implemented` (-32014)
rather than `method_not_found`, so a client can tell "not yet" from "no such method".

**v4.0 — remote workflow.** Handoff, bundle export/import, epoch fencing, workspace
descriptor and transfer (§5.3, §5.5, §6.2, §7).

Acceptance: round-trip a session between two in-process daemons; the source refuses writes
after `move`; a stale bundle is rejected without mutation; `fork` produces a `parent_id` and a
distinct workspace; a dirty worktree transfers and applies cleanly; a submodule is refused by
name; `id_rsa` is refused unless overridden.

**v5.0 — packaging.** systemd unit, Nix module, auto-spawn lock, blob garbage collection,
optional `session.purge`.

Acceptance: the daemon survives a client kill and a service restart; auto-spawn and systemd
cannot race.

`DECIDED`: no ratatui TUI is scheduled. The CLI is line-oriented, and the web surface covers
what a TUI would have covered. If one is ever wanted it is an additional client of
`sa-client` and changes nothing else.

---

## 15. Engineering constraints

- Rust, edition 2021, workspace lints as configured in `Cargo.toml`: `unwrap_used` denied,
  `print_stdout`/`print_stderr` denied outside binaries, pedantic allowed. Tests may use
  `expect` with a message.
- No language-level package managers. Dependencies are declared in `Cargo.toml` and
  provided through nixpkgs.
- No npm, no JavaScript build step. The web assets (§4.3) are hand-written ES modules and
  CSS, embedded with `rust-embed`. If a build step is ever genuinely required it is a
  `build.rs`, not a package manager.
- Pre-commit gates are mandatory: `cargo fmt --check`, `cargo clippy --all-targets
  --all-features -- -D warnings`, `cargo test --workspace`, `cargo deny check`.
- No `unsafe`.
- Tests are behaviour-named (`kills_daemon_mid_turn_and_recovers`), not
  `test_recovery_2`.

---

## 16. Resolved questions

There are no open questions blocking any release. The previous revision's "HTTP out of
scope" decision is superseded: the web surface is a scheduled release, not a maybe.

1. **Remote toolchain** (v4.0): a pre-provisioned host is the contract. `nix develop`
   integration is deferred past v5.0. `toolchain_hint` is recorded for diagnostics only, and
   import emits a `log` warning when the hint names a tool the target lacks. Rationale: the
   daemon is not a build orchestrator, and `nix develop` would need to wrap every tool call.
2. **CLI editing** (v1.0): a single-line prompt composer with history, plus an `$EDITOR`
   escape for multi-line input. Rationale: a full in-terminal editor is a project of its own,
   and users already have one.
3. **Checkpoint retention** (v1.0): 10 per session, pruned automatically. Answered by
   replacing stashes with private refs in §8.4, which makes pruning a single `update-ref -d`
   instead of index arithmetic on a shared stash list.
4. **Bundle transport** (v4.0): a separate `ssh <host> sa handoff --receive` invocation
   streaming raw bytes on stdin, not the JSON-RPC channel. Rationale: the RPC channel is
   newline-delimited JSON, so a binary bundle would need base64 and a framing exception for
   one message type. A second process costs nothing and keeps the protocol clean.
5. **Web surface** (v2.0): served by the daemon over loopback HTTP with token plus header
   authentication, TLS terminated by a reverse proxy. Rationale: HTTP is a passive carrier
   with no outbound connection or reconnect state, so it does not widen the daemon's failure
   domain the way a chat gateway would.
6. **Knowledge isolation** (v3.0): a namespace ACL enforced in the dispatcher, replacing the
   container-per-tenant pattern. Rationale: the Hermes deployment runs three containers to
   get three isolated memory stores; the same guarantee is a row in the session record and a
   check in the dispatcher, with one process to supervise instead of three.

---

## 17. Decision log

Settled decisions, in the order they were made. This section is the quick reference; the
normative text is in the sections above.

| # | Decision | Section |
| --- | --- | --- |
| 1 | daemon-owned state, disposable clients, event-sourced log | §2.4, §5.1 |
| 2 | sqlite at rest, bundle in transit, one write path each | §6.1, §6.2 |
| 3 | epoch fencing, never clocks | §5.3 |
| 4 | SSH stdio is a carrier; systemd owns daemon lifetime | §4.2, §12 |
| 5 | single protocol version integer, no negotiation | §3.1 |
| 6 | move by default, fork requires an explicit workspace | §5.5, §7.5 |
| 7 | deny on approval timeout | §8.3 |
| 8 | unattended handoff defaults to `allowlisted` | §8.1 |
| 9 | checkpoint before every unattended turn | §8.4 |
| 10 | at most one attached client per session; takeover evicts | §3.6 |
| 11 | session deletion is a tombstone, not a purge | §5.7 |
| 12 | workspace is exactly one git repo; remote optional | §7.1 |
| 13 | workspace lock keyed on git common directory | §7.4 |
| 14 | secrets refused at export, overridable and recorded | §7.6 |
| 15 | OpenAI-compatible wire only, one provider impl | §10.2 |
| 16 | automatic compaction at 80% of context limit | §10.4 |
| 17 | sequential tool calls | §9 |
| 18 | HTTP deferred, unscheduled | §4.3 |
| 19 | oversized payloads spill to blobs, flattened in the bundle | §6.3 |
| 20 | every `tool.requested` gets a terminal event, including on recovery | §5.2, §5.6 |
| 21 | pre-provisioned remote host; no `nix develop` integration | §16 |
| 22 | single-line composer plus `$EDITOR` escape | §16 |
| 23 | checkpoints are private refs, not stashes; 10 retained | §8.4 |
| 24 | bundles move over a dedicated `sa handoff --receive` process | §16 |
| 25 | the web surface is a scheduled release, served by the daemon over loopback HTTP | §4.3, §14 |
| 26 | knowledge isolation is a namespace ACL, not a process per tenant | §11.3 |
| 27 | knowledge is markdown on disk; the index is derived and rebuildable | §11.1, §11.2 |
| 28 | FTS5 only, no embeddings until keyword recall demonstrably fails | §11.2 |
| 29 | background review is opt-in and budgeted | §11.5 |
| 30 | the web surface drives detached turns and never attaches | §3.6, §4.3 |
| 31 | the CLI is the coding surface; no ratatui TUI is scheduled | §1, §14 |

---

## Appendix A — v1.0 implementation plan

Scope: `sa-proto` plus a daemon serving `hello` and `ping` over the unix socket, with
`sa rpc --stdio` proxying to it. No LLM, no database, no sessions, no web listener, no
systemd, no auto-spawn. This is the first task of v1.0 (§14), not the whole release.

`sa-web` is not created at v1.0; it arrives at v2.0. The dependency-rule check in A.2.6
treats an absent crate as passing.

### A.1 File layout

```text
Cargo.toml                       workspace, members, lints
rust-toolchain.toml
flake.nix                        devShell, checks
ci/check-deps.sh                 dependency-rule gate (§2.2)
crates/sa-proto/src/lib.rs
crates/sa-proto/src/rpc.rs       json-rpc 2.0 types
crates/sa-proto/src/framing.rs   line codec, 1 MiB cap
crates/sa-proto/src/error.rs     error codes
crates/sa-core/src/lib.rs
crates/sa-core/src/dispatcher.rs method dispatch, hello gate
crates/sa-core/src/peer.rs       uid check, pure
crates/sa-daemon/src/lib.rs
crates/sa-daemon/src/main.rs
crates/sa-daemon/src/socket.rs   bind, perms, accept
crates/sa-daemon/src/conn.rs     per-connection loop
crates/sa-client/src/lib.rs      connect, handshake, call
crates/sa/src/main.rs
crates/sa/src/cli.rs
crates/sa/src/proxy.rs
```

### A.2 Tasks

**A.2.1 Scaffold.** Workspace `Cargo.toml` with `[workspace.lints.clippy]
unwrap_used = "deny"`, `print_stdout`/`print_stderr` denied at workspace level and allowed
only in `sa` and `sa-daemon`. `flake.nix` devShell with rust toolchain, `cargo-deny`,
`marksman`. `.gitignore` with `/target` only: `sa` writes nothing else.

Dependencies, declared up front so `cargo deny` has something to check: `serde`,
`serde_json`, `tokio`, `thiserror`, `tracing`, `tracing-subscriber`, `clap`, `nix`.

Check: `cargo build --workspace` produces five empty crates.

**A.2.2 `sa-proto`.** JSON-RPC request/response/notification types, `PROTOCOL_VERSION: u32 =
1`, error code enum, line codec with a 1 MiB cap.

Error codes: standard range `-32700` parse, `-32600` invalid request, `-32601` method not
found, `-32602` invalid params, `-32603` internal. Application range from `-32000`:
`version_mismatch`, `peer_uid_mismatch`, `line_too_long`, `handshake_required`. The remaining
application codes (`session_busy`, `stale_owner`, `workspace_diverged`,
`workspace_unreachable`, `context_exhausted`, `approval_timeout`, `bundle_invalid`) are
declared now and unused until their milestone, because adding a variant later is a churn
across every match arm.

Tests: `round_trips_a_request`, `a_request_without_an_id_is_a_notification`,
`rejects_a_wrong_jsonrpc_member`, `rejects_a_missing_jsonrpc_member`.

`sa-proto` is serde-only (§2.1), so the line codec is not here. Both transports construct
`LinesCodec::new_with_max_length(MAX_LINE_LEN)` directly, and the framing tests live beside
the codec in `sa-daemon`.

**A.2.3 `sa-core`.** `Dispatcher` with `ping` and the `hello` gate. `peer::check_uid(expected,
actual) -> Result<(), Error>` as a pure function.

The handshake gate is stateful per connection: the first message must be `hello`, with a 5 s
deadline, and any other first message is `handshake_required`. `ping` returns
`{version, uptime, sessions}` with `sessions: 0` at v1.0.

Tests: `rejects_ping_before_hello`, `rejects_unknown_protocol_version_naming_both`,
`uid_check_rejects_mismatch`, `ping_reports_protocol_version`.

**A.2.4 `sa-daemon`.** Bind `$XDG_RUNTIME_DIR/sa/daemon.sock`, creating the directory at
`0700` and setting the socket to `0600` after bind. `SO_PEERCRED` via
`nix::sys::socket::getsockopt(sockopt::PeerCredentials)`, compared with `geteuid()`. Reject
mismatches by writing the error and closing, before reading any frame.

SIGTERM and SIGINT trigger a clean shutdown that stops accepting and drains connections. No
in-flight work exists at v1.0, so there is nothing to mark interrupted.

Tests (integration, `tests/`): `socket_is_0600_and_dir_is_0700`,
`serves_ping_over_socket`, `rejects_oversized_line_and_closes`,
`malformed_json_is_a_parse_error`, `refuses_to_bind_over_a_live_daemon`,
`shutdown_removes_the_socket`.

The uid-mismatch path cannot be exercised without a second uid, so it is covered by the unit
test in A.2.3 plus an assertion that the check is called on every accepted connection.

**A.2.5 `sa-client` and `sa` binary.** `sa-client` provides `Client::connect(path)`,
`handshake()`, and `call()`. The `sa` binary provides:

```text
sa daemon            foreground daemon
sa ping              connect, handshake, ping, print json
sa rpc --stdio       connect to the socket and relay bytes to stdin/stdout
```

`sa rpc --stdio` is a dumb relay: `tokio::io::copy` in both directions over the socket and
the stdio halves, with no protocol awareness. The daemon's `SO_PEERCRED` check sees the
proxy's own uid, which is the point.

Tests: `ping_cli_reports_version`, `stdio_proxy_serves_ping`,
`stdio_proxy_relays_errors_verbatim`, `stdio_proxy_exits_nonzero_when_daemon_absent`.

**A.2.6 Dependency-rule gate.** `ci/check-deps.sh` asserts:

```text
cargo tree -p sa-core   | grep -qE 'ratatui|crossterm|hyper|axum' -> must fail
cargo tree -p sa-proto  | grep -vE 'sa-proto|serde'              -> must be empty
cargo tree -p sa-web    -> skipped if the crate is absent
cargo tree -p sa-daemon | grep -q 'ratatui|crossterm'            -> must fail
```

Wired into `flake.nix` checks and run by the pre-commit gate.

### A.3 Acceptance

v1.0 is done when:

1. `cargo test --workspace` is green, including every test named above.
2. `ci/check-deps.sh` passes.
3. `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`,
   `cargo deny check` all pass.
4. A raw client driven by `nc -U` against the socket gets a valid `hello`/`ping` exchange.
5. The same exchange through `ssh localhost sa rpc --stdio` works, proving the carrier is
   disposable.
