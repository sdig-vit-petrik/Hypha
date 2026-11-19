#!/usr/bin/env bash
# usage: scripts/check-links.sh [file.md ...]
#
# Every relative link in the tracked Markdown files (or in the files named) must name an
# existing file, and every anchor (#...) must name a heading of its target. Slugs follow
# GitHub and GitLab: lower case, punctuation other than `-` and `_` removed, spaces to `-`.
set -euo pipefail
cd "$(dirname "$0")/.."

python3 - "$@" <<'PY'
import pathlib, re, subprocess, sys

if len(sys.argv) > 1:
    files = [pathlib.Path(name) for name in sys.argv[1:]]
else:
    listed = subprocess.run(["git", "ls-files", "*.md"], capture_output=True, text=True, check=True)
    files = [pathlib.Path(name) for name in listed.stdout.split()
             if not pathlib.Path(name).name.startswith("hypha-")]

link = re.compile(r"\[[^\]]*\]\(([^)\s]+)\)")
fence = re.compile(r"^\s*```")

def slug(heading):
    text = heading.strip().lower()
    text = re.sub(r"[^\w\- ]", "", text)
    return text.replace(" ", "-")

def anchors(path):
    found = set()
    inside = False
    for line in path.read_text(encoding="utf-8").splitlines():
        if fence.match(line):
            inside = not inside
            continue
        if not inside and line.startswith("#"):
            found.add(slug(line.lstrip("#")))
    return found

broken = []
for path in files:
    inside = False
    for number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        if fence.match(line):
            inside = not inside
            continue
        if inside:
            continue
        for target in link.findall(line):
            if re.match(r"^[a-z]+:", target):
                continue
            file_part, _, anchor = target.partition("#")
            resolved = (path.parent / file_part) if file_part else path
            if not resolved.exists():
                broken.append(f"{path}:{number}: {target}")
                continue
            if anchor and resolved.suffix == ".md" and anchor not in anchors(resolved):
                broken.append(f"{path}:{number}: {target} (no such heading)")

if broken:
    print("broken links:", file=sys.stderr)
    for item in broken:
        print("  " + item, file=sys.stderr)
    sys.exit(1)
print(f"OK: links in {len(files)} Markdown files resolve.")
PY
