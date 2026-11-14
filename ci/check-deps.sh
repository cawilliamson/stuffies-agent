#!/usr/bin/env bash
# dependency-rule gate. see SPEC.md 2.2.
set -euo pipefail

fail() { echo "check-deps: $*" >&2; exit 1; }

# sa-core must not pull in a terminal crate. it does use reqwest (and so hyper) for the
# model client; the ban on http *servers* belongs to sa-web at v2.0.
if cargo tree -p sa-core | grep -qE 'ratatui|crossterm'; then
  fail "sa-core depends on a terminal crate"
fi

# sa-proto must stay serde-only: no workspace crates, no tokio, no terminal, no http.
# a deny-list, not an allow-list: serde's own transitive tree is not our business.
if cargo tree -p sa-proto --edges normal | tail -n +2 \
    | grep -qE 'sa-(core|daemon|client) v|tokio v|ratatui|crossterm|hyper v|axum'; then
  cargo tree -p sa-proto --edges normal
  fail "sa-proto depends on a workspace crate, tokio, a terminal or an http stack"
fi

# sa-daemon must not depend on a terminal crate.
if cargo tree -p sa-daemon | grep -qE 'ratatui|crossterm'; then
  fail "sa-daemon depends on a terminal crate"
fi

# sa-web is optional until v2.0.
if cargo metadata --no-deps --format-version 1 | grep -q '"name":"sa-web"'; then
  if cargo tree -p sa-web | grep -qE 'sa-daemon|ratatui|crossterm'; then
    fail "sa-web depends on sa-daemon or a terminal crate"
  fi
fi

echo "check-deps: ok"
