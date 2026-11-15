# sa

A daemon-first coding agent. A long-lived daemon owns session state; the terminal client,
one-shot CLI and web surface are disposable clients of it.

The design lives in `SPEC.md`. Build with `cargo build`, test with `cargo test`.

Configuration is daemon-local under `$XDG_CONFIG_HOME/sa`; nothing is written into a
workspace.
