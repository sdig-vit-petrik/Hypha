#!/usr/bin/env python3
"""Requirements traceability and the qualification report.

usage: scripts/traceability.py render [--verify]
       scripts/traceability.py report --output FILE [--nextest-junit FILE]...
                                      [--gtest-xml DIR] [--run NAME=pass|fail]...

render writes docs/traceability.md from docs/traceability.toml; with --verify it
writes nothing and fails when the file differs. Both refuse a requirement of
docs/requirements-specification.md without an entry, an entry without a requirement,
and a test reference that names no test in the tree.

report writes the result of every requirement from the test results it is given: the
JUnit files of cargo nextest, the directory of gtest XML files that ctest fills
(build/gtest-xml), and the outcome of each whole run. A requirement counts as verified
when every one of its references ran and none failed, or when its method is not a test
and its evidence is stated.

Exit status: 0 done (report: every requirement verified), 1 the generated file differs
or a requirement is not verified, 2 an input is invalid.
"""

from __future__ import annotations

import argparse
import datetime
import fnmatch
import platform
import re
import subprocess
import sys
import tomllib
import xml.etree.ElementTree as ElementTree
from dataclasses import dataclass, field
from pathlib import Path

REPO = Path(__file__).resolve().parents[1]
SOURCE = REPO / "docs" / "traceability.toml"
RENDERED = REPO / "docs" / "traceability.md"
SPECIFICATION = REPO / "docs" / "requirements-specification.md"
RUST_ROOTS = ("crates", "apps")
CXX_TEST_DIRS = ("crates/hypha-cxx/test", "crates/hypha-cxx/base/test")
RUN_NAMES = ("check", "coverage", "miri", "fuzz", "asan", "tsan")
ENTRY_KEYS = ("id", "tests", "evidence", "gap")
METHOD_NAMES = {"T": "test", "A": "analysis", "I": "inspection", "D": "demonstration"}

SPEC_ROW = re.compile(r"^\| \*\*((?:FSR|NFR)-\d+)\*\* \|.*\| ([TAID](?:, [TAID])*) \|\s*$")
GTEST_SUITE = re.compile(r"^\s*TEST(?:_F)?\(\s*(\w+)\s*,", re.MULTILINE)

STATUS_OK = "pass"
STATUS_FAIL = "FAIL"
STATUS_NOT_RUN = "not run"
STATUS_NO_TEST = "no test"
STATUS_STATED = "stated"
STATUS_NO_EVIDENCE = "no evidence"
VERIFIED_STATUSES = (STATUS_OK, STATUS_STATED)


class InputError(Exception):
    """An input file is missing or invalid."""


@dataclass
class Requirement:
    id: str
    methods: list[str]
    tests: list[str] = field(default_factory=list)
    evidence: str = ""
    gap: str = ""


@dataclass
class Count:
    """Tests that ran and tests that failed."""

    run: int = 0
    failed: int = 0

    def add(self, other: Count) -> None:
        self.run += other.run
        self.failed += other.failed


@dataclass
class Results:
    """Test results by name: nextest suites, gtest suites, whole runs."""

    rust: dict[str, Count] = field(default_factory=dict)
    gtest: dict[str, Count] = field(default_factory=dict)
    runs: dict[str, bool] = field(default_factory=dict)


# --- the sources ---------------------------------------------------------------


def load_methods(specification: Path) -> dict[str, list[str]]:
    """The verification methods of each requirement, in the specification's order."""
    methods: dict[str, list[str]] = {}
    for line in specification.read_text(encoding="utf-8").splitlines():
        row = SPEC_ROW.match(line)
        if row is None:
            continue
        requirement_id = row.group(1)
        if requirement_id in methods:
            raise InputError(f"{specification.name}: {requirement_id} is stated twice")
        methods[requirement_id] = row.group(2).split(", ")
    if not methods:
        raise InputError(f"{specification.name}: no requirement row found")
    return methods


