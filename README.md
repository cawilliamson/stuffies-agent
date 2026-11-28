# sa

A daemon-first coding agent. A long-lived daemon owns session state; the terminal client,
one-shot CLI and web surface are disposable clients of it.

Behaviour lives in `specs/` and is the source of truth for what `sa` does. The scenarios
there are executed by `cargo test`, so a claim the code does not meet is a red build rather
than a document nobody re-reads. The reasoning behind it is in `docs/design.md`, and the work
not yet built is in `docs/changes/`.

Build with `cargo build`, test with `cargo test`, and run the full gate with `./ci/gate.sh`.

Configuration is daemon-local under `$XDG_CONFIG_HOME/stuffies-agent`; nothing is written into a
workspace.
