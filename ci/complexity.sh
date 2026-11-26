#!/usr/bin/env bash
# fails when a function's cognitive complexity exceeds the budget.
#
# complexity is the one maintainability signal nothing else in the gate reports. the
# allowlist is acknowledged debt: each entry should shrink or disappear, never grow.
set -euo pipefail
cd "$(dirname "$0")/.."

budget=50
allowlist="sa-tui/src/app.rs::apply_event sa-tui/src/lib.rs::run sa-core/src/llm.rs::stream"

tmp=$(mktemp)
trap 'rm -f "$tmp"' EXIT

if ! command -v rust-code-analysis-cli >/dev/null 2>&1; then
  echo "complexity: rust-code-analysis-cli not found. run inside the devShell:" >&2
  echo "            nix develop -c ./ci/gate.sh" >&2
  exit 1
fi

rust-code-analysis-cli -m -p crates -O json > "$tmp"
python3 - "$tmp" "$budget" "$allowlist" <<'PY'
import json
import sys

path, budget, allow_raw = sys.argv[1], float(sys.argv[2]), sys.argv[3]
allowed = set(allow_raw.split())

def metric(node, *keys):
    for key in keys:
        if not isinstance(node, dict):
            return None
        node = node.get(key)
    return node if isinstance(node, (int, float)) else None

offenders = []
for line in open(path):
    line = line.strip()
    if not line:
        continue
    unit = json.loads(line)
    source = unit.get("name", "")
    short = source.split("crates/")[-1] if "crates/" in source else source

    def walk(node):
        if not isinstance(node, dict):
            return
        if node.get("kind") == "function":
            cog = metric(node, "metrics", "cognitive", "sum")
            if cog is not None and cog > budget:
                key = f"{short}::{node.get('name')}"
                if key not in allowed:
                    offenders.append((cog, key, node.get("start_line")))
        for space in node.get("spaces") or []:
            walk(space)

    walk(unit)

if offenders:
    offenders.sort(reverse=True)
    print(f"complexity: {len(offenders)} function(s) exceed {budget:.0f}", file=sys.stderr)
    for cog, key, line in offenders:
        print(f"  {cog:.0f}  {key} (line {line})", file=sys.stderr)
    sys.exit(1)
print(f"complexity: ok (budget {budget:.0f})")
PY
