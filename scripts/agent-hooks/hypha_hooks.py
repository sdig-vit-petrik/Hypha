#!/usr/bin/env python3
"""The rules every coding agent in this repository runs under.

One engine, five thin adapters. Each agent's hook configuration calls

    python3 -I scripts/agent-hooks/hypha_hooks.py --agent <agent> --event <event>

with the agent's hook JSON on stdin. The engine turns that JSON into one
`Call`, applies the rules below, and answers in the agent's own format.

Agents:  claude (tested), codex, copilot, gemini, antigravity (written from the
         vendors' docs, untested here; a user of that agent verifies and fixes).
Events:  pre-tool   before a tool call: may block it
         post-edit  after a file edit: rustfmt, then checks on the added lines
         stop       a subagent hands back: refuse while clippy is red

A hook that fails on its own (bad JSON, an exception) lets the call through and
prints a warning to stderr: a broken hook must not lock every session. A matched
rule blocks.

The hooks catch accidents, not a hostile agent: a shell command can hide a path
in a variable. AGENTS.md stays the rule.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shlex
import subprocess
import sys
from dataclasses import dataclass, field
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
HOME = Path.home()
ALLOWLIST_FILES = (REPO / ".agent-allowed-paths", REPO / ".agent-allowed-paths.local")

# Paths an agent may read and write outside the repository.
BUILTIN_READ_WRITE = (
    Path("/tmp"),
    Path("/dev/shm"),
    Path("/dev/null"),
    Path("/dev/stdin"),
    Path("/dev/stdout"),
    Path("/dev/stderr"),
    Path("/dev/tty"),
    # This repository's Claude Code memory.
    HOME / ".claude/projects" / str(REPO).replace("/", "-") / "memory",
)
# Paths an agent may only read: the system and the Rust toolchain.
BUILTIN_READ_ONLY = (
    Path("/usr"),
    Path("/bin"),
    Path("/sbin"),
    Path("/lib"),
    Path("/lib64"),
    Path("/proc"),
    Path("/sys"),
    Path("/etc"),
    HOME / ".cargo",
    HOME / ".rustup",
)

# Files that make up the fence. No agent edits them; the owner does.
PROTECTED = (
    ".agent-allowed-paths",
    ".agent-allowed-paths.local",
    ".claude/settings.json",
    ".claude/settings.local.json",
    ".codex/hooks.json",
    ".codex/config.toml",
    ".github/hooks",
    ".gemini/settings.json",
    ".agents/hooks.json",
    ".githooks",
    "scripts/agent-hooks",
)

# The command rules look at the words of each simple command, never at the raw
# text, so a rule's words inside a quoted message or a heredoc do not trigger it.
# Scripts that clear the shared memory of every hypha process on this host.
STACK_KILLERS = {"test.sh"}
GIT_RULES = {
    "push": "agents never push; the owner does",
    "stash": "agents never stash: it hides the owner's uncommitted work",
    "clean": "agents never run git clean: it deletes untracked work. Ask the owner",
    "rebase": "agents never rewrite history; the owner does",
}
# What the main session may do and a subagent may not: the branch and its history
# belong to the session the owner talks to.
SUBAGENT_GIT_RULES = {
    "commit": "subagents do not commit; hand back and the orchestrator commits",
    "switch": "subagents stay on the current branch",
    "merge": "subagents do not merge; the orchestrator does",
    "branch": "subagents do not create, delete or rename branches",
    "tag": "subagents do not tag",
    "cherry-pick": "subagents do not change history; the orchestrator does",
    "revert": "subagents do not change history; the orchestrator does",
}
# System and package managers: nothing is installed and the system is the owner's.
SYSTEM_PROGRAMS = {
    "sudo",
    "doas",
    "su",
    "apt",
    "apt-get",
    "dpkg",
    "dnf",
    "yum",
    "pacman",
    "zypper",
    "snap",
    "flatpak",
    "brew",
}
# `rustup` subcommands that only read.
RUSTUP_READ_ONLY = {"show", "which", "run", "doc", "help", "--version", "-V"}
DOWNLOADERS = {"curl", "wget"}
INTERPRETERS = {"bash", "sh", "zsh", "dash", "python", "python3", "perl", "ruby", "node"}
# Words that run the next word as the command.
PREFIXES = {"env", "time", "nohup", "command", "exec", "nice", "timeout", "xargs"}
SHELLS = {"bash", "sh", "zsh", "dash"}
WRITERS = {"tee", "rm", "mv", "cp", "chmod", "truncate", "ln", "install", "dd", "touch"}
SEPARATORS = {";", "&&", "||", "|", "&", "(", ")", "\n", "|&"}
REDIRECTS = {">", ">>", "&>", ">|"}
HEREDOC = re.compile(
    r"<<-?\s*(['\"]?)([A-Za-z_][A-Za-z0-9_]*)\1[^\n]*\n(.*?\n)?\s*\2[ \t]*(\n|$)", re.S
)
SUBAGENT_ONLY_FILES = re.compile(r"^(hypha-audit-|hypha-design-audit-)")
# A shell word that is plainly a path: no quotes, braces, spaces or regex syntax.
PATH_WORD = re.compile(r"^[~/.][A-Za-z0-9._+@%,:=/-]*$")


@dataclass
class Call:
    """One tool call, whatever agent made it."""

    kind: str  # "bash", "read", "search", "edit", "other"
    command: str = ""
    paths: list[str] = field(default_factory=list)
    cwd: Path = REPO
    is_subagent: bool = False
    added_text: str | None = None  # for an edit: the text it inserted, when known


class Blocked(Exception):
    """A rule matched; the message goes back to the agent."""


# --- normalisation, one function per agent -------------------------------------------


def _paths_from_args(args: dict) -> list[str]:
    """Every argument whose name says it is a path (documented-only adapters)."""
    found = []
    for key, value in args.items():
        named_path = any(word in key.lower() for word in ("path", "file", "target", "dir", "cwd"))
        if named_path and isinstance(value, str) and value:
            found.append(value)
    return found


def from_claude(payload: dict) -> Call:
    tool = payload.get("tool_name", "")
    args = payload.get("tool_input") or {}
    call = Call(kind="other", cwd=Path(payload.get("cwd") or REPO))
    # agent_id is present inside a subagent (and with `--agent`, which this repo
    # does not use for the owner's session).
    call.is_subagent = bool(payload.get("agent_id"))
    if tool == "Bash":
        call.kind = "bash"
        call.command = args.get("command", "")
    elif tool in ("Read", "NotebookRead"):
        call.kind = "read"
        call.paths = [args.get("file_path", "")]
    elif tool in ("Grep", "Glob"):
        call.kind = "search"
        call.paths = [args.get("path") or str(call.cwd)]
        pattern = args.get("pattern", "")
        if pattern.startswith(("/", "~")):
            call.paths.append(pattern.split("*", 1)[0] or "/")
    elif tool in ("Edit", "MultiEdit", "Write", "NotebookEdit"):
        call.kind = "edit"
        call.paths = [args.get("file_path") or args.get("notebook_path", "")]
        if tool == "Edit":
            call.added_text = args.get("new_string")
        elif tool == "MultiEdit":
            call.added_text = "\n".join(e.get("new_string", "") for e in args.get("edits", []))
    elif tool == "LSP":
        call.kind = "read"
        call.paths = [args.get("filePath", "")]
    call.paths = [p for p in call.paths if p]
    return call


def from_codex(payload: dict) -> Call:
    # Documented-only: https://www.codex-docs.com/en/docs/hooks (tool_name, tool_input).
    tool = payload.get("tool_name", "")
    args = payload.get("tool_input") or {}
    call = Call(kind="other", cwd=Path(payload.get("cwd") or REPO))
    if tool in ("Bash", "shell", "local_shell"):
        command = args.get("command", "")
        call.kind = "bash"
        call.command = shlex.join(command) if isinstance(command, list) else str(command)
    elif tool.startswith("apply_patch"):
        call.kind = "edit"
        patch = str(args.get("input") or args.get("patch") or "")
        call.paths = re.findall(r"^\*\*\* (?:Add|Update|Delete) File: (.+)$", patch, re.M)
        call.added_text = "\n".join(line[1:] for line in patch.splitlines() if line.startswith("+"))
    else:
        call.paths = _paths_from_args(args)
    return call


def from_copilot(payload: dict) -> Call:
    # Documented-only: https://docs.github.com/en/copilot/reference/hooks-reference
    # (camelCase events: toolName, toolArgs as a JSON string).
    tool = payload.get("toolName", "")
    args = payload.get("toolArgs") or {}
    if isinstance(args, str):
        args = json.loads(args or "{}")
    call = Call(kind="other", cwd=Path(payload.get("cwd") or REPO))
    if tool in ("bash", "shell", "powershell"):
        call.kind = "bash"
        call.command = args.get("command", "")
    elif tool in ("edit", "create", "write"):
        call.kind = "edit"
        call.paths = _paths_from_args(args)
        call.added_text = args.get("new_str") or args.get("file_text") or args.get("content")
    else:
        call.kind = "read"
        call.paths = _paths_from_args(args)
    return call


def from_gemini(payload: dict) -> Call:
    # Documented-only: https://geminicli.com/docs/hooks/ (tool_name, tool_input).
    tool = payload.get("tool_name", "")
    args = payload.get("tool_input") or {}
    call = Call(kind="other", cwd=Path(payload.get("cwd") or os.environ.get("GEMINI_CWD") or REPO))
    if tool == "run_shell_command":
        call.kind = "bash"
        call.command = args.get("command", "")
        if args.get("directory"):
            call.paths = [args["directory"]]
    elif tool in ("write_file", "replace"):
        call.kind = "edit"
        call.paths = _paths_from_args(args)
        call.added_text = args.get("new_string") or args.get("content")
    else:
        call.kind = "read"
        call.paths = _paths_from_args(args)
    return call


def from_antigravity(payload: dict) -> Call:
    # Documented-only: https://antigravity.google/docs/hooks (toolCall {name, args}).
    tool_call = payload.get("toolCall") or {}
    tool = tool_call.get("name", "")
    args = tool_call.get("args") or {}
    workspaces = payload.get("workspacePaths") or [str(REPO)]
    call = Call(kind="other", cwd=Path(args.get("Cwd") or workspaces[0]))
    if tool == "run_command":
        call.kind = "bash"
        call.command = args.get("CommandLine", "")
    elif tool.startswith(("write_", "replace_", "edit_")):
        call.kind = "edit"
        call.paths = _paths_from_args(args)
        call.added_text = args.get("CodeContent") or args.get("ReplacementContent")
    else:
        call.kind = "read"
        call.paths = _paths_from_args(args)
    return call


NORMALISE = {
    "claude": from_claude,
    "codex": from_codex,
    "copilot": from_copilot,
    "gemini": from_gemini,
    "antigravity": from_antigravity,
}


# --- the path fence --------------------------------------------------


def allowlist() -> list[Path]:
    entries = []
    for listing in ALLOWLIST_FILES:
        if not listing.is_file():
            continue
        for line in listing.read_text().splitlines():
            entry = line.split("#", 1)[0].strip()
            if entry:
                entries.append(Path(os.path.expanduser(entry)).resolve())
    return entries


def resolve(path: str, cwd: Path) -> Path:
    expanded = Path(os.path.expanduser(path))
    absolute = expanded if expanded.is_absolute() else cwd / expanded
    return absolute.resolve()


def under(path: Path, roots) -> bool:
    return any(path == root or root in path.parents for root in roots)


def outside_fence(path: Path, writes: bool) -> bool:
    allowed = [REPO, *BUILTIN_READ_WRITE, *allowlist()]
    if not writes:
        allowed.extend(BUILTIN_READ_ONLY)
    return not under(path, allowed)


def strip_heredocs(command: str) -> str:
    """The command without the bodies of its heredocs (they are data, not commands)."""
    return HEREDOC.sub("\n", command)


def shell_words(command: str) -> list[str]:
    """The words of a shell command; separators and redirections are words of their own."""
    text = strip_heredocs(command)
    try:
        lexer = shlex.shlex(text, posix=True, punctuation_chars=";&|<>()\n")
        lexer.whitespace = " \t\r"
        lexer.whitespace_split = True
        words = list(lexer)
    except ValueError:
        words = text.split()
    # shlex groups runs of punctuation (`;\n`, `)\n`); split those back into separators.
    result = []
    for word in words:
        is_run = len(word) > 1 and set(word) <= set(";&|()\n") and word not in SEPARATORS
        result.extend(list(word) if is_run else [word])
    return result


def simple_commands(command: str) -> list[list[str]]:
    """Each simple command as its words, with `env`/`time`/... prefixes and variable
    assignments dropped and `bash -c '...'` unpacked into its own commands."""
    commands = []
    current: list[str] = []
    for word in shell_words(command) + [";"]:
        if word not in SEPARATORS:
            current.append(word)
            continue
        while current and (current[0] in PREFIXES or re.match(r"^[A-Za-z_]\w*=", current[0])):
            current = current[1:]
        unpack = len(current) >= 3 and Path(current[0]).name in SHELLS and current[1] == "-c"
        if unpack:
            commands.extend(simple_commands(current[2]))
        elif current:
            commands.append(current)
        current = []
    return commands


def redirect_targets(command: str) -> list[str]:
    words = shell_words(command)
    return [words[i + 1] for i, word in enumerate(words[:-1]) if word in REDIRECTS]


def shell_paths(command: str) -> list[str]:
    """The words of a shell command that are plainly paths (`/x`, `~/x`, `../x`)."""
    found = []
    for word in shell_words(command):
        # `--flag=/path` and `VAR=/path`: the part after the last `=` is the path.
        candidate = word.rsplit("=", 1)[-1] if "=" in word else word
        is_path = PATH_WORD.match(candidate) is not None
        looks_absolute = candidate.startswith(("/", "~"))
        climbs = candidate.startswith("..")
        if is_path and (looks_absolute or climbs) and not is_bus_name(candidate):
            found.append(candidate)
    return found


def is_bus_name(word: str) -> bool:
    """`/clock`, `/fusion/sensor/imu`: an absolute word whose first component does not
    exist under `/` is a topic or node name, not a file. Nothing an agent runs can
    create a directory there."""
    if not word.startswith("/") or word == "/":
        return False
    first = word.split("/", 2)[1]
    return bool(first) and first not in (".", "..") and not (Path("/") / first).exists()


def check_fence(call: Call) -> None:
    candidates = list(call.paths)
    if call.kind == "bash":
        candidates.extend(shell_paths(call.command))  # heredoc bodies are stripped there
    writes = call.kind == "edit"
    for raw in candidates:
        path = resolve(raw, call.cwd)
        if outside_fence(path, writes):
            raise Blocked(
                f"{path} is outside the repository. Agents stay inside {REPO} unless the "
                f"owner lists the path in .agent-allowed-paths(.local). Ask the owner."
            )


def checkouts(path: Path) -> list[Path]:
    """The repository, and the worktree under it that holds `path`: a worktree has a
    copy of every fence file, and a commit made there carries it to the branch."""
    worktrees = REPO / ".claude" / "worktrees"
    if worktrees not in path.parents:
        return [REPO]
    name = path.relative_to(worktrees).parts[0]
    return [REPO, worktrees / name]


def check_protected(call: Call) -> None:
    targets = []
    if call.kind == "edit":
        targets = [resolve(p, call.cwd) for p in call.paths]
    elif call.kind == "bash":
        targets = [resolve(t, call.cwd) for t in redirect_targets(call.command)]
        for words in simple_commands(call.command):
            writes = (
                Path(words[0]).name in WRITERS
                or words[:2] == ["sed", "-i"]
                or (words[0] == "sed" and any(w.startswith("-i") for w in words[1:]))
            )
            if writes:
                targets.extend(resolve(w, call.cwd) for w in words[1:] if not w.startswith("-"))
    for target in targets:
        if under(target, [root / p for root in checkouts(target) for p in PROTECTED]):
            raise Blocked(
                f"{target.relative_to(REPO)} is part of the agent fence; only the owner edits it."
            )


# --- the command rules ---------------------------------------------------


def maps_library(maps: Path, library: str) -> bool:
    """Whether the process behind this `/proc/<pid>/maps` file has `library` mapped."""
    try:
        return library in maps.read_text(errors="replace")
    except OSError:  # the process ended, or it is another user's
        return False


def live_stack(proc: Path = Path("/proc")) -> bool:
    """Whether a hypha system runs on this host: `hypha launch`, or any process that has
    the C ABI library mapped. No application is named: every node loads libhypha_ffi."""
    launched = subprocess.run(
        ["pgrep", "-f", r"hypha_launch|hypha launch"], capture_output=True, check=False
    )
    if launched.returncode == 0:
        return True
    return any(maps_library(maps, "libhypha_ffi") for maps in proc.glob("[0-9]*/maps"))


def git_subcommand(words: list[str]) -> tuple[str, list[str]]:
    """`git -C dir -c k=v push x` -> ("push", ["x"])."""
    rest = words[1:]
    while rest and rest[0].startswith("-"):
        takes_value = rest[0] in ("-C", "-c", "--git-dir", "--work-tree")
        rest = rest[2:] if takes_value else rest[1:]
    return (rest[0], rest[1:]) if rest else ("", [])


def command_rule(words: list[str], is_subagent: bool) -> str | None:
    """The reason this simple command is blocked, or None."""
    program = Path(words[0]).name
    args = words[1:]
    script = Path(args[0]).name if program in SHELLS and args else program
    if script in STACK_KILLERS and live_stack():
        return f"a hypha stack is running; {script} clears its shared memory and would kill it. Ask the owner"
    if program == "cargo":
        sub = next((a for a in args if not a.startswith(("-", "+"))), "")
        if sub == "clean" and is_subagent:
            return (
                "subagents never delete target/: the orchestrator decides when the build trees go"
            )
        if sub == "install":
            return "install nothing"
        if sub in ("publish", "login", "yank", "owner"):
            return "agents never publish"
        blanket_update = sub == "update" and not any(
            a in ("-p", "--package", "--dry-run") or a.startswith("--package=") for a in args
        )
        if blanket_update:
            return "cargo update moves every dependency; name the package with -p"
        toolchain = next((a.removeprefix("+") for a in args if a.startswith("+")), "")
        if toolchain and not toolchain_installed(toolchain):
            return f"toolchain {toolchain} is not installed and cargo would install it: install nothing"
    if program == "rustup":
        sub = next((a for a in args if not a.startswith("+")), "")
        listing = args[1:2] == ["list"]
        if sub not in RUSTUP_READ_ONLY and not listing:
            return "install nothing: toolchains and components are the owner's"
    if program in SYSTEM_PROGRAMS:
        return "install nothing; the system is the owner's"
    if program in ("pkill", "killall"):
        return "kill only a process you started, by its pid: a name could match the owner's running stack"
    if program == "rm":
        for arg in args:
            top = arg.removeprefix("./").split("/", 1)[0]
            if top in ("target", "build") and is_subagent:
                return "subagents never delete target/ or build/: the orchestrator decides when they go"
    if program == "git":
        sub, rest = git_subcommand(words)
        if sub in GIT_RULES:
            return GIT_RULES[sub]
        if sub == "reset" and "--hard" in rest:
            return "agents never hard-reset: it destroys uncommitted work"
        if sub == "checkout" and "--" not in rest and is_subagent:
            return "subagents stay on the current branch"
        discards = (sub == "checkout" and "--" in rest) or (
            sub == "restore" and not any(a in ("--staged", "-S") for a in rest)
        )
        if discards:
            return "agents never discard uncommitted work: it may be the owner's. Ask the owner"
        forced_delete = sub == "branch" and (
            "-D" in rest or ("--delete" in rest and "--force" in rest)
        )
        if forced_delete:
            return (
                "agents never force-delete a branch: its commits may be nowhere else. Ask the owner"
            )
        listing = sub == "branch" and (not rest or all(a.startswith("-") for a in rest))
        if is_subagent and sub in SUBAGENT_GIT_RULES and not listing:
            return SUBAGENT_GIT_RULES[sub]
        if sub == "commit" and any(a in ("--no-verify", "-n", "--amend") for a in rest):
            return (
                "agents commit new commits through the pre-commit hook: no --no-verify, no --amend"
            )
        if sub == "config" and any(a.lower() == "core.hookspath" for a in rest):
            return "the git hooks path is the owner's setting"
    pip_install = (program.startswith("pip") and args[:1] == ["install"]) or (
        program.startswith("python")
        and "-m" in args
        and args[args.index("-m") + 1 :][:2] == ["pip", "install"]
    )
    if pip_install:
        return "install nothing"
    npm_global = (
        program == "npm"
        and args[:1] in (["i"], ["install"])
        and ("-g" in args or "--global" in args)
    )
    if npm_global:
        return "install nothing"
    if program == "clean-shm.sh" and "--force" in args and live_stack():
        return "a hypha stack is running; clean-shm.sh --force would kill it. Ask the owner"
    return None


def toolchain_installed(name: str) -> bool:
    """Whether rustup has `name` (`1.98.0`, `nightly`). True when that cannot be told."""
    try:
        installed = [entry.name for entry in (HOME / ".rustup" / "toolchains").iterdir()]
    except OSError:
        return True
    return any(entry == name or entry.startswith(name + "-") for entry in installed)


def downloads_into_interpreter(command: str) -> bool:
    """`curl ... | sh`: a download piped straight into an interpreter."""
    words = shell_words(command)
    downloading = False
    start = True
    for word in words:
        if word in SEPARATORS:
            piped = word in ("|", "|&")
            downloading = downloading and piped
            start = True
            continue
        if start:
            start = False
            program = Path(word).name
            if downloading and program in INTERPRETERS:
                return True
            downloading = program in DOWNLOADERS
    return False


def check_command(call: Call) -> None:
    if downloads_into_interpreter(call.command):
        raise Blocked(
            "blocked: never pipe a download into an interpreter; download nothing to run."
        )
    for words in simple_commands(call.command):
        reason = command_rule(words, call.is_subagent)
        if reason:
            raise Blocked(f"blocked: {reason}.")


def check_subagent_files(call: Call) -> None:
    if not call.is_subagent:
        return
    for raw in call.paths:
        path = resolve(raw, call.cwd)
        is_board_file = path.parent == REPO and SUBAGENT_ONLY_FILES.match(path.name)
        is_brief = REPO / "hypha-audit-tasks" in path.parents
        if (is_board_file or is_brief) and call.kind == "edit":
            raise Blocked("blocked: the hypha-audit-* files are the orchestrator's.")


def pre_tool(call: Call) -> None:
    check_fence(call)
    check_protected(call)
    check_subagent_files(call)
    if call.kind == "bash":
        check_command(call)


# --- after an edit -----------------------------------------------------

ALLOW_WITHOUT_REASON = re.compile(r"#!?\[allow\((?!.*\breason\s*=)")
UNSAFE_BLOCK = re.compile(r"\bunsafe\s*\{")
UNWRAP = re.compile(r"\.unwrap\(\)|\.expect\(")


def added_lines(path: Path) -> dict[int, str]:
    """Line number -> text of every line the working tree adds over HEAD."""
    relative = path.relative_to(REPO)
    tracked = subprocess.run(
        ["git", "-C", str(REPO), "ls-files", "--error-unmatch", str(relative)],
        capture_output=True,
        check=False,
    )
    lines = path.read_text().splitlines()
    if tracked.returncode != 0:
        return dict(enumerate(lines, start=1))
    diff = subprocess.run(
        ["git", "-C", str(REPO), "diff", "-U0", "--no-color", "HEAD", "--", str(relative)],
        capture_output=True,
        text=True,
        check=False,
    ).stdout
    added = {}
    for match in re.finditer(r"^@@ -\S+ \+(\d+)(?:,(\d+))? @@", diff, re.M):
        start = int(match.group(1))
        count = int(match.group(2) or 1)
        for number in range(start, start + count):
            added[number] = lines[number - 1]
    return added


def rust_findings(path: Path, inserted: str | None) -> list[str]:
    lines = path.read_text().splitlines()
    first_test_line = next(
        (n for n, text in enumerate(lines, start=1) if "#[cfg(test)]" in text), len(lines) + 1
    )
    in_test_tree = any(part in ("tests", "benches", "examples") for part in path.parts)
    findings = []
    for number, text in added_lines(path).items():
        # For an edit whose inserted text is known, report only what it added.
        if inserted is not None and text.strip() and text.strip() not in inserted:
            continue
        code = text.split("//", 1)[0]
        if ALLOW_WITHOUT_REASON.search(code):
            findings.append(
                f'{number}: #[allow] without `reason = "..."`; use #[expect(.., reason)]'
            )
        if UNSAFE_BLOCK.search(code):
            window = "\n".join(lines[max(0, number - 7) : number])
            if "SAFETY:" not in window:
                findings.append(f"{number}: `unsafe` block without a `// SAFETY:` comment above it")
        shipped = not in_test_tree and number < first_test_line
        if shipped and UNWRAP.search(code):
            findings.append(f"{number}: unwrap()/expect() in shipped code; return the error")
    return findings


LOWERED_LINT = re.compile(
    r"""^\s*[A-Za-z_][\w:-]*\s*=\s*(?:"(allow|warn)"|\{[^}]*level\s*=\s*"(allow|warn)")"""
)


def lint_findings(path: Path) -> list[str]:
    """Added lines of a manifest that set a lint to `allow` or `warn`."""
    findings = []
    for number, text in added_lines(path).items():
        if LOWERED_LINT.search(text.split("#", 1)[0]):
            findings.append(
                f"{number}: a lint level of allow or warn is added; lint levels are not "
                "lowered without the owner's yes (fix the code, or #[expect] at the site)"
            )
    return findings


def post_edit(call: Call) -> list[str]:
    findings = []
    for raw in call.paths:
        path = resolve(raw, call.cwd)
        is_repo_manifest = (
            path.name in ("Cargo.toml", "clippy.toml") and REPO in path.parents and path.is_file()
        )
        if is_repo_manifest:
            findings.extend(f"{path.relative_to(REPO)}:{f}" for f in lint_findings(path))
        is_repo_rust = path.suffix == ".rs" and REPO in path.parents and path.is_file()
        if not is_repo_rust:
            continue
        subprocess.run(
            ["rustfmt", "--quiet", str(path)], cwd=REPO, capture_output=True, check=False
        )
        findings.extend(
            f"{path.relative_to(REPO)}:{f}" for f in rust_findings(path, call.added_text)
        )
    return findings


# --- a subagent hands back ---------------------------------------------------


def changed_packages() -> list[str]:
    status = subprocess.run(
        ["git", "-C", str(REPO), "status", "--porcelain"],
        capture_output=True,
        text=True,
        check=False,
    ).stdout
    packages = set()
    for line in status.splitlines():
        parts = Path(line[3:].strip()).parts
        is_rust = line.endswith((".rs", "Cargo.toml"))
        in_crate = len(parts) >= 2 and parts[0] in ("crates", "apps")
        if not (is_rust and in_crate):
            continue
        manifest = REPO / parts[0] / parts[1] / "Cargo.toml"
        if manifest.is_file():
            name = re.search(r'^name\s*=\s*"([^"]+)"', manifest.read_text(), re.M)
            if name:
                packages.add(name.group(1))
    return sorted(packages)


def stop(payload: dict, call: Call) -> str | None:
    """The reason to refuse the hand-back, or None."""
    if not call.is_subagent:
        return None
    # Three refusals per subagent at most, so a stuck agent cannot loop forever.
    key = re.sub(r"\W", "_", f"{payload.get('session_id')}-{payload.get('agent_id')}")
    counter = Path("/tmp") / f"hypha-hooks-stop-{key}"
    refusals = int(counter.read_text()) if counter.is_file() else 0
    if refusals >= 3:
        print(
            "hypha hooks: clippy still red after 3 refusals; letting the hand-back through",
            file=sys.stderr,
        )
        return None
    packages = changed_packages()
    if not packages:
        return None
    command = ["cargo", "clippy", "-q", "--locked", "--all-targets"]
    for package in packages:
        command += ["-p", package]
    command += ["--", "-D", "warnings"]
    env = {
        **os.environ,
        "CARGO_INCREMENTAL": "0",
        "CARGO_PROFILE_DEV_DEBUG": "0",
        "CARGO_PROFILE_TEST_DEBUG": "0",
    }
    result = subprocess.run(command, cwd=REPO, capture_output=True, text=True, env=env, check=False)
    if result.returncode == 0:
        counter.unlink(missing_ok=True)
        return None
    counter.write_text(str(refusals + 1))
    return (
        "cargo clippy is red on "
        + ", ".join(packages)
        + "; fix it before handing back:\n"
        + result.stderr[-4000:]
    )


# --- answering in each agent's format -------------------------------------------------


def deny(agent: str, event: str, reason: str) -> int:
    """Write the refusal the agent understands and return the exit code."""
    if agent == "copilot" and event == "pre-tool":
        print(json.dumps({"permissionDecision": "deny", "permissionDecisionReason": reason}))
        return 0
    if agent in ("gemini", "antigravity"):
        print(
            json.dumps({"decision": "deny" if event == "pre-tool" else "block", "reason": reason})
        )
        return 0
    # Claude Code and Codex: exit 2 blocks a tool call or a stop and shows stderr.
    print(reason, file=sys.stderr)
    return 2


def run(agent: str, event: str, raw: str) -> int:
    payload = json.loads(raw or "{}")
    call = NORMALISE[agent](payload)
    try:
        if event == "pre-tool":
            pre_tool(call)
        elif event == "post-edit":
            findings = post_edit(call)
            if findings:
                return deny(
                    agent, event, "Fix these in the lines you just added:\n" + "\n".join(findings)
                )
        elif event == "stop":
            reason = stop(payload, call)
            if reason:
                return deny(agent, event, reason)
    except Blocked as blocked:
        return deny(agent, event, f"{blocked} (hypha agent hooks, see AGENTS.md)")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--agent", required=True, choices=sorted(NORMALISE))
    parser.add_argument("--event", required=True, choices=("pre-tool", "post-edit", "stop"))
    options = parser.parse_args()
    raw = sys.stdin.read()
    try:
        return run(options.agent, options.event, raw)
    except Exception as error:  # noqa: BLE001 - a broken hook must fail open
        print(f"hypha hooks: internal error, call allowed: {error!r}", file=sys.stderr)
        return 0


if __name__ == "__main__":
    sys.exit(main())
