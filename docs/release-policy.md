# Release policy

What a hypha release is, what stays fixed inside a release line, and how to upgrade
and roll back.

Readers: an integrator who pins a hypha version, and the engineer who cuts a release.

## 1. Versions

A version is `MAJOR.MINOR.PATCH`. A **release line** is one `MAJOR.MINOR`. A release
is a git tag `vX.Y.Z` on `main` together with a GitHub release of the same name that
carries its notes and its qualification report (§4). A tag without the GitHub release
is not a release, and neither is anything else.

## 2. What a release line freezes

Inside a release line only fixes are made. These do not change:

| Frozen | Stated in |
|---|---|
| The C ABI revision, and so the library file name | `abi.revision` |
| The wire frame format | [interface-specification.md](interface-specification.md), wire frame |
| The recording format (MCAP layout and metadata) | [tools.md](tools.md) |
| Source compatibility of the public C++ headers: an application that built against one patch release builds against the next | `crates/hypha-cxx/include/` |
| The requirements and the assumptions of use | [requirements-specification.md](requirements-specification.md), [failure-analysis.md](failure-analysis.md) §4 |

A fix that cannot keep one of these starts a new release line.

Between release lines nothing is compatible: rebuild every application, and do not
mix two lines in one domain.

## 3. Supported platforms

| | |
|---|---|
| Operating system | Ubuntu 26.04, on amd64 and arm64 |
| C++ compiler | The GCC of that distribution; C++23 |
| Rust | One exact version for development and CI, `channel` of `rust-toolchain.toml`; the MSRV, `rust-version` of `Cargo.toml`, is the highest `rust-version` of the dependencies (1.89), and `test.sh` and CI build the workspace with it too |
| Builder image | `docker/Dockerfile`, which installs exactly these |

macOS is a development platform with no guarantee. `.github/workflows/ci.yml` defines the
gate on amd64 for pushes and pull requests, and a nightly run of the arm64 C++ suite and the
qualification. CI minutes are limited, so the workflow is switched off and the owner
enables it for a run; the gate that counts is the local one, `./check.sh` and
`./test.sh --clean`, in the builder image.

## 4. What a release must pass

1. `./check.sh` and `./test.sh --clean` in the builder image.
2. `scripts/qualify.sh` on the release commit, in the builder image. Its report is
   attached to the GitHub release and lists every requirement that is not verified, with
   the reason.
3. A triage and compaction pass ([features.md](features.md) §2): remove unused public
   surface, merge duplicated test harnesses, fold or delete implemented `docs/design/`
   notes, and shrink the files pinned in `scripts/check-size-and-boundaries.sh`.
4. The release notes, `docs/releases/X.Y.Z.md`, reviewed in the release's pull request:
   a summary; the breaking changes and the upgrade; the changes, with their issues; the
   known open defects; whether an assumption of use
   ([failure-analysis.md](failure-analysis.md) §4) changed; the requirements the
   qualification did not verify; the platforms tested by hand. The tag and the GitHub
   release are made in one step, never a bare tag, with this file as the text and the
   qualification report attached:
   `gh release create vX.Y.Z --target main --notes-file docs/releases/X.Y.Z.md report.md`.

## 5. Upgrade and rollback

hypha has no rolling upgrade.

1. Stop every process of the domain.
2. Install the new library and the applications built against it.
3. Reset the shared memory of the domain
   ([containerized-deployment.md](containerized-deployment.md) §4).
4. Start the processes.

A rollback is the same procedure with the previous build. Keep the previous library
and applications until the new release has run in the system.

## 6. Support

Before 1.0.0, a release line receives no fixes once the next line is released: every
fix lands in the next release, and an upgrade is the way to get it. From
1.0.0 on, the release notes of a line state how long it receives fixes.

## 7. Branches

| Branch | What it holds | How it changes |
|---|---|---|
| `main` | Released states only; each merge is tagged `vX.Y.Z` | A pull request from `develop`, nothing else |
| `develop` | The next release; CI is green | Pull requests from topic branches; while a release candidate is qualified, fixes only |
| `feat/*`, `fix/*`, `docs/*`, `chore/*` | One change | Branched from `develop`, merged back by pull request, then deleted |
| `support/X.Y` | Fixes for an older release line, from 1.0.0 on (§6) | Created from that line's last tag when it needs a fix; each fix lands on `develop` first and is cherry-picked; tagged there, never merged into `main` |

Every change has a GitHub issue, except a typo-sized fix. Its branch is
`<type>/<issue>-<slug>` (`fix/26-macos-tests`). Each commit names the issue in a trailer
line: `Refs #N` while the work goes on, `Fixes #N` in the commit or pull request that
finishes it. `develop` is the default branch, so the issue closes when that commit
reaches `develop`.

The rule is enforced in three places: a ruleset on `main` (pull requests only, the CI
checks required, no force push, no deletion), the CI job `source-branch` (a pull
request into `main` must come from this repository's `develop`), and
`.githooks/pre-commit` (no commit on `main`).
