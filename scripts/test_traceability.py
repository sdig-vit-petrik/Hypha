#!/usr/bin/env python3
"""Tests of scripts/traceability.py. Run: python3 -I scripts/test_traceability.py

Each test builds a small repository tree in a temporary directory: a specification,
a traceability source, one Rust crate with a test binary and one gtest source file.
"""

from __future__ import annotations

import os
import sys
import tempfile
import unittest
from unittest import mock
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import traceability  # noqa: E402

SPECIFICATION = """\
| ID | Requirement | Priority | Verify |
|---|---|---|---|
| **FSR-01** | The software **shall** send. | Reliability | T |
| **NFR-01** | One library. | Architecture | Reliability | I |
"""

SOURCE = """\
[[requirement]]
id = "FSR-01"
tests = ["alpha/sends", "cxx:test_send"]

[[requirement]]
id = "NFR-01"
evidence = "Inspection: one library"
"""

GTEST_FILE = """\
TEST(Send, DeliversOneSample) {}
TEST_F(SendFixture, RefusesAFullPool) {}
"""

NEXTEST_JUNIT = """\
<testsuites>
  <testsuite name="alpha">
    <testcase name="unit"/>
  </testsuite>
  <testsuite name="alpha::sends">
    <testcase name="one"/>
    <testcase name="two">{failure}</testcase>
    <testcase name="three"><skipped/></testcase>
  </testsuite>
  <testsuite name="alpha::sends_more">
    <testcase name="other"/>
  </testsuite>
  <testsuite name="alphabet::sends">
    <testcase name="foreign"/>
  </testsuite>
</testsuites>
"""

GTEST_XML = """\
<testsuites>
  <testsuite name="Send">
    <testcase name="DeliversOneSample" status="run" result="completed"/>
    <testcase name="Disabled" status="notrun" result="suppressed"/>
  </testsuite>
  <testsuite name="SendFixture">
    <testcase name="RefusesAFullPool" status="run" result="completed">{failure}</testcase>
  </testsuite>
</testsuites>
"""


class Tree:
    """A repository tree in a temporary directory."""

    def __init__(self, case: unittest.TestCase, source: str = SOURCE) -> None:
        self.holder = tempfile.TemporaryDirectory()
        case.addCleanup(self.holder.cleanup)
        self.root = Path(self.holder.name)
        self.write("docs/requirements-specification.md", SPECIFICATION)
        self.write("docs/traceability.toml", source)
        self.write("crates/alpha/Cargo.toml", "")
        self.write("crates/alpha/tests/sends.rs", "")
        self.write("crates/hypha-cxx/test/test_send.cpp", GTEST_FILE)
        self.write("crates/hypha-cxx/test/send_peer.cpp", "int main() {}\n")
        (self.root / "crates/hypha-cxx/base/test").mkdir(parents=True)

    def write(self, relative: str, text: str) -> Path:
        path = self.root / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text, encoding="utf-8")
        return path

    def results(self, rust_failure: str = "", gtest_failure: str = "") -> traceability.Results:
        results = traceability.Results()
        junit = self.write("junit.xml", NEXTEST_JUNIT.format(failure=rust_failure))
        traceability.read_nextest_junit(junit, results)
        self.write("gtest-xml/shard.xml", GTEST_XML.format(failure=gtest_failure))
        traceability.read_gtest_xml(self.root / "gtest-xml", results)
        return results

    def statuses(self, results: traceability.Results) -> dict[str, str]:
        statuses: dict[str, str] = {}
        for requirement in traceability.load_checked(self.root):
            counts = [
                traceability.reference_count(self.root, reference, results)
                for reference in requirement.tests
            ]
            statuses[requirement.id] = traceability.requirement_status(requirement, counts)
        return statuses


