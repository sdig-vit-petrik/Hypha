#!/usr/bin/env python3
"""The porting tool of examples/autoware: the vendored upstream files and the port's patches.

usage: examples/autoware/tools/port.py verify
       examples/autoware/tools/port.py vendor <repository> [--add PATH]... [--from URL|DIR]
       examples/autoware/tools/port.py edit <package> <work dir> [--source DIR]
       examples/autoware/tools/port.py diff <package> <work dir> [--source DIR]

verify checks, offline, every file of upstream/<repository>/ against the SHA-256 of its
manifest upstream/<repository>/vendor.toml, refuses a file the manifest does not list,
and checks that every upstream/patches/<package>.patch applies to its vendored package
(patch --dry-run, with the flags of the build). ./check.sh runs it.

vendor fetches the repository's commit (git fetch --depth 1, from the manifest's url or
from --from, a mirror or a local clone), writes every listed file and every file under
each --add path at the commit, and rewrites the manifest's hashes. A new repository
starts from a manifest holding url, commit and, when there is one, ref (its tag or
branch).

edit copies the vendored package to <work dir> and applies its patch there: an editable
copy outside the build directory. diff writes the edits of <work dir> back to
upstream/patches/<package>.patch, against the vendored package, with the patch header.
The package's directory comes from its patch header; --source (upstream/<repository>/...)
names it for a package without a patch.

Exit status: 0 done, 1 a check failed, 2 the input is invalid.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import shutil
import subprocess
import sys
import tempfile
import tomllib
from dataclasses import dataclass, field
from pathlib import Path

AUTOWARE = Path(__file__).resolve().parents[1]
# The vendored files of the C++ SDK (crates/hypha-cxx/upstream): verified and vendored
# the same way, with no patches.
SDK_UPSTREAM = AUTOWARE.parents[1] / "crates" / "hypha-cxx" / "upstream"
MANIFEST = "vendor.toml"
README = "README.md"
PATCHES = "patches"
# Names a file system leaves beside the vendored files; never vendored, never checked.
IGNORED_NAMES = frozenset({".DS_Store", "__pycache__"})
MANIFEST_KEYS = frozenset({"url", "ref", "commit", "renamed", "files"})
COMMIT = re.compile(r"^[0-9a-f]{40}$")
SHA256 = re.compile(r"^[0-9a-f]{64}$")
PATCH_HEADER = re.compile(
    r"^(?P<package>[\w.+-]+): the edits of the hypha port, applied to a copy of "
    r"(?P<source>upstream/\S+) by the build \(patch -p1\)\."
)
# The flags of autoware_patched() in examples/autoware/CMakeLists.txt.
PATCH_FLAGS = ("-p1", "--forward", "--batch")
EXIT_FAILED = 1
EXIT_INVALID = 2


class PortError(Exception):
    """An invalid input: a manifest, a patch header, an argument or a fetch."""


@dataclass
class Manifest:
    """upstream/<repository>/vendor.toml: where the files come from, and their hashes."""

    url: str
    commit: str
    # The tag or branch the commit was taken from; empty when none is recorded.
    ref: str = ""
    # Path here -> path upstream, for the files whose paths differ.
    renamed: dict[str, str] = field(default_factory=dict)
    # Path here -> SHA-256 of the file.
    files: dict[str, str] = field(default_factory=dict)


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def _string(value: object, name: str, where: Path) -> str:
    if not isinstance(value, str) or not value:
        raise PortError(f"{where}: {name} is not a non-empty string")
    return value


def load_manifest(path: Path) -> Manifest:
    try:
        data = tomllib.loads(path.read_text(encoding="utf-8"))
    except (OSError, tomllib.TOMLDecodeError) as error:
        raise PortError(f"{path}: {error}") from error
    unknown = sorted(set(data) - MANIFEST_KEYS)
    if unknown:
        raise PortError(f"{path}: unknown keys {unknown}")
    commit = _string(data.get("commit"), "commit", path)
    if not COMMIT.match(commit):
        raise PortError(f"{path}: commit {commit!r} is not a full 40-digit commit id")
    manifest = Manifest(
        url=_string(data.get("url"), "url", path),
        commit=commit,
        ref=_string(data["ref"], "ref", path) if "ref" in data else "",
    )
    for table in ("renamed", "files"):
        entries = data.get(table, {})
        if not isinstance(entries, dict):
            raise PortError(f"{path}: {table} is not a table")
        for key, value in entries.items():
            _string(value, f"{table}.{key!r}", path)
            _check_relative(key, path)
        getattr(manifest, table).update(entries)
    for local, digest in manifest.files.items():
        if not SHA256.match(digest):
            raise PortError(f"{path}: files.{local!r} is not a SHA-256")
    stray = sorted(set(manifest.renamed) - set(manifest.files))
    if stray:
        raise PortError(f"{path}: renamed lists files that files does not: {stray}")
    return manifest


def _check_relative(name: str, where: Path) -> None:
    parts = Path(name).parts
    if Path(name).is_absolute() or ".." in parts or not parts:
        raise PortError(f"{where}: {name!r} is not a relative path inside the repository")
    if name in (MANIFEST, README):
        raise PortError(f"{where}: {name!r} is the port's own file, not a vendored one")


def _quote(text: str) -> str:
    # A JSON string is a TOML basic string.
    return json.dumps(text, ensure_ascii=False)


def render_manifest(manifest: Manifest) -> str:
    lines = [
        "# The vendored files: examples/autoware/tools/port.py vendor writes the hashes,",
        "# port.py verify checks them. README.md says why these files.",
        f"url = {_quote(manifest.url)}",
    ]
    if manifest.ref:
        lines.append(f"ref = {_quote(manifest.ref)}")
    lines.append(f"commit = {_quote(manifest.commit)}")
    if manifest.renamed:
        lines += ["", "# Path here = path upstream, for the files whose paths differ.", "[renamed]"]
        lines += [f"{_quote(k)} = {_quote(v)}" for k, v in sorted(manifest.renamed.items())]
    lines += ["", "# Path here = SHA-256 of the file.", "[files]"]
    lines += [f"{_quote(k)} = {_quote(v)}" for k, v in sorted(manifest.files.items())]
    return "\n".join(lines) + "\n"


def _repository_files(directory: Path) -> list[str]:
    """Every file under one vendored repository but the port's README and manifest."""
    found = []
    for path in sorted(directory.rglob("*")):
        relative = path.relative_to(directory)
        if any(part in IGNORED_NAMES for part in relative.parts) or path.is_dir():
            continue
        if relative.as_posix() in (MANIFEST, README):
            continue
        found.append(relative.as_posix())
    return found


