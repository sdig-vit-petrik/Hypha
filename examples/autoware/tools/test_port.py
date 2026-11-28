#!/usr/bin/env python3
"""Tests of examples/autoware/tools/port.py. Run: python3 -I examples/autoware/tools/test_port.py

Each test builds a small examples/autoware tree in a temporary directory: one vendored
repository with its manifest and one package patch. The vendor tests fetch from a git
repository made in the temporary directory, so nothing is downloaded.
"""

from __future__ import annotations

import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import port  # noqa: E402

SOURCE = "upstream/demo/pkg"
FILTER = "int main() {\n  int width = 1;\n  return width;\n}\n"
FILTER_EDITED = "int main() {\n  int width = 2;  // hypha port: two.\n  return width;\n}\n"
LICENSE = "Apache 2.0\n"
# Fixed author and committer, and no user configuration, so the fixture commits are the same.
GIT_ENVIRONMENT = {
    "GIT_CONFIG_GLOBAL": os.devnull,
    "GIT_CONFIG_NOSYSTEM": "1",
    "GIT_AUTHOR_NAME": "port test",
    "GIT_AUTHOR_EMAIL": "port@test.invalid",
    "GIT_AUTHOR_DATE": "2026-01-01T00:00:00Z",
    "GIT_COMMITTER_NAME": "port test",
    "GIT_COMMITTER_EMAIL": "port@test.invalid",
    "GIT_COMMITTER_DATE": "2026-01-01T00:00:00Z",
}


def git(repository: Path, *arguments: str) -> str:
    return subprocess.run(  # noqa: S603
        ["git", "-C", str(repository), *arguments],  # noqa: S607
        capture_output=True,
        text=True,
        check=True,
        env={**os.environ, **GIT_ENVIRONMENT},
    ).stdout.strip()


