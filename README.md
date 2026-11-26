# sa

A daemon-first coding agent. A long-lived daemon owns session state; the terminal client,
one-shot CLI and web surface are disposable clients of it.

Behaviour lives in `openspec/specs/` and is the source of truth for what `sa` does. The
reasoning behind it is in `docs/design.md`, and the work not yet built is in
`openspec/changes/`. Check a change with `openspec validate`.

Build with `cargo build`, test with `cargo test`, and run the full gate with `./ci/gate.sh`.

Configuration is daemon-local under `$XDG_CONFIG_HOME/stuffies-agent`; nothing is written into a
workspace.