def verify_repository(directory: Path) -> list[str]:
    """The problems of one vendored repository: changed, missing and unlisted files."""
    manifest_path = directory / MANIFEST
    if not manifest_path.is_file():
        return [f"{directory.name}: no {MANIFEST}"]
    manifest = load_manifest(manifest_path)
    problems = []
    for local, digest in sorted(manifest.files.items()):
        path = directory / local
        if not path.is_file():
            problems.append(f"{directory.name}/{local}: listed, missing")
        elif sha256(path) != digest:
            problems.append(f"{directory.name}/{local}: differs from its SHA-256 in {MANIFEST}")
    problems += [
        f"{directory.name}/{local}: not listed in {MANIFEST}"
        for local in _repository_files(directory)
        if local not in manifest.files
    ]
    return problems


@dataclass
class Patch:
    package: str
    path: Path
    # The vendored package, relative to examples/autoware: upstream/<repository>/...
    source: str


def read_patch_header(path: Path) -> Patch:
    comment = []
    for line in path.read_text(encoding="utf-8").splitlines():
        if not line.startswith("#"):
            break
        comment.append(line.removeprefix("#").strip())
    match = PATCH_HEADER.match(" ".join(comment))
    if match is None:
        raise PortError(
            f"{path}: the header does not say '<package>: the edits of the hypha port, "
            "applied to a copy of upstream/<path> by the build (patch -p1).'"
        )
    if match["package"] != path.stem:
        raise PortError(f"{path}: the header names {match['package']}, the file {path.stem}")
    return Patch(package=path.stem, path=path, source=match["source"])


def patch_header(package: str, source: str) -> str:
    return (
        f"# {package}: the edits of the hypha port, applied to a copy of\n"
        f"# {source} by the build (patch -p1).\n"
        "# Every edit is also marked '// hypha port:' with its reason.\n"
    )


def _patch_program() -> str:
    program = shutil.which("patch")
    if program is None:
        raise PortError("patch is not on PATH")
    return program


def apply_patch(patch: Path, directory: Path, dry_run: bool) -> tuple[bool, str]:
    command = [_patch_program(), *PATCH_FLAGS, "-d", str(directory), "-i", str(patch)]
    if dry_run:
        command.append("--dry-run")
    result = subprocess.run(command, capture_output=True, text=True, check=False)  # noqa: S603
    return result.returncode == 0, (result.stdout + result.stderr).strip()