def load_requirements(source: Path, methods: dict[str, list[str]]) -> list[Requirement]:
    """The entries of the traceability source, one per requirement of the specification."""
    try:
        document = tomllib.loads(source.read_text(encoding="utf-8"))
    except tomllib.TOMLDecodeError as error:
        raise InputError(f"{source.name}: {error}") from error
    entries = document.get("requirement")
    if not isinstance(entries, list):
        raise InputError(f"{source.name}: no [[requirement]] entry")

    requirements: list[Requirement] = []
    seen: set[str] = set()
    for entry in entries:
        requirement_id = entry.get("id")
        if not isinstance(requirement_id, str):
            raise InputError(f"{source.name}: an entry has no id")
        for key in entry:
            if key not in ENTRY_KEYS:
                raise InputError(f"{source.name}: {requirement_id}: unknown key '{key}'")
        if requirement_id in seen:
            raise InputError(f"{source.name}: {requirement_id} has two entries")
        if requirement_id not in methods:
            raise InputError(
                f"{source.name}: {requirement_id} is not a requirement of {SPECIFICATION.name}"
            )
        seen.add(requirement_id)
        tests = entry.get("tests", [])
        evidence = entry.get("evidence", "")
        gap = entry.get("gap", "")
        tests_are_text = isinstance(tests, list) and all(isinstance(test, str) for test in tests)
        if not tests_are_text:
            raise InputError(f"{source.name}: {requirement_id}: tests is not a list of text")
        if not isinstance(evidence, str) or not isinstance(gap, str):
            raise InputError(f"{source.name}: {requirement_id}: evidence and gap are text")
        requirements.append(
            Requirement(requirement_id, methods[requirement_id], tests, evidence, gap)
        )

    for requirement_id in methods:
        if requirement_id not in seen:
            raise InputError(f"{source.name}: {requirement_id} has no entry")
    return requirements


# --- test references ------------------------------------------------------------


def rust_crate_dir(repo: Path, crate: str) -> Path | None:
    for root in RUST_ROOTS:
        candidate = repo / root / crate
        if (candidate / "Cargo.toml").is_file():
            return candidate
    return None


def rust_reference_parts(reference: str) -> tuple[str, str] | None:
    """Crate and binary pattern of '<crate>/<pattern>', or None for another form."""
    parts = reference.split("/")
    if len(parts) != 2 or not parts[0] or not parts[1]:
        return None
    return parts[0], parts[1]


def cxx_files(repo: Path, pattern: str) -> list[Path]:
    """The gtest source files a 'cxx:<pattern>' reference names."""
    found: list[Path] = []
    for directory in CXX_TEST_DIRS:
        for path in sorted((repo / directory).glob("*.cpp")):
            if fnmatch.fnmatchcase(path.stem, pattern):
                found.append(path)
    return found


def cxx_suites(files: list[Path]) -> list[str]:
    """The gtest suite names the files declare, each once."""
    suites: list[str] = []
    for path in files:
        for suite in GTEST_SUITE.findall(path.read_text(encoding="utf-8")):
            if suite not in suites:
                suites.append(suite)
    return suites


def reference_error(repo: Path, reference: str) -> str | None:
    """Why the reference names no test in the tree, or None when it names one."""
    if reference.startswith("run:"):
        name = reference.removeprefix("run:")
        if name not in RUN_NAMES:
            return f"unknown run '{name}' (known: {', '.join(RUN_NAMES)})"
        return None

    if reference.startswith("cxx:"):
        files = cxx_files(repo, reference.removeprefix("cxx:"))
        if not files:
            return "no gtest source file of that name"
        if not cxx_suites(files):
            return "the file declares no gtest suite"
        return None

    parts = rust_reference_parts(reference)
    if parts is None:
        return "not of the form <crate>/<binary>, cxx:<file> or run:<name>"
    crate, pattern = parts
    crate_dir = rust_crate_dir(repo, crate)
    if crate_dir is None:
        return f"no crate '{crate}'"
    if pattern == "*":
        return None
    for path in (crate_dir / "tests").glob("*.rs"):
        if fnmatch.fnmatchcase(path.stem, pattern):
            return None
    return f"no test binary '{pattern}' in {crate_dir.relative_to(repo)}/tests"


