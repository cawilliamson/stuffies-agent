# stuffies-agent

A daemon-first coding agent. The daemon owns session state; the terminal client, one-shot
CLI and web surface are disposable clients of it. The design lives in `SPEC.md`.

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
