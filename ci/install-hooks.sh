#!/usr/bin/env bash
# points git at the repository's hooks. run once per clone.
set -euo pipefail
cd "$(dirname "$0")/.."
git config core.hooksPath ci/hooks
echo "installed: core.hooksPath -> ci/hooks"
