#!/usr/bin/env bash
# dependency-rule gate. see specs/engineering-constraints.feature.
set -euo pipefail

fail() { echo "check-deps: $*" >&2; exit 1; }

# every check below shells out to cargo, and a missing cargo makes each `if ... | grep`
# evaluate false rather than fail. refuse to report ok without a toolchain.
command -v cargo >/dev/null 2>&1 || fail "cargo not found; run inside the devshell"

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

# sa-tui is a view: it must not reach the daemon or the engine directly.
if cargo metadata --no-deps --format-version 1 | grep -q '"name":"sa-tui"'; then
  if cargo tree -p sa-tui | grep -qE 'sa-daemon|sa-core'; then
    fail "sa-tui depends on sa-daemon or sa-core"
  fi
fi

# sa-web is optional until v2.0.
if cargo metadata --no-deps --format-version 1 | grep -q '"name":"sa-web"'; then
  if cargo tree -p sa-web | grep -qE 'sa-daemon|ratatui|crossterm'; then
    fail "sa-web depends on sa-daemon or a terminal crate"
  fi
fi

echo "check-deps: ok"
