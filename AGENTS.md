# stuffies-agent

A daemon-first coding agent. The daemon owns session state; the terminal client, one-shot
CLI and web surface are disposable clients of it. The behaviour lives in `specs/`, the
reasoning in `docs/design.md`, and the open work in `docs/changes/`.

## the spec is executable

`specs/` holds Gherkin, and `crates/sa-core/tests/specs.rs` runs it under `cargo test` with
`fail_on_skipped`. A scenario whose steps are not written is a skipped step, and a skipped
step is a failure: a claim nothing implements cannot pass. There is no prose specification
beside the code, because a second statement of the same behaviour is a second thing to keep
true.

`@allow.skipped` is the one exception, and it is a debt marker rather than a permission: it
says a scenario is written and not yet executed. The tag is removed when its steps land.

What that means when changing behaviour:

- one requirement per behaviour a test could name, and scenarios that describe an observable
  wire or filesystem effect rather than an intention
- change the scenario in the same turn as the code, or the build is red
- `./ci/gate.sh` before calling a change done, and again against `sa daemon` rather than only
  `sa --standalone`

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

## the workspace is the user's

A turn leaves no traces in the directory it works on. Everything the agent writes for its
own bookkeeping — event log, checkpoints, locks, caches, logs, scratch files — lives under
`$XDG_STATE_HOME/stuffies-agent` and `$XDG_RUNTIME_DIR`. Nothing of ours belongs in the
workspace, and the workspace's `.git` is part of the workspace.

The only things that appear in a workspace are the changes the turn was asked to make: the
files it edited, and the files its tools created on purpose. Anything else is a trace, and a
trace is a bug, not a detail.

The line is between what the agent writes and what the user's own commands write. Build
output from a command a turn runs belongs to the toolchain that produced it, and a project's
ignore file already covers it. What the agent must never do is add a cache, a marker or a
directory of its own.

This is why the agent has no permission system (spec 8) and why the one guard it keeps is a
checkpoint: the user's defence against a bad turn is that their directory is exactly as they
left it, plus the edits they wanted. A recovery point that lives in their `.git` is not that.

## the daemon runs on its own

Nothing of ours may need an external program to work. The token is read from its file in
rust and the tools are built in. A language server, an MCP server, the shell the agent's own
commands run in and `git` for the checkpoint store are the exceptions, because they are the
things the agent is fronting rather than utilities it depends on.

bash is the fallback, not the default. A built-in tool is bounded, structured and needs
nothing installed; the shell is for what no built-in tool does, such as running a project's
tests. When a new need appears, the answer is a tool, not a command line.

## ordering

Alphabetical, everywhere a collection is written down: tool specs, slash commands, help
text, enum variants, match arms over a fixed set, config keys, imports, `specs` listings,
test fixtures. If a list can be sorted without changing behaviour, it is sorted.

The exceptions are the ones where order carries meaning, and they are narrow: a wire
protocol that defines the sequence, a dependency's own required order, a ranking or a
time series, and a list the reader is meant to follow step by step. "It reads better this
way" is not one of them.

## testing

`sa --standalone <command>` builds the backend into the front-end process, so a change can be
exercised end to end without a daemon: no socket, no database, nothing to restart between
builds. Use it to iterate quickly and for one-off checks of a client path.

It is opt-in, never the default and never a fallback. A command that needs a daemon still
fails without one; `--standalone` is the thing you ask for.

It is also not the deployment standard. The supported shape is the daemon plus a disposable
client, and standalone shares its startup with it, so behaviour that only works in one of
them is a bug. Test daemon mode too before calling a change done: start `sa daemon`, attach a
client to it, and confirm the same behaviour over the socket.
