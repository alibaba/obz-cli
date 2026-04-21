#!/usr/bin/env bash
# Configure git to use the committed .githooks/ directory.
# Run once after cloning: ./scripts/install-hooks.sh
#
# This sets core.hooksPath in the repo-local git config (.git/config).
# In a git worktree, the local config is shared across all worktrees of
# the same repository, so this only needs to be run once per checkout.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
HOOK_FILE="$REPO_ROOT/.githooks/pre-commit"

if [ ! -f "$HOOK_FILE" ]; then
    echo "ERROR: Missing hook file: $HOOK_FILE"
    echo "       Make sure you have the latest code (git pull)."
    exit 1
fi

if [ ! -x "$HOOK_FILE" ]; then
    echo "WARN: Hook file is not executable, fixing permissions..."
    chmod +x "$HOOK_FILE"
fi

git -C "$REPO_ROOT" config core.hooksPath .githooks

echo "Done. git hooks configured:"
echo "  core.hooksPath = .githooks  (repo-local, written to .git/config)"
echo ""
echo "The pre-commit hook enforces:"
echo "  - cargo fmt --check  (code formatting)"
echo "  - cargo clippy -D warnings  (lint, zero warnings)"
echo "  - No todo!() / unimplemented!() / dbg!() in staged .rs files"