def _source_dir(root: Path, source: str) -> Path:
    directory = (root / source).resolve()
    upstream = (root / "upstream").resolve()
    if not directory.is_relative_to(upstream) or directory == upstream:
        raise PortError(f"{source}: not a directory under upstream/")
    if not directory.is_dir():
        raise PortError(f"{source}: no such directory")
    return directory


def verify_patches(root: Path) -> list[str]:
    problems = []
    for path in sorted((root / "upstream" / PATCHES).glob("*.patch")):
        try:
            patch = read_patch_header(path)
            directory = _source_dir(root, patch.source)
        except PortError as error:
            problems.append(str(error))
            continue
        applies, output = apply_patch(path, directory, dry_run=True)
        if not applies:
            problems.append(f"{PATCHES}/{path.name} does not apply to {patch.source}:\n{output}")
    return problems


def verify(root: Path) -> list[str]:
    upstream = root / "upstream"
    problems = []
    for directory in sorted(p for p in upstream.iterdir() if p.is_dir()):
        if directory.name in IGNORED_NAMES or directory.name == PATCHES:
            continue
        problems += verify_repository(directory)
    return problems + verify_patches(root)


def verify_sdk(upstream: Path) -> list[str]:
    """The problems of the SDK's vendored repositories, if it has any."""
    problems = []
    if upstream.is_dir():
        for directory in sorted(p for p in upstream.iterdir() if p.is_dir()):
            problems += verify_repository(directory)
    return problems


def _git(repository: Path, *arguments: str) -> bytes:
    result = subprocess.run(  # noqa: S603
        ["git", "-C", str(repository), *arguments],  # noqa: S607
        capture_output=True,
        check=False,
    )
    if result.returncode != 0:
        message = result.stderr.decode(errors="replace").strip()
        raise PortError(f"git {' '.join(arguments)}: {message}")
    return result.stdout


def vendor(directory: Path, additions: list[str], origin: str | None) -> Manifest:
    """Writes the manifest's files, and those under each addition, at the commit."""
    manifest_path = directory / MANIFEST
    manifest = load_manifest(manifest_path)
    for addition in additions:
        _check_relative(addition.rstrip("/"), manifest_path)
    # git runs in the scratch clone: a local directory is named by its absolute path.
    if origin is not None and Path(origin).is_dir():
        origin = str(Path(origin).resolve())
    with tempfile.TemporaryDirectory(prefix="port-vendor-") as scratch:
        clone = Path(scratch)
        _git(clone, "init", "--quiet")
        _git(clone, "fetch", "--quiet", "--depth", "1", origin or manifest.url, manifest.commit)
        fetched = _git(clone, "rev-parse", "FETCH_HEAD").decode().strip()
        if fetched != manifest.commit:
            raise PortError(f"fetched {fetched}, the manifest names {manifest.commit}")
        wanted = {local: manifest.renamed.get(local, local) for local in manifest.files}
        for addition in additions:
            listed = _git(clone, "ls-tree", "-r", "-z", "--name-only", fetched, "--", addition)
            names = [name for name in listed.decode().split("\0") if name]
            if not names:
                raise PortError(f"{addition}: no file at {fetched}")
            wanted.update({name: name for name in names})
        files = {}
        for local, path in sorted(wanted.items()):
            data = _git(clone, "cat-file", "blob", f"{fetched}:{path}")
            target = directory / local
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(data)
            files[local] = hashlib.sha256(data).hexdigest()
    manifest.files = files
    manifest_path.write_text(render_manifest(manifest), encoding="utf-8")
    return manifest


def _resolve_source(root: Path, package: str, source: str | None) -> tuple[str, Path]:
    patch_path = root / "upstream" / PATCHES / f"{package}.patch"
    if source is None:
        if not patch_path.is_file():
            raise PortError(f"{package}: no patch; --source names its vendored directory")
        source = read_patch_header(patch_path).source
    source = Path(source).as_posix().rstrip("/")
    return source, _source_dir(root, source)


def edit(root: Path, package: str, work: Path, source: str | None) -> None:
    _, directory = _resolve_source(root, package, source)
    if work.exists():
        raise PortError(f"{work} exists; edit makes a new copy")
    shutil.copytree(directory, work)
    patch_path = root / "upstream" / PATCHES / f"{package}.patch"
    if patch_path.is_file():
        applied, output = apply_patch(patch_path, work, dry_run=False)
        if not applied:
            raise PortError(f"{patch_path.name} does not apply:\n{output}")


