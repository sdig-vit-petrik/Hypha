# Safety plan

How hypha is managed, developed, verified and changed: roles, lifecycle, verification,
modification, defects and tools. It has the form of a functional safety plan under
IEC 61508-1 clause 6 and IEC 61508-3 clauses 6 and 7.

Readers: everyone who changes hypha, and an assessor.

## 1. Status

- hypha is SIL 0 software and this plan claims no compliance. It applies from its
  first issue (2026-11-19). The code written before it was not developed under it;
  [sil2-readiness.md](sil2-readiness.md) §2 says how that code would be assessed.
- A rule below that is not yet practised is marked **not yet**. The plan states the
  rule; it does not pretend the record exists.

## 2. Item and target

The item and the claim are those of [safety-manual.md](safety-manual.md) §2 and
[sil2-readiness.md](sil2-readiness.md) §2: the library, its C interface and the C++
SDK, developed with the rigour of systematic capability SC 2. The requirements of
class S in [requirements-specification.md](requirements-specification.md) are the
safety-related ones.

## 3. Roles

| Role | Does | Held by |
|---|---|---|
| Owner | Decides requirements, public interfaces, releases; accountable for every change | The maintainer |
| Designer and implementer | Writes design, code and tests | The maintainer, with an LLM assistant |
| Verifier | Reviews each change against its requirement; reviews the documents | **Vacant.** Must not be the implementer of the change |
| Validator | Runs the qualification on the reference platform and signs the report | **Vacant.** |
| Assessor | Judges the whole against the standard | **Vacant.** External and independent of the project |

Rules:

- **Accountability.** A change is the owner's, whoever or whatever typed it. Text
  from an LLM assistant is a draft until the owner has read it and the gates of
  section 5 have passed. The assistant holds no role.
- **Independence, not yet.** With one person, verification is not independent. Until
  the verifier role is filled, every change is verified by the gates alone, and the
  release notes say so.
- **Competence, not yet.** No competence record exists. Each role holder records the
  training and experience that qualify them before an assessment.

## 4. Lifecycle

Each phase has an input, an output and a check that ends it. A change of any size
goes through the phases it touches, in this order.

| Phase | Output | Ends when |
|---|---|---|
| Requirements | A row of [requirements-specification.md](requirements-specification.md) with class and method; an entry in `traceability.toml` | `check.sh` accepts the trace; the owner approved the wording |
| Failure analysis | A row or an assumption of use in [failure-analysis.md](failure-analysis.md), when the change adds or changes a function of class S | The row names its measure and its evidence |
| Architecture and interface | [architecture.md](architecture.md), [interface-specification.md](interface-specification.md); for a C ABI change the header and `abi.revision` | `check.sh` (generated header, dependency rules) |
| Detailed design and code | The code, under `docs/rust-style.md` and the workspace lints | clippy without warnings; no new `unsafe` outside the named crates |
| Module test | Tests named for the behaviour they fix, the error path included | `cargo nextest`, `ctest`; coverage gate |
| Integration test | Multi-process tests; the installed SDK flow | `./test.sh` |
| Validation | The qualification report on the reference platform | `scripts/qualify.sh`; every class S requirement verified |
| Release | A tag, the report, the release notes | [release-policy.md](release-policy.md) §4 |
| Operation and modification | Defect reports, impact analyses | Sections 6 and 7 |

## 5. Verification

### 5.1 On every change

| Check | Command | Passes when |
|---|---|---|
| Style, generated files, dependency rules, advisories, traceability | `./check.sh` | Exit 0 |
| Static analysis | `cargo clippy --workspace --all-targets -- -D warnings` (in `test.sh`) | No warning |
| Tests | `./test.sh` | Every test passes |

### 5.2 Before a release, and after a change to `hypha_ffi`, `hypha_sys`, the transport or the run loop

| Check | Command | Passes when |
|---|---|---|
| Line coverage | `scripts/coverage.sh` | 80 % for each gated crate |
| Undefined behaviour in Rust | `scripts/miri.sh` | No finding |
| Memory and thread errors in C++ | `scripts/sanitize.sh address`, `thread` | No report |
| Fuzzing of every parser and decoder | `scripts/fuzz.sh 600` | No crash |
| The result per requirement | `scripts/qualify.sh` | Every class S requirement verified |

Not yet: these do not run in CI; coverage does not include `hypha_node` or the C++
SDK and does not measure branches ([sil2-readiness.md](sil2-readiness.md) G-23 to
G-25).

### 5.3 Validation

Not yet. Validation needs a reference platform (hardware, kernel, scheduling setup)
and a reference workload. Neither is defined, so NFR-03, NFR-04 and NFR-06 are
targets. The validation plan is written when the platform is chosen; its content is:
the configuration, the workload file, the runs (rated load, sustained overload,
endurance), the pass limits, and who signs. The rated-load workload and its limits exist
(`examples/load/rated.toml`, run by `scripts/rated-load.sh` and in `scripts/qualify.sh`),
and so does the sustained-overload workload (`examples/load/overload.toml` and its
latency variant, run by `scripts/overload.sh`), which waits for a Linux run before it
counts for NFR-05; the configuration and who signs do not.

### 5.4 Reviews

Not yet. Every document of `docs/` and every change to class S code needs a review by
the verifier, recorded with the change (the reviewer, the date, the findings and
their closure). Until the role is filled no review record exists.

