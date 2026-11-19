# Agent hooks

`hypha_hooks.py` holds the rules every coding agent in this repository runs under.
Each agent's hook configuration calls it with that agent's hook JSON on stdin.
Test: `python3 -I scripts/agent-hooks/test_hooks.py`.

| Agent | Configuration | State |
|---|---|---|
| Claude Code | `.claude/settings.json` | tested |
| Codex CLI | `.codex/hooks.json` | written from codex-docs.com/en/docs/hooks (a mirror of the Codex docs), untested |
| GitHub Copilot | `.github/hooks/hypha.json` | written from docs.github.com/en/copilot/reference/hooks-reference, untested |
| Gemini CLI | `.gemini/settings.json` | written from geminicli.com/docs/hooks, untested |
| Antigravity | `.agents/hooks.json` | written from antigravity.google/docs/hooks, untested |
| every agent, every human | `.githooks/pre-commit` | tested |

If you use one of the untested agents, check its hooks fire (`/hooks` in Codex,
Gemini and Antigravity) and fix the adapter or the engine's `from_<agent>` function.
The vendors' payload field names are the likely place to need a fix.

## What the hooks do

- **Before every tool call:**
  - Block any path outside the repository, unless it is built in or listed in
    `.agent-allowed-paths` (committed) or `.agent-allowed-paths.local` (personal,
    git-ignored).
  - Block edits to the files that make up the fence: the allowlists, the five
    adapter files, `.githooks/` and this directory. The copies in a worktree under
    `.claude/worktrees/` are protected as well.
  - Block these commands: `git push`, `stash`, `reset --hard`; setting
    `core.hooksPath`.
  - Block what discards work or rewrites history: `git clean`, `git restore`
    and `git checkout -- <path>` of the working tree (`restore --staged` is
    allowed), `rebase`, `branch -D`, `commit --no-verify` and `commit --amend`.
  - Block installs and system changes: `cargo install`, `pip install`,
    `npm -g`, `rustup` beyond its read-only subcommands, `cargo +<toolchain>`
    for a toolchain that is not installed (cargo would install it), `sudo` and
    the package managers, a download piped into an interpreter.
  - Block `cargo update` without `-p`, `cargo publish`, and `pkill`/`killall`
    (a name could match the owner's running stack; kill by pid).
  - Block `clean-shm.sh --force` and `test.sh` while a hypha stack is running:
    both clear its shared memory. A stack is running when `hypha launch` runs or any
    process has `libhypha_ffi` mapped (read from `/proc`); no application is named.
  - For subagents only: block `cargo clean` and deleting `target/` or `build/`
    (the main session frees the disk when it decides to); block `git commit`, `switch`, branch `checkout`, `merge`,
    `branch` (but listing), `tag`, `cherry-pick`, `revert`, and edits of the
    `hypha-audit-*` files. The main session may do these.
- **After an edit of `Cargo.toml` or `clippy.toml`:** report any added line that
  sets a lint to `allow` or `warn`.
- **After an edit of a `.rs` file:** run `rustfmt` on the file, then report any
  added line with `#[allow]` without a reason, `unsafe {` without a
  `// SAFETY:` comment, or `unwrap()`/`expect(` in shipped code.
- **When a subagent hands back** (Claude Code and Codex): refuse while
  `cargo clippy` is red on the crates changed in the working tree, three times
  at most.

## Limits

- **Fail open:** a hook that fails on its own error lets the call through and
  prints a warning to stderr.
- **Files, not the network:** the fence covers paths. `curl`, `git fetch` and the
  web tools of an agent are not blocked, except a download piped into an interpreter.
- **Accidents only:** the hooks catch accidents, not an agent that hides a path
  in a variable or `$(...)`. AGENTS.md stays the rule.
- **Bus names are not paths:** an absolute word whose first component does not
  exist under `/` (`/clock`, `/fusion/sensor/imu`) is a topic or node name.
- **Data is not a command:** the command rules read the words of each simple
  command (`bash -c '...'` is unpacked). Quoted arguments and heredoc bodies are
  data, so a commit message or a heredoc that mentions `git push` passes. A
  program inside a heredoc (a Python script that calls `git push`) is not checked.
- **Copilot's cloud agent** reads hooks from the default branch only.
- **Codex** runs a hook only after it has been trusted in `/hooks`.
- **Antigravity's** allow-all/bypass mode switches hooks off: do not use it here.