def check_references(repo: Path, requirements: list[Requirement]) -> None:
    errors: list[str] = []
    for requirement in requirements:
        for reference in requirement.tests:
            error = reference_error(repo, reference)
            if error is not None:
                errors.append(f"{requirement.id}: '{reference}': {error}")
    if errors:
        raise InputError("\n".join(errors))


# --- the generated document -----------------------------------------------------


def verified_by_cell(requirement: Requirement) -> str:
    parts: list[str] = []
    if requirement.tests:
        parts.append(", ".join(f"`{reference}`" for reference in requirement.tests))
    if requirement.evidence:
        parts.append(requirement.evidence)
    if not parts:
        return "None"
    return "; ".join(parts)


def render(requirements: list[Requirement]) -> str:
    lines = [
        "# Requirements traceability",
        "",
        "<!-- Generated by scripts/traceability.py from traceability.toml. Edit that file. -->",
        "",
        "Each requirement of [requirements-specification.md](requirements-specification.md)",
        "and the evidence that verifies it. A test reference is `<crate>/<binary>` (a Rust",
        "test binary under `<crate>/tests/`), `cxx:<file>` (a gtest source file of",
        "`crates/hypha-cxx`) or `run:<name>` (a whole run: `check.sh`, coverage, Miri,",
        "fuzzing, a sanitizer); `*` stands for any name. The gap column says what the stated",
        "method still lacks. The letters and numbers in brackets (R1, P4, ...) are the items",
        "of the former backlog document. `scripts/traceability.py report` writes the result",
        "of every row from a test run.",
    ]
    sections = (("FSR", "Functional requirements"), ("NFR", "Non-functional requirements"))
    for prefix, title in sections:
        lines += ["", f"## {title}", "", "| ID | Verified by | Gap |", "|---|---|---|"]
        for requirement in requirements:
            if requirement.id.startswith(prefix):
                cell = verified_by_cell(requirement)
                lines.append(f"| {requirement.id} | {cell} | {requirement.gap} |")
    return "\n".join(lines) + "\n"


# --- test results ---------------------------------------------------------------


def parse_xml(path: Path) -> ElementTree.Element:
    try:
        # The files are written by cargo nextest and gtest in this build tree.
        return ElementTree.parse(path).getroot()  # noqa: S314
    except (OSError, ElementTree.ParseError) as error:
        raise InputError(f"{path}: {error}") from error


def read_nextest_junit(path: Path, results: Results) -> None:
    for suite in parse_xml(path).iter("testsuite"):
        name = suite.get("name", "")
        count = results.rust.setdefault(name, Count())
        for case in suite.iter("testcase"):
            skipped = case.find("skipped") is not None
            if skipped:
                continue
            failed = case.find("failure") is not None or case.find("error") is not None
            count.add(Count(1, 1 if failed else 0))


def read_gtest_xml(directory: Path, results: Results) -> None:
    files = sorted(directory.glob("*.xml"))
    if not files:
        raise InputError(f"{directory}: no gtest XML file")
    for path in files:
        for suite in parse_xml(path).iter("testsuite"):
            name = suite.get("name", "")
            count = results.gtest.setdefault(name, Count())
            for case in suite.iter("testcase"):
                ran = case.get("status") == "run" and case.get("result") != "skipped"
                if not ran:
                    continue
                failed = case.find("failure") is not None
                count.add(Count(1, 1 if failed else 0))


def read_run(argument: str, results: Results) -> None:
    name, separator, outcome = argument.partition("=")
    well_formed = separator == "=" and name in RUN_NAMES and outcome in ("pass", "fail")
    if not well_formed:
        raise InputError(f"--run {argument}: expected <{'|'.join(RUN_NAMES)}>=pass|fail")
    results.runs[name] = outcome == "pass"


