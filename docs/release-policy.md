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
| The requirements and the assumptions of use | [requirements-specification.md](requirements-specification.md), [safety-manual.md](safety-manual.md) |

A fix that cannot keep one of these starts a new release line.

Between release lines nothing is compatible: rebuild every application, and do not
mix two lines in one domain.

## 3. Supported platforms

| | |
|---|---|
| Operating system | Ubuntu 26.04, on amd64 and arm64 |
| C++ compiler | The GCC of that distribution; C++23 |
| Rust | The latest stable, 1.98 or newer (`rust-toolchain.toml`, `rust-version`); not pinned |
| Builder image | `docker/Dockerfile`, which installs exactly these |

macOS is a development platform with no guarantee: CI builds the workspace there and
runs clippy, no test. CI (`.github/workflows/ci.yml`) runs the gate on amd64 for every
push and pull request, and nightly on `develop` (or on demand) the C++ suite on arm64
and the qualification on amd64. The Rust suite and the qualification run on amd64 only,
so an arm64 release runs them by hand in the builder image and the release notes say so.

## 4. What a release must pass

1. `./check.sh` and `./test.sh --clean` in the builder image.
2. `scripts/qualify.sh` on the release commit: the report of CI's `qualification` job
   (nightly or started by hand), or of a run in the builder image, is attached to the
   release. Every requirement of class S is verified, or the release notes name
   the exception and its reason.
3. For the first release of a line, and after a change to the transport or the run
   loop: the endurance run of NFR-06. For every release: a 24 hour run. Neither run
   exists yet ([sil2-readiness.md](sil2-readiness.md), G-28); until they do, the
   release notes say so.
4. The release notes, `docs/releases/X.Y.Z.md`, written by hand and reviewed in the
   pull request of the release: a summary; the breaking changes and the upgrade; the
   changes, with their issues; the defects fixed and the known open ones, each with
   its class ([safety-plan.md](safety-plan.md) §7); whether an assumption of use
   changed; the qualification, with every requirement not verified and its reason;
   the platforms tested by hand. The tag and the GitHub release are made in the same
   step, never a bare tag: the GitHub release takes this file as its text and the
   qualification report as an attachment:
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
fix, of any class, lands in the next release, and an upgrade is the way to get it. From
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