def _diff_lines(output: str) -> list[str]:
    """diff -ruN output without the file times, so a patch does not change with them."""
    lines = []
    header = 0
    for line in output.splitlines():
        if line.startswith("Binary files "):
            raise PortError(f"a patch holds text only: {line}")
        if line.startswith("diff "):
            header = 2
            # diff names the two files last; the patch says how it was made.
            _, _, left, right = line.rsplit(" ", 3)
            lines.append(f"diff -ruN {left} {right}")
            continue
        if header and line.startswith(("--- ", "+++ ")):
            header -= 1
            line = line.split("\t", 1)[0]
        lines.append(line)
    return lines


def diff(root: Path, package: str, work: Path, source: str | None) -> Path:
    source, directory = _resolve_source(root, package, source)
    if not work.is_dir():
        raise PortError(f"{work}: no such directory")
    with tempfile.TemporaryDirectory(prefix="port-diff-") as scratch:
        base = Path(scratch)
        shutil.copytree(directory, base / "a")
        shutil.copytree(work, base / "b", ignore=shutil.ignore_patterns("*.orig", "*.rej"))
        result = subprocess.run(
            ["diff", "-ruN", "a", "b"],  # noqa: S607
            cwd=base,
            capture_output=True,
            text=True,
            check=False,
        )
    if result.returncode == 0:
        raise PortError(f"{work}: no edits against {source}")
    if result.returncode != 1:
        raise PortError(f"diff failed: {result.stderr.strip()}")
    patch_path = root / "upstream" / PATCHES / f"{package}.patch"
    patch_path.parent.mkdir(parents=True, exist_ok=True)
    body = "\n".join(_diff_lines(result.stdout)) + "\n"
    patch_path.write_text(patch_header(package, source) + body, encoding="utf-8")
    applies, output = apply_patch(patch_path, directory, dry_run=True)
    if not applies:
        raise PortError(f"{patch_path.name} written but does not apply:\n{output}")
    return patch_path


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(
        description=__doc__.split("\n\n", 1)[0], formatter_class=argparse.RawTextHelpFormatter
    )
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser("verify", help="check the vendored files and the patches, offline")
    vendor_parser = commands.add_parser("vendor", help="fetch a repository's listed files")
    vendor_parser.add_argument(
        "repository",
        help="the directory name under examples/autoware/upstream/ or crates/hypha-cxx/upstream/",
    )
    vendor_parser.add_argument("--add", action="append", default=[], metavar="PATH")
    vendor_parser.add_argument("--from", dest="origin", metavar="URL|DIR")
    for name, text in (("edit", "make a patched copy"), ("diff", "write the patch back")):
        sub = commands.add_parser(name, help=text)
        sub.add_argument("package")
        sub.add_argument("work", type=Path, help="the editable copy")
        sub.add_argument("--source", metavar="DIR", help="upstream/<repository>/<package>")
    arguments = parser.parse_args(argv)

    try:
        if arguments.command == "verify":
            problems = verify(AUTOWARE)
            for problem in problems:
                print(f"examples/autoware/upstream: {problem}", file=sys.stderr)
            sdk_problems = verify_sdk(SDK_UPSTREAM)
            for problem in sdk_problems:
                print(f"crates/hypha-cxx/upstream: {problem}", file=sys.stderr)
            if problems or sdk_problems:
                return EXIT_FAILED
            print("examples/autoware/upstream: the vendored files and the patches match")
            print("crates/hypha-cxx/upstream: the vendored files match")
        elif arguments.command == "vendor":
            directory = AUTOWARE / "upstream" / arguments.repository
            if not directory.is_dir():
                directory = SDK_UPSTREAM / arguments.repository
            if arguments.repository in (PATCHES, "") or not directory.is_dir():
                raise PortError(f"{arguments.repository}: not a vendored repository")
            manifest = vendor(directory, arguments.add, arguments.origin)
            print(f"{arguments.repository}: {len(manifest.files)} files at {manifest.commit}")
            problems = verify_repository(directory)
            for problem in problems:
                print(problem, file=sys.stderr)
            if problems:
                return EXIT_FAILED
        elif arguments.command == "edit":
            edit(AUTOWARE, arguments.package, arguments.work, arguments.source)
            print(f"{arguments.package}: the patched copy is {arguments.work}")
        else:
            patch_path = diff(AUTOWARE, arguments.package, arguments.work, arguments.source)
            print(f"{arguments.package}: wrote {patch_path.relative_to(AUTOWARE)}")
    except PortError as error:
        print(f"port.py: {error}", file=sys.stderr)
        return EXIT_INVALID
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
