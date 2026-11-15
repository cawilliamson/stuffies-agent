#!/usr/bin/env bash
# test-driven-development gate.
#
# a green test suite only proves tests run. mutation testing proves they assert: it
# rewrites the source in small ways and fails if no test notices. that is the closest a
# build can get to enforcing test-first.
set -euo pipefail

if ! command -v cargo-mutants >/dev/null 2>&1; then
  echo "tdd: cargo-mutants not found. run inside the devShell:" >&2
  echo "     nix develop -c ./ci/tdd.sh" >&2
  exit 1
fi

# scope: the crates that carry logic.
exec cargo mutants \
  --package sa-core \
  --package sa-tui \
  --package sa-proto \
  --timeout 120 \
  "$@"