class SourceChecks(unittest.TestCase):
    def refused(self, source: str) -> str:
        tree = Tree(self, source)
        with self.assertRaises(traceability.InputError) as raised:
            traceability.load_checked(tree.root)
        return str(raised.exception)

    def test_a_valid_source_loads_with_the_methods_of_the_specification(self) -> None:
        tree = Tree(self)
        requirements = traceability.load_checked(tree.root)
        self.assertEqual([r.id for r in requirements], ["FSR-01", "NFR-01"])
        self.assertEqual(requirements[0].methods, ["T"])
        self.assertEqual(requirements[1].methods, ["I"])

    def test_a_requirement_without_an_entry_is_refused(self) -> None:
        message = self.refused('[[requirement]]\nid = "FSR-01"\ntests = ["alpha/sends"]\n')
        self.assertIn("NFR-01 has no entry", message)

    def test_an_entry_for_no_requirement_is_refused(self) -> None:
        message = self.refused(SOURCE + '\n[[requirement]]\nid = "FSR-99"\n')
        self.assertIn("FSR-99 is not a requirement", message)

    def test_a_second_entry_of_one_requirement_is_refused(self) -> None:
        message = self.refused(SOURCE + '\n[[requirement]]\nid = "NFR-01"\n')
        self.assertIn("NFR-01 has two entries", message)

    def test_an_unknown_key_is_refused(self) -> None:
        message = self.refused(SOURCE + 'note = "x"\n')
        self.assertIn("unknown key 'note'", message)

    def test_a_reference_to_a_missing_rust_binary_is_refused(self) -> None:
        message = self.refused(SOURCE.replace("alpha/sends", "alpha/gone"))
        self.assertIn("FSR-01: 'alpha/gone': no test binary 'gone'", message)

    def test_a_reference_to_a_missing_crate_is_refused(self) -> None:
        message = self.refused(SOURCE.replace("alpha/sends", "beta/sends"))
        self.assertIn("no crate 'beta'", message)

    def test_a_reference_to_a_missing_gtest_file_is_refused(self) -> None:
        message = self.refused(SOURCE.replace("cxx:test_send", "cxx:test_gone"))
        self.assertIn("no gtest source file", message)

    def test_a_reference_to_a_file_without_a_gtest_suite_is_refused(self) -> None:
        message = self.refused(SOURCE.replace("cxx:test_send", "cxx:send_peer"))
        self.assertIn("declares no gtest suite", message)

    def test_an_unknown_run_is_refused(self) -> None:
        message = self.refused(SOURCE.replace("cxx:test_send", "run:lint"))
        self.assertIn("unknown run 'lint'", message)

    def test_a_measurement_run_is_refused_as_a_reference(self) -> None:
        message = self.refused(SOURCE.replace("cxx:test_send", "run:rated_load"))
        self.assertIn("'rated_load' is a measurement on no reference platform", message)


class Rendering(unittest.TestCase):
    def test_the_document_has_one_row_per_requirement_under_its_section(self) -> None:
        tree = Tree(self)
        text = traceability.render(traceability.load_checked(tree.root))
        functional, non_functional = text.split("## Non-functional requirements")
        self.assertIn("| FSR-01 | `alpha/sends`, `cxx:test_send` |  |", functional)
        self.assertIn("| NFR-01 | Inspection: one library |  |", non_functional)

    def test_a_requirement_with_no_evidence_reads_none(self) -> None:
        source = SOURCE.replace('evidence = "Inspection: one library"', 'gap = "Open."')
        tree = Tree(self, source)
        text = traceability.render(traceability.load_checked(tree.root))
        self.assertIn("| NFR-01 | None | Open. |", text)


class ReferenceResults(unittest.TestCase):
    def count(self, reference: str) -> traceability.Count:
        tree = Tree(self)
        return traceability.reference_count(tree.root, reference, tree.results())

    def test_a_binary_reference_counts_that_binary_only_and_no_skipped_test(self) -> None:
        self.assertEqual(self.count("alpha/sends"), traceability.Count(2, 0))

    def test_a_crate_reference_counts_its_unit_tests_and_every_binary(self) -> None:
        self.assertEqual(self.count("alpha/*"), traceability.Count(4, 0))

    def test_a_pattern_counts_the_binaries_it_matches(self) -> None:
        self.assertEqual(self.count("alpha/sends*"), traceability.Count(3, 0))

    def test_a_gtest_reference_counts_the_suites_of_its_file_that_ran(self) -> None:
        self.assertEqual(self.count("cxx:test_send"), traceability.Count(2, 0))

    def test_a_run_without_an_outcome_counts_nothing(self) -> None:
        self.assertEqual(self.count("run:miri"), traceability.Count(0, 0))

    def test_a_failed_run_counts_as_one_failure(self) -> None:
        tree = Tree(self)
        results = tree.results()
        traceability.read_run("miri=fail", results)
        count = traceability.reference_count(tree.root, "run:miri", results)
        self.assertEqual(count, traceability.Count(1, 1))

    def test_a_malformed_run_argument_is_refused(self) -> None:
        for argument in ("miri", "miri=ok", "lint=pass", "rated_load=ok", "miri=limits-fail"):
            with self.assertRaises(traceability.InputError):
                traceability.read_run(argument, traceability.Results())

    def test_a_directory_without_gtest_results_is_refused(self) -> None:
        tree = Tree(self)
        (tree.root / "empty").mkdir()
        with self.assertRaises(traceability.InputError):
            traceability.read_gtest_xml(tree.root / "empty", traceability.Results())