def reference_count(repo: Path, reference: str, results: Results) -> Count:
    """The tests of one reference that ran and that failed."""
    total = Count()
    if reference.startswith("run:"):
        name = reference.removeprefix("run:")
        if name in results.runs:
            total.add(Count(1, 0 if results.runs[name] else 1))
        return total

    if reference.startswith("cxx:"):
        files = cxx_files(repo, reference.removeprefix("cxx:"))
        for suite in cxx_suites(files):
            if suite in results.gtest:
                total.add(results.gtest[suite])
        return total

    parts = rust_reference_parts(reference)
    if parts is None:
        return total
    crate, pattern = parts
    for name, count in results.rust.items():
        # nextest names a suite '<crate>' (unit tests) or '<crate>::<binary>'.
        suite_crate, _, binary = name.partition("::")
        if suite_crate != crate:
            continue
        unit_tests_of_whole_crate = binary == "" and pattern == "*"
        named_binary = binary != "" and fnmatch.fnmatchcase(binary, pattern)
        if unit_tests_of_whole_crate or named_binary:
            total.add(count)
    return total


def requirement_status(requirement: Requirement, counts: list[Count]) -> str:
    if not requirement.tests:
        verified_by_test = "T" in requirement.methods
        if verified_by_test:
            return STATUS_NO_TEST
        if requirement.evidence:
            return STATUS_STATED
        return STATUS_NO_EVIDENCE
    for count in counts:
        if count.failed > 0:
            return STATUS_FAIL
    for count in counts:
        if count.run == 0:
            return STATUS_NOT_RUN
    return STATUS_OK


# --- the report -----------------------------------------------------------------


def tool_line(command: list[str]) -> str:
    """The first output line of a version command, or why there is none."""
    try:
        done = subprocess.run(command, capture_output=True, text=True, cwd=REPO, check=False)  # noqa: S603
    except OSError as error:
        return f"not available ({error.strerror})"
    if done.returncode != 0 or not done.stdout.strip():
        return f"not available (exit {done.returncode})"
    return done.stdout.strip().splitlines()[0]


def environment_rows() -> list[tuple[str, str]]:
    commit = tool_line(["git", "rev-parse", "HEAD"])
    changed = subprocess.run(
        ["git", "status", "--porcelain"],  # noqa: S607
        capture_output=True,
        text=True,
        cwd=REPO,
        check=False,
    )
    tree_is_modified = changed.returncode != 0 or changed.stdout.strip() != ""
    if tree_is_modified:
        commit += " (with uncommitted changes)"
    now = datetime.datetime.now(datetime.UTC).strftime("%Y-%m-%d %H:%M UTC")
    return [
        ("Commit", commit),
        ("Date", now),
        ("ABI revision", (REPO / "abi.revision").read_text(encoding="utf-8").strip()),
        ("Host", platform.platform()),
        ("Rust", tool_line(["rustc", "--version"])),
        ("C++", tool_line(["c++", "--version"])),
    ]


