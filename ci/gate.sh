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

step "supply chain"
cargo deny check

printf '\ngate: ok\n'
