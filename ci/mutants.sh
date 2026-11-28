#!/usr/bin/env bash
# assertion gate: a test that runs but asserts nothing still passes.
#
# cargo-mutants rewrites the source in small ways and fails when no test notices, which is the
# only check that a scenario actually verifies what it names. scoping it to the diff keeps it
# bounded; the gate itself runs `cargo test`, so the spec suite has already executed by the
# time this runs.
#
# usage:
#   ci/mutants.sh                 mutate what is staged, for the pre-commit hook
#   ci/mutants.sh <base-ref>      mutate what the branch changed, for ci
set -euo pipefail
cd "$(dirname "$0")/.."

base="${1:-}"
diff=$(mktemp)
trap 'rm -f "$diff"' EXIT

if [ -z "$base" ]; then
  git diff --cached -U0 >"$diff"
else
  git diff "$base"...HEAD -U0 >"$diff"
fi

if [ ! -s "$diff" ]; then
  echo "mutants: nothing changed, nothing to mutate"
  exit 0
fi

if ! command -v cargo-mutants >/dev/null 2>&1; then
  echo "mutants: cargo-mutants not found. run inside the devShell:" >&2
  echo "         nix develop -c ./ci/mutants.sh" >&2
  exit 1
fi

# the crates that carry logic. a mutant in a crate nothing tests is noise, not a finding.
exec cargo mutants \
  --in-diff "$diff" \
  --package sa-core \
  --package sa-proto \
  --package sa-tui \
  --timeout 120