class PortTest(unittest.TestCase):
    def setUp(self) -> None:
        scratch = tempfile.TemporaryDirectory()
        self.addCleanup(scratch.cleanup)
        self.scratch = Path(scratch.name)
        self.root = self.scratch / "autoware"
        self.repository = self.root / "upstream" / "demo"
        self.package = self.repository / "pkg"
        (self.package / "src").mkdir(parents=True)
        (self.package / "src" / "filter.cpp").write_text(FILTER)
        (self.repository / "LICENSE").write_text(LICENSE)
        (self.repository / "README.md").write_text("# demo (vendored)\n")
        (self.root / "upstream" / "patches").mkdir()
        self.write_manifest()
        self.work = self.scratch / "work"

    def write_manifest(self, commit: str = "0" * 40, url: str = "https://example.invalid") -> None:
        files = {
            name: port.sha256(self.repository / name) for name in ("LICENSE", "pkg/src/filter.cpp")
        }
        manifest = port.Manifest(url=url, commit=commit, ref="1.0.0", files=files)
        (self.repository / "vendor.toml").write_text(port.render_manifest(manifest))

    def make_patch(self) -> Path:
        port.edit(self.root, "pkg", self.work, SOURCE)
        (self.work / "src" / "filter.cpp").write_text(FILTER_EDITED)
        return port.diff(self.root, "pkg", self.work, SOURCE)

    def make_upstream(self) -> tuple[Path, str]:
        """A git repository holding the vendored files at their upstream paths."""
        origin = self.scratch / "origin"
        (origin / "pkg" / "src").mkdir(parents=True)
        (origin / "pkg" / "src" / "filter.cpp").write_text(FILTER)
        (origin / "pkg" / "src" / "extra.cpp").write_text("// not vendored\n")
        (origin / "LICENSE").write_text(LICENSE)
        git(origin, "init", "--quiet")
        git(origin, "add", ".")
        git(origin, "commit", "--quiet", "-m", "upstream")
        # GitHub and GitLab serve a commit by its id; a local repository does on request.
        git(origin, "config", "uploadpack.allowAnySHA1InWant", "true")
        return origin, git(origin, "rev-parse", "HEAD")

    def test_verify_accepts_files_that_match_their_manifest(self) -> None:
        self.make_patch()
        self.assertEqual(port.verify(self.root), [])

    def test_verify_refuses_a_changed_vendored_file(self) -> None:
        (self.package / "src" / "filter.cpp").write_text(FILTER_EDITED)
        self.assertEqual(
            port.verify(self.root),
            ["demo/pkg/src/filter.cpp: differs from its SHA-256 in vendor.toml"],
        )

    def test_verify_refuses_a_missing_and_an_unlisted_file(self) -> None:
        (self.repository / "LICENSE").unlink()
        (self.package / "src" / "extra.cpp").write_text("// extra\n")
        (self.package / ".DS_Store").write_text("ignored\n")
        self.assertEqual(
            port.verify(self.root),
            ["demo/LICENSE: listed, missing", "demo/pkg/src/extra.cpp: not listed in vendor.toml"],
        )

    def test_a_root_places_the_files_among_others_and_checks_only_those_it_lists(self) -> None:
        origin, commit = self.make_upstream()
        placed = self.scratch / "placed"
        (placed / "defs").mkdir(parents=True)
        (placed / "defs" / "other.msg").write_text("not vendored\n")
        directory = placed / "upstream" / "demo"
        directory.mkdir(parents=True)
        manifest = port.Manifest(
            url=str(origin),
            commit=commit,
            root="../../defs",
            renamed={"Filter.cpp": "pkg/src/filter.cpp"},
            files={"Filter.cpp": "0" * 64},
        )
        (directory / "vendor.toml").write_text(port.render_manifest(manifest))
        port.vendor(directory, [], None)
        self.assertEqual((placed / "defs" / "Filter.cpp").read_text(), FILTER)
        self.assertEqual(port.load_manifest(directory / "vendor.toml").root, "../../defs")
        self.assertEqual(port.verify_repository(directory), [])
        (placed / "defs" / "Filter.cpp").write_text(FILTER_EDITED)
        self.assertEqual(
            port.verify_repository(directory),
            ["demo/Filter.cpp: differs from its SHA-256 in vendor.toml"],
        )

    def test_verify_refuses_a_repository_without_a_manifest(self) -> None:
        (self.root / "upstream" / "other").mkdir()
        self.assertEqual(port.verify(self.root), ["other: no vendor.toml"])

    def test_verify_refuses_a_patch_that_does_not_apply(self) -> None:
        patch = self.make_patch()
        patch.write_text(patch.read_text().replace("-  int width = 1;", "-  int width = 9;"))
        problems = port.verify(self.root)
        self.assertEqual(len(problems), 1)
        self.assertTrue(problems[0].startswith("patches/pkg.patch does not apply to " + SOURCE))

    def test_verify_refuses_a_patch_without_its_header(self) -> None:
        patch = self.make_patch()
        patch.write_text(patch.read_text().replace("applied to a copy", "applied to"))
        problems = port.verify(self.root)
        self.assertEqual(len(problems), 1)
        self.assertIn("the header does not say", problems[0])

    def test_load_manifest_refuses_a_short_commit_and_an_unknown_key(self) -> None:
        manifest = self.repository / "vendor.toml"
        text = manifest.read_text()
        manifest.write_text(text.replace("0" * 40, "0" * 7))
        with self.assertRaisesRegex(port.PortError, "not a full 40-digit commit id"):
            port.load_manifest(manifest)
        manifest.write_text('tag = "1.0.0"\n' + text)
        with self.assertRaisesRegex(port.PortError, r"unknown keys \['tag'\]"):
            port.load_manifest(manifest)

    def test_diff_writes_the_patch_with_its_header_and_without_file_times(self) -> None:
        patch = self.make_patch()
        self.assertEqual(
            patch.read_text(),
            "# pkg: the edits of the hypha port, applied to a copy of\n"
            f"# {SOURCE} by the build (patch -p1).\n"
            "# Every edit is also marked '// hypha port:' with its reason.\n"
            "diff -ruN a/src/filter.cpp b/src/filter.cpp\n"
            "--- a/src/filter.cpp\n"
            "+++ b/src/filter.cpp\n"
            "@@ -1,4 +1,4 @@\n"
            " int main() {\n"
            "-  int width = 1;\n"
            "+  int width = 2;  // hypha port: two.\n"
            "   return width;\n"
            " }\n",
        )

    def test_edit_of_a_fresh_checkout_regenerates_the_same_patch(self) -> None:
        patch = self.make_patch()
        written = patch.read_text()
        fresh = self.scratch / "fresh"
        port.edit(self.root, "pkg", fresh, None)
        self.assertEqual((fresh / "src" / "filter.cpp").read_text(), FILTER_EDITED)
        port.diff(self.root, "pkg", fresh, None)
        self.assertEqual(patch.read_text(), written)
        self.assertEqual((self.package / "src" / "filter.cpp").read_text(), FILTER)

    def test_edit_refuses_an_existing_copy_and_diff_one_without_edits(self) -> None:
        self.make_patch()
        with self.assertRaisesRegex(port.PortError, "exists"):
            port.edit(self.root, "pkg", self.work, None)
        unedited = self.scratch / "unedited"
        port.edit(self.root, "pkg", unedited, None)
        (unedited / "src" / "filter.cpp").write_text(FILTER)
        with self.assertRaisesRegex(port.PortError, "no edits"):
            port.diff(self.root, "pkg", unedited, None)

    def test_edit_needs_a_source_for_a_package_without_a_patch(self) -> None:
        with self.assertRaisesRegex(port.PortError, "--source"):
            port.edit(self.root, "pkg", self.work, None)
        with self.assertRaisesRegex(port.PortError, "not a directory under upstream/"):
            port.edit(self.root, "pkg", self.work, "shim")

    def test_vendor_reproduces_the_tree_from_its_manifest(self) -> None:
        origin, commit = self.make_upstream()
        self.write_manifest(commit=commit, url=str(origin))
        shutil.rmtree(self.package)
        (self.repository / "LICENSE").unlink()
        manifest = port.vendor(self.repository, [], None)
        self.assertEqual(sorted(manifest.files), ["LICENSE", "pkg/src/filter.cpp"])
        self.assertEqual((self.package / "src" / "filter.cpp").read_text(), FILTER)
        self.assertEqual(port.verify(self.root), [])

    def test_vendor_adds_a_directory_and_rewrites_the_hashes(self) -> None:
        origin, commit = self.make_upstream()
        self.write_manifest(commit=commit, url="https://example.invalid/unreachable")
        (self.package / "src" / "filter.cpp").write_text(FILTER_EDITED)
        port.vendor(self.repository, ["pkg/src/"], str(origin))
        manifest = port.load_manifest(self.repository / "vendor.toml")
        self.assertEqual(
            sorted(manifest.files), ["LICENSE", "pkg/src/extra.cpp", "pkg/src/filter.cpp"]
        )
        self.assertEqual((self.package / "src" / "filter.cpp").read_text(), FILTER)
        self.assertEqual(port.verify(self.root), [])

    def test_vendor_refuses_a_path_missing_at_the_commit(self) -> None:
        origin, commit = self.make_upstream()
        self.write_manifest(commit=commit, url=str(origin))
        with self.assertRaisesRegex(port.PortError, "no file at"):
            port.vendor(self.repository, ["pkg/missing"], None)

    def test_verify_sdk_checks_the_sdk_upstream_directory(self) -> None:
        sdk = self.scratch / "sdk-upstream"
        self.assertEqual(port.verify_sdk(sdk), [])
        shutil.copytree(self.repository, sdk / "demo")
        self.assertEqual(port.verify_sdk(sdk), [])
        (sdk / "demo" / "pkg" / "src" / "filter.cpp").write_text(FILTER_EDITED)
        self.assertEqual(
            port.verify_sdk(sdk),
            ["demo/pkg/src/filter.cpp: differs from its SHA-256 in vendor.toml"],
        )


if __name__ == "__main__":
    unittest.main()
