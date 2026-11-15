#!/usr/bin/env bash
# the pre-commit gate: every check the project requires, in one command.
#
# run it before committing. a red gate means do not commit.
set -euo pipefail
cd "$(dirname "$0")/.."

step() { printf '\n==> %s\n' "$1"; }

step "format"
cargo fmt --check

step "clippy"
cargo clippy --all-targets --all-features --locked -- -D warnings

step "test"
cargo test --workspace --locked

step "dependencies"
./ci/check-deps.sh

step "unused dependencies"
if ! command -v cargo-machete >/dev/null 2>&1; then
  echo "gate: cargo-machete not found. run inside the devShell:" >&2
  echo "      nix develop -c ./ci/gate.sh" >&2
  exit 1
fi
cargo machete

step "anti-patterns"
if ! command -v ast-grep >/dev/null 2>&1; then
  echo "gate: ast-grep not found. run inside the devShell:" >&2
  echo "      nix develop -c ./ci/gate.sh" >&2
  exit 1
fi
ast-grep scan --config sgconfig.yml

step "supply chain"
cargo deny check

printf '\ngate: ok\n'