## 6. Modification

Every change states, in its commit message or its pull request:

1. **What and why**: the requirement, the defect or the decision it serves.
2. **Impact**: the requirements, the assumptions of use, the C ABI, the wire frame
   and the documents it touches. "None" is an answer and must be written.
3. **Verification**: which checks of section 5 ran and their result.

Rules that follow from the impact:

| The change touches | Then |
|---|---|
| A requirement or an assumption of use | Update the specification and its change log, the failure analysis and the safety manual in the same change |
| The C ABI | Header, `abi.revision`, both SDKs and the tests together; a new release line |
| The wire frame or the recording format | A new release line |
| `hypha_ffi`, `hypha_sys`, the transport, the run loop | The checks of 5.2 |
| A dependency version | [dependency-assessment.md](dependency-assessment.md); `cargo deny`; the checks of 5.2 for the transport |
| The compiler or the builder image | The full gate and the checks of 5.2 |

Not yet: commits before this plan carry no impact statement.

## 7. Defects

A defect is reported as an issue on the repository. A defect that could lead an
application to use wrong, old or missing data without a fault indication is reported
privately first (the repository's security advisory form), so that users can be told
before the details are public.

| Class | Meaning | Handling |
|---|---|---|
| A | A fault that hypha claims to detect is not detected, or wrong data reaches an application without an indication; a violated class S requirement | Fix in every supported release line. A notice to users with the affected versions and a workaround. A regression test. An entry in the failure analysis. |
| B | A class S function fails in a detected way (a crash, a refused operation, a false fault) | Fix in the current release line. A regression test. |
| C | Anything else | Fix in the next release. |

Every fix first reproduces the defect in a test. The release notes list the defects
fixed and the known open ones with their class.

Not yet: no defect has been classified, and the project has no list of users to
notify.

## 8. Tools

Class by possible effect on the delivered code: T1 none; T2 can fail to detect a
defect; T3 can introduce one.

| Tool | Version control | Class | Justification today |
|---|---|---|---|
| `rustc`, LLVM, `cargo` | Not pinned: the latest stable (`rust-toolchain.toml`, the builder image) | T3 | None beyond the tests. Not qualified. |
| C++ compiler, standard library | The builder image | T3 | None beyond the tests. Not qualified. |
| Linker, CMake | The builder image | T3 | None beyond the tests. |
| `cbindgen` | `docker/Dockerfile` | T3 | The generated header is committed and compared on every change; the C test `test_c_abi.c` compiles against it. |
| `hypha_msggen` | In this repository | T3 | Developed under this plan; golden MD5 of every message, encode and decode round trips. |
| clippy | The toolchain | T2 | A missed finding is covered by the tests and the other analyses. |
| Miri, ASan, UBSan, TSan | Nightly toolchain of `docker/rust-nightly`; the compiler | T2 | Each is one of several independent detections. |
| `cargo-fuzz`, proptest | `docker/Dockerfile`; `Cargo.lock` | T2 | As above. |
| nextest, gtest, ctest | `docker/Dockerfile`; the builder image | T2 | A test that silently does not run would count as "not run" in the qualification report. |
| `cargo llvm-cov` | `docker/Dockerfile` | T2 | The measure is a gate, not evidence of correctness. |
| `cargo deny` | `docker/Dockerfile` | T2 | Advisory data is external. |
| `scripts/traceability.py` | In this repository | T2 | `scripts/test_traceability.py`. |
| git | — | T1 | |

`docker/Dockerfile` names the exact version of `cbindgen`, `cargo-nextest`,
`cargo-llvm-cov`, `cargo-deny`, `cargo-fuzz`, `clang-format`, `ruff` and `mypy`. The
packages of the distribution (the C++ compiler, CMake, gtest) come from its archive
when the image is built and are not pinned to a version; the base image is pinned by
digest.

Open: a qualified Rust toolchain, or the argument that the tests would reveal a
compiler defect ([sil2-readiness.md](sil2-readiness.md) G-15, G-17).

## 9. Configuration management

- Everything that makes a release is in the repository or pinned from it: sources,
  `Cargo.lock`, `fuzz/Cargo.lock`, `docker/Dockerfile` (base image by digest),
  `abi.revision`, the vendored crate with its patch (`vendor/`). The Rust compiler is
  not pinned: `rust-toolchain.toml` takes the latest stable, so the qualification report
  names the `rustc` a build used.
- A release is a tag ([release-policy.md](release-policy.md)). The qualification
  report names the commit and says when the tree had uncommitted changes.
- Generated files that are committed are compared with their generator on every
  change (`check.sh`): the C header, the MCAP reader header, `traceability.md`.

## 10. Documents

| Document | Phase |
|---|---|
| [requirements-specification.md](requirements-specification.md) | Requirements |
| [failure-analysis.md](failure-analysis.md) | Failure analysis |
| [architecture.md](architecture.md), [interface-specification.md](interface-specification.md), [communication-integrity.md](communication-integrity.md) | Architecture and interface |
| [traceability.md](traceability.md), the qualification report | Verification and validation |
| [dependency-assessment.md](dependency-assessment.md) | Pre-existing software |
| [safety-manual.md](safety-manual.md) | Use |
| [release-policy.md](release-policy.md) | Release, modification |
| [sil2-readiness.md](sil2-readiness.md) | The gaps against the standard |