def build_report(repo: Path, requirements: list[Requirement], results: Results) -> tuple[str, int]:
    """The report text and the number of requirements that are not verified."""
    statuses: dict[str, str] = {}
    counts_of: dict[str, list[Count]] = {}
    for requirement in requirements:
        counts = [reference_count(repo, reference, results) for reference in requirement.tests]
        counts_of[requirement.id] = counts
        statuses[requirement.id] = requirement_status(requirement, counts)

    by_test = sum(1 for status in statuses.values() if status == STATUS_OK)
    stated = sum(1 for status in statuses.values() if status == STATUS_STATED)
    open_ids = [rid for rid, status in statuses.items() if status not in VERIFIED_STATUSES]
    with_gap = [r.id for r in requirements if statuses[r.id] in VERIFIED_STATUSES and r.gap]

    lines = ["# Qualification report", "", "| | |", "|---|---|"]
    for label, value in environment_rows():
        lines.append(f"| {label} | {value} |")

    lines += ["", "## Result", ""]
    lines.append(f"- {len(requirements)} requirements.")
    lines.append(f"- {by_test} verified by tests that ran and passed.")
    lines.append(f"- {stated} verified by analysis, inspection or demonstration, as stated.")
    lines.append(f"- {len(open_ids)} not verified: {', '.join(open_ids) or 'none'}.")
    lines.append(
        f"- {len(with_gap)} verified with a recorded gap: {', '.join(with_gap) or 'none'}."
    )

    lines += ["", "## Runs", "", "| Run | Result |", "|---|---|"]
    for name in RUN_NAMES:
        if name not in results.runs:
            outcome = "not run"
        elif results.runs[name]:
            outcome = "pass"
        else:
            outcome = "FAIL"
        lines.append(f"| {name} | {outcome} |")

    lines += [
        "",
        "## Requirements",
        "",
        "Status: `pass` every reference ran and none failed; `FAIL` a test failed;",
        "`not run` a reference has no result; `no test` the method is a test and none is",
        "named; `stated` verified by another method, see the evidence; `no evidence` none.",
        "",
        "| ID | Method | Status | Tests run | Failed | Gap |",
        "|---|---|---|---|---|---|",
    ]
    for requirement in requirements:
        run = sum(count.run for count in counts_of[requirement.id])
        failed = sum(count.failed for count in counts_of[requirement.id])
        method = ", ".join(METHOD_NAMES[letter] for letter in requirement.methods)
        lines.append(
            f"| {requirement.id} | {method} | {statuses[requirement.id]} | {run} | {failed} "
            f"| {requirement.gap} |"
        )

    lines += [
        "",
        "## Evidence",
        "",
        "| ID | Reference or stated evidence | Tests run | Failed |",
        "|---|---|---|---|",
    ]
    for requirement in requirements:
        for reference, count in zip(requirement.tests, counts_of[requirement.id], strict=True):
            lines.append(f"| {requirement.id} | `{reference}` | {count.run} | {count.failed} |")
        if requirement.evidence:
            lines.append(f"| {requirement.id} | {requirement.evidence} | | |")
    return "\n".join(lines) + "\n", len(open_ids)


# --- commands -------------------------------------------------------------------


def load_checked(repo: Path) -> list[Requirement]:
    methods = load_methods(repo / SPECIFICATION.relative_to(REPO))
    requirements = load_requirements(repo / SOURCE.relative_to(REPO), methods)
    check_references(repo, requirements)
    return requirements


def command_render(arguments: argparse.Namespace) -> int:
    text = render(load_checked(REPO))
    if not arguments.verify:
        RENDERED.write_text(text, encoding="utf-8")
        return 0
    current = RENDERED.read_text(encoding="utf-8") if RENDERED.is_file() else ""
    if current != text:
        print(
            "docs/traceability.md differs from docs/traceability.toml; "
            "run: scripts/traceability.py render",
            file=sys.stderr,
        )
        return 1
    return 0


def command_report(arguments: argparse.Namespace) -> int:
    requirements = load_checked(REPO)
    results = Results()
    for path in arguments.nextest_junit:
        read_nextest_junit(Path(path), results)
    if arguments.gtest_xml is not None:
        read_gtest_xml(Path(arguments.gtest_xml), results)
    for argument in arguments.run:
        read_run(argument, results)

    text, open_count = build_report(REPO, requirements, results)
    Path(arguments.output).write_text(text, encoding="utf-8")
    verified = len(requirements) - open_count
    print(f"{arguments.output}: {verified} of {len(requirements)} requirements verified")
    if open_count > 0:
        return 1
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    commands = parser.add_subparsers(dest="command", required=True)
    render_parser = commands.add_parser("render")
    render_parser.add_argument("--verify", action="store_true")
    report_parser = commands.add_parser("report")
    report_parser.add_argument("--output", required=True)
    report_parser.add_argument("--nextest-junit", action="append", default=[])
    report_parser.add_argument("--gtest-xml")
    report_parser.add_argument("--run", action="append", default=[])
    arguments = parser.parse_args()

    try:
        if arguments.command == "render":
            return command_render(arguments)
        return command_report(arguments)
    except InputError as error:
        print(error, file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