class Statuses(unittest.TestCase):
    def test_every_reference_ran_and_none_failed_is_a_pass(self) -> None:
        tree = Tree(self)
        statuses = tree.statuses(tree.results())
        self.assertEqual(statuses, {"FSR-01": "pass", "NFR-01": "stated"})

    def test_a_failed_rust_test_fails_its_requirement(self) -> None:
        tree = Tree(self)
        statuses = tree.statuses(tree.results(rust_failure="<failure/>"))
        self.assertEqual(statuses["FSR-01"], "FAIL")

    def test_a_failed_gtest_case_fails_its_requirement(self) -> None:
        tree = Tree(self)
        statuses = tree.statuses(tree.results(gtest_failure="<failure/>"))
        self.assertEqual(statuses["FSR-01"], "FAIL")

    def test_a_reference_without_a_result_is_not_run(self) -> None:
        tree = Tree(self)
        results = traceability.Results()
        traceability.read_nextest_junit(
            tree.write("junit.xml", NEXTEST_JUNIT.format(failure="")), results
        )
        self.assertEqual(tree.statuses(results)["FSR-01"], "not run")

    def test_a_test_requirement_that_names_no_test_has_no_test(self) -> None:
        source = SOURCE.replace('tests = ["alpha/sends", "cxx:test_send"]', 'evidence = "By hand"')
        tree = Tree(self, source)
        self.assertEqual(tree.statuses(tree.results())["FSR-01"], "no test")

    def test_an_inspection_requirement_without_evidence_has_no_evidence(self) -> None:
        source = SOURCE.replace('evidence = "Inspection: one library"', "")
        tree = Tree(self, source)
        self.assertEqual(tree.statuses(tree.results())["NFR-01"], "no evidence")


class Report(unittest.TestCase):
    def test_the_report_counts_the_requirements_that_are_not_verified(self) -> None:
        tree = Tree(self)
        requirements = traceability.load_checked(tree.root)
        passed = tree.results()
        text, open_count, regressions = traceability.build_report(tree.root, requirements, passed)
        self.assertEqual((open_count, regressions), (0, 0))
        self.assertIn("| FSR-01 | test | pass | 4 | 0 |  |", text)

        failed = tree.results(rust_failure="<failure/>")
        text, open_count, regressions = traceability.build_report(tree.root, requirements, failed)
        self.assertEqual((open_count, regressions), (1, 1), "a failed test is a regression")
        self.assertIn("1 not verified: FSR-01.", text)
        self.assertIn("| FSR-01 | `alpha/sends` | 2 | 1 |", text)

    def test_a_requirement_with_no_test_yet_and_a_recorded_gap_is_open_but_no_regression(
        self,
    ) -> None:
        no_test = SOURCE.replace('tests = ["alpha/sends", "cxx:test_send"]', 'gap = "No test yet."')
        tree = Tree(self, no_test)
        requirements = traceability.load_checked(tree.root)
        _, open_count, regressions = traceability.build_report(
            tree.root, requirements, tree.results()
        )
        self.assertEqual((open_count, regressions), (1, 0))

        unexplained = SOURCE.replace('tests = ["alpha/sends", "cxx:test_send"]', "")
        tree = Tree(self, unexplained)
        requirements = traceability.load_checked(tree.root)
        _, open_count, regressions = traceability.build_report(
            tree.root, requirements, tree.results()
        )
        self.assertEqual((open_count, regressions), (1, 1), "an open requirement without a gap")

    def test_a_measurement_whose_limits_failed_changes_no_exit_status(self) -> None:
        for outcome in ("pass", "limits-fail"):
            results = traceability.Results()
            traceability.read_run(f"rated_load={outcome}", results)
            self.assertEqual(traceability.exit_status(0, 0, results), 0, outcome)
            self.assertEqual(
                traceability.exit_status(1, 0, results), traceability.EXIT_ONLY_KNOWN_GAPS
            )

    def test_a_measurement_that_failed_to_run_fails_the_report(self) -> None:
        results = traceability.Results()
        traceability.read_run("rated_load=fail", results)
        self.assertEqual(traceability.exit_status(0, 0, results), 1)
        self.assertEqual(traceability.exit_status(1, 0, results), 1)

    def test_the_runs_table_marks_the_measurement_as_taken_on_a_development_host(self) -> None:
        tree = Tree(self)
        requirements = traceability.load_checked(tree.root)
        note = "on a development host: no reference platform is defined (G-10)"
        expected = {
            "pass": "limits pass",
            "limits-fail": "limits FAIL",
            "fail": "run FAIL",
            None: "not run",
        }
        for outcome, shown in expected.items():
            results = tree.results()
            if outcome is not None:
                traceability.read_run(f"rated_load={outcome}", results)
            text, _, _ = traceability.build_report(tree.root, requirements, results)
            self.assertIn(f"| rated_load | {shown}, {note} |", text)

    def test_the_report_names_the_builder_image_by_its_content_digest(self) -> None:
        image = {"HYPHA_BUILDER_IMAGE": "hypha-builder:ci", "HYPHA_BUILDER_IMAGE_ID": "sha256:ab12"}
        with mock.patch.dict(os.environ, image):
            rows = dict(traceability.environment_rows())
        self.assertEqual(rows["Builder image"], "hypha-builder:ci (sha256:ab12)")

        with mock.patch.dict(os.environ, {}, clear=True):
            rows = dict(traceability.environment_rows())
        self.assertEqual(rows["Builder image"], "none: not run through scripts/in-image.sh")


if __name__ == "__main__":
    unittest.main()
