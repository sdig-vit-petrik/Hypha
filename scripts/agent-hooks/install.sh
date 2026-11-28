#!/usr/bin/env bash
# usage: ./scripts/agent-hooks/install.sh
#
# Installs the repository's unified coding-agent configuration (Claude Code, Codex,
# Gemini CLI, Antigravity, GitHub Copilot) as gitignored symlinks into the checkout
# and enables .githooks/. Run once per checkout or worktree.
set -euo pipefail

repo="$(cd "$(dirname "$0")/../.." && pwd)"
overlay="$repo/scripts/agent-hooks/overlay"

while IFS= read -r -d '' src; do
  rel="${src#"$overlay"/}"
  dst="$repo/$rel"
  mkdir -p "$(dirname "$dst")"
  ln -sfn "$src" "$dst"
done < <(find "$overlay" -type f -print0)

ln -sfn AGENTS.md "$repo/CLAUDE.md"
git -C "$repo" config core.hooksPath .githooks

echo "OK: agent configs linked into $repo and core.hooksPath set to .githooks."
