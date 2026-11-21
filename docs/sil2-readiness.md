# SIL 2 readiness assessment

What an assessor would ask of hypha for systematic capability SC 2 under
IEC 61508:2010, what hypha can show today, and what is missing.

Readers: the owner who decides whether to seek an assessment, and an engineer who
closes a gap.

## 1. Status

- hypha is SIL 0 software. This document claims no compliance. It is a
  self-assessment written by the project, not by an assessor.
- The clause and table numbers are those of IEC 61508-3:2010 as the author recalls
  them, without the text of the standard at hand. **Check every reference against the
  standard before this document goes to an assessor.** The recommendation level of a
  technique (R, HR) is deliberately not repeated here for that reason.
- Each "has" statement names its evidence. A statement without evidence is a gap.
- Companion documents: [failure-analysis.md](failure-analysis.md) (failure modes and
  the assumptions of use), [safety-manual.md](safety-manual.md),
  [safety-plan.md](safety-plan.md), [release-policy.md](release-policy.md),
  [dependency-assessment.md](dependency-assessment.md),
  [traceability.md](traceability.md) (requirement to test),
  [communication-integrity.md](communication-integrity.md) (frame checks).
- Sections 4 and 5 record the state before those companions were drafted
  (2026-11-19); the summary of section 3 and the table of section 8 carry the state
  after.

## 2. What can be assessed

A SIL belongs to a safety function of a system, and hypha has none: it carries
messages and reports faults, and the application decides what a fault means. What an
assessor can certify is narrower:

- **The item.** `libhypha_ffi`, the C ABI and the C++ SDK headers, as a software
  element for use in a safety function of another party (a compliant item).
- **The claim.** Systematic capability SC 2: the element was specified, built and
  verified with the rigour IEC 61508-3 asks for SIL 2, so that it may be used in a
  SIL 2 function when the integrator keeps the conditions of its safety manual.
- **Not in the item.** The tools (`hypha record`, `play`, `launch`, `echo`, the
  Foxglove adapter), the ROS 1 bridge, simulated time and replay, the Rust SDK of the
  tools, the examples. They are development and diagnosis aids. The safety manual must
  say that none of them takes part in a safety function.
- **Not hypha's.** The hazard and risk analysis of the system, the allocation of
  safety functions, the hardware, the operating system, the scheduler and the
  watchdog. The integrator owns them. hypha states what it assumes of them.

Two routes exist for an element (7.4.2.12). Code written from now on can follow the
standard from the start (route 1S). The code that exists was not developed under a
planned safety lifecycle, so it needs the assessment of a development that did not
follow the standard (route 3S, 7.4.2.13): the assessor judges the element from its
specification, its design records and its verification evidence, and asks for more
evidence where the process cannot be shown. Operating history (route 2S) does not
apply: hypha has no fielded hours.

## 3. Summary

| Area | State | Section |
|---|---|---|
| Functional safety management, roles, competence | Plan drafted; roles vacant, no competence record | 4.1 |
| Independent assessment and verification | Missing; cannot be produced by the author | 4.1, 6 |
| Safety lifecycle and planning | Drafted ([safety-plan.md](safety-plan.md)); applies from now, not to existing code | 4.2 |
| Software safety requirements | Partly: a tested requirements specification, not a safety one | 4.3, 7 |
| Validation plan | Missing | 4.4 |
| Architecture | Mostly there | 4.5 |
| Tools and languages | Partly; the compiler is not qualified | 4.6 |
| Detailed design and coding | Mostly there for Rust; partly for C++ | 4.7 |
| Module and integration testing | Mostly there; coverage measure too weak | 4.8 |
| Validation: timing, load, endurance | Missing | 4.9 |
| Modification procedure | Drafted; no record for past changes | 4.10 |
| Verification records | Partly: results exist, review records do not | 4.11 |
| Pre-existing software (iceoryx2, crates, C++ library) | Requirements on the transport drafted; two have no evidence | 4.12 |
| Safety manual | Drafted ([safety-manual.md](safety-manual.md)) | 4.13 |
| Communication (IEC 61784-3) | Partly: checks exist, no residual error argument | 4.14 |
| Non-interference between elements | Mostly there | 4.15 |

"Mostly there" means an assessor would ask for a record, not for new engineering.

Last complete qualification run, 2026-11-21, `scripts/qualify.sh` on the merge of every
open branch, in the builder image (Ubuntu 26.04, arm64, tools at their pinned versions),
from an empty build: `test.sh --clean`, `check.sh`, the Rust suite (1789 tests), the C++
suite, coverage, Miri, fuzzing (60 s a target), ASan with UBSan and TSan all passed. 47 of
51 requirements are verified; FSR-02, NFR-03, NFR-04 and NFR-06 have no test yet. The
TSan suppression (`called_from_lib:libhypha_ffi`) hid four reports in that run; in each,
both accesses are inside the uninstrumented library (`malloc` against
`pthread_mutex_lock`, and two reads in the transport's logger), none in the C++ SDK or
the tests. One author ran it; there is no review record (G-30).

## 4. Assessment by clause

### 4.1 Management of functional safety (IEC 61508-1 clause 6, IEC 61508-3 clause 6)

Asked: named persons for each role, evidence of their competence, a functional
safety plan, configuration management, a procedure for handling defects found in the
field, and an assessment by persons independent of the development.

| Has | Evidence |
|---|---|
| Configuration management | git; `Cargo.lock` pins every Rust dependency; `rust-toolchain.toml` pins the compiler; `abi.revision` |
| Change control by gates | `check.sh`, `test.sh`, CI (`.github/workflows/ci.yml`) |
| Dependency advisories and licences | `deny.toml`, `cargo deny` in `check.sh` |

Gaps:

- G-01. No functional safety plan, no role assignment, no competence record.
- G-02. One developer writes, reviews and releases. No independent verifier, no
  independent assessor. See section 6.
- G-03. Code and documents are written with an LLM assistant. No standard forbids
  this, but the assessor will ask who is accountable for each change and how it was
  reviewed. There is no review record per change.
- G-04. No defect procedure: no severity classes, no rule for informing users of a
  safety-relevant defect, no named owner of a production-blocking defect.
- G-05. Release baselines: one tag (`0.1.0`). No rule for what a release freezes.

### 4.2 Safety lifecycle (7.1)

Asked: a lifecycle chosen in advance, with the inputs, outputs and verification of
each phase.

Has: the V-model documents exist as artefacts (requirements, architecture, interface
specification, tests, traceability). Gap G-06: no document defines the phases, their
order and their exit criteria; the artefacts were written alongside the code.

### 4.3 Software safety requirements specification (7.2)

Asked: requirements derived from the system safety requirements, each one clear,
verifiable and traceable, covering the safety functions, the response to faults, the
interfaces, the modes, and the capacity and response times.

| Has | Evidence |
|---|---|
| 51 requirements with IDs and a verification method each | `requirements-specification.md` |
| Each traced to tests, checked by machine | `traceability.toml`, `scripts/traceability.py`, `check.sh` |
| Fault response stated | FSR-06, FSR-10, FSR-11, FSR-27, FSR-39, NFR-09 |

Gaps:

- G-07. No requirement is classified as safety-related or not. Every row carries the
  priority "Reliability". An assessor needs the safety-related subset named.
- G-08. The requirements do not come from a hazard analysis, because hypha has no
  system. [failure-analysis.md](failure-analysis.md) is the substitute: failure modes
  of the element and the assumptions of use. It is a first issue and unreviewed.
- G-09. Several requirements are not verifiable as written, contradict another
  requirement, or describe a past state. Section 7 lists them.
- G-10. Three requirements have no evidence at all: NFR-03, NFR-04, NFR-06.

### 4.4 Validation plan (7.3)

Asked: before validation, a plan that states what is validated, on which
configuration, by which method, and the pass criteria.

Gap G-11: no plan. The pass criteria of timing and load are stated as requirements
(NFR-03, NFR-04, NFR-06) but no reference configuration (hardware, kernel, scheduling
setup) is defined, so the numbers cannot be validated anywhere.

### 4.5 Architecture (7.4.3)

Asked: an architecture that states the elements, their interfaces, the measures for
fault detection and fault containment, and the treatment of elements of lower
integrity.

| Has | Evidence |
|---|---|
| Layering with a machine-checked dependency rule | `architecture.md` §2, `scripts/check-deps.sh` |
| One interface for every binding | `interface-specification.md`, NFR-01 |
| Fail-stop on an internal fault, no unwinding across the boundary | `architecture.md` §3.3, NFR-09 |
| Process isolation and non-blocking publication | `architecture.md` §3.1, §3.2 |
| Frame checks on every receive, no optional profile | `architecture.md` §3.4, FSR-09, FSR-10 |
| No thread created by the framework | FSR-14, `hypha_iceoryx2/housekeeping_creates_no_thread` |
| Bounded queues and pools | NFR-05, `interface-specification.md` (service limits) |

Gaps:

- G-12. No statement of which elements are safety-related. The tools link the same
  core; the architecture must show they cannot affect a running safety function.
  Known coupling: a tool is a participant of the domain with the same UID and can
  publish on any topic.
- G-13. A growable publisher is not a memory bound (FSR-06 and NFR-05 hold only for a
  topic with a payload limit).
- G-14. No deadline guarantee and no worst-case execution time. This is a design
  property, not a defect; the safety manual must state it and the integrator must
  supply the watchdog (4.13).

### 4.6 Support tools and programming languages (7.4.4)

Asked: a suitable, strongly typed language with a coding standard; each tool classed
by its possible effect on the executable (T1 none, T2 fails to detect an error, T3
can introduce one); T3 tools justified by certification, by history of use or by
measures that would catch their errors.

| Tool | Class | State |
|---|---|---|
| `rustc` 1.98.0, LLVM, `cargo` | T3 | Pinned. Not qualified. |
| C++ compiler and standard library of the builder image | T3 | Pinned by the image. Not qualified. |
| `cbindgen` (generates `hypha_ffi.h`) | T3 | Output committed and compared in `check.sh`. |
| `hypha_msggen` (generates message types) | T3 | Golden MD5 of every message, round-trip tests (FSR-04, FSR-21). |
| CMake, linker | T3 | Not assessed. |
| clippy, Miri, sanitizers, fuzzers, nextest, gtest, llvm-cov | T2 | Not assessed. |
| `scripts/traceability.py` | T2 | Own tests (`scripts/test_traceability.py`). |

Gaps:

- G-15. The Rust compiler is not qualified. A qualified Rust toolchain exists
  (Ferrocene, qualified by TÜV SÜD for IEC 61508); whether its current release and
  scope cover the pinned language version and the standard library hypha uses must be
  checked with the vendor. hypha uses `std`, threads of the host, and allocation.
- G-16. No tool classification record and no justification per tool. Drafted since:
  [safety-plan.md](safety-plan.md) §8.
- G-43. The builder image installed `cbindgen`, `cargo-nextest`, `cargo-llvm-cov`,
  `cargo-deny`, `cargo-fuzz`, `clang-format` and `ruff` without an exact version, so
  two builds of the image differed: a fresh image reformatted 32 files of an unchanged
  tree. Pinned in `docker/Dockerfile` on 2026-11-19.
- G-17. The alternative to a qualified compiler is to show that the tests would catch
  a compiler error. That argument needs coverage of the object code paths (4.8).

### 4.7 Detailed design and coding (7.4.5, 7.4.6)

Asked: modular design, a coding standard, defensive programming, limited use of
dynamic objects, pointers and recursion, no unconditional jumps, bounded complexity.

| Has | Evidence |
|---|---|
| Coding standard for Rust, enforced by the compiler and clippy | `docs/rust-style.md`, workspace lints in `Cargo.toml` |
| No `unsafe` outside two named crates and two named modules | `unsafe_code = "forbid"`; NFR-02 |
| No panic, unwrap, unchecked index or unchecked arithmetic in shipped code | lints `panic`, `unwrap_used`, `indexing_slicing`, `arithmetic_side_effects` denied; NFR-08 |
| Explicit status on every C ABI call, checked boundary values | `interface-specification.md`, `hypha_ffi/abi_boundary_values` |
| Size limits | `architecture.md` §5.3, `check.sh` (C++ headers at most 400 lines) |
| Undefined behaviour search | Miri over the transport-free crates; ASan, UBSan and TSan over the C++ tests |

Gaps:

- G-18. `hypha_ffi` holds about 230 `unsafe` blocks and `hypha_sys` 13. NFR-02 asks
  that this part is independently audited. No audit record exists.
- G-19. The C++ SDK has no stated coding standard beyond the two rules `check.sh`
  enforces (no default lambda capture, no `decltype(auto)`), and no static analyser
  runs on it. A SIL 2 claim that includes the headers needs a named subset and a
  checker. The SDK requires C++23 (`cxx_std_23`); the published subsets stop earlier
  (MISRA C++:2023 covers C++17, the AUTOSAR guidelines C++14), so either the SDK is
  restricted to an older language level or the project writes and justifies its own
  subset.
- G-20. Dynamic allocation happens after start-up (growable messages, decoded C++
  messages, queues of the SDK). There is no statement of where allocation occurs on
  the cyclic path and no measure that bounds it.
- G-21. Miri cannot run a node, `hypha_ffi` or `hypha_sys`, so the code with the most
  `unsafe` is the code it does not check. The sanitizers instrument the C++ side only.
- G-22. No complexity measure is recorded or limited.

### 4.8 Module and integration testing (7.4.7, 7.4.8, 7.5)

Asked: tests derived from the specification, boundary values, error cases, a
structural coverage measure with a stated target and a justification of what is not
covered, integration tests of the elements and of software with hardware.

| Has | Evidence |
|---|---|
| 1785 Rust tests and 506 C++ tests, run on every change | `test.sh`, CI |
| Tests named by the behaviour they fix, one per requirement clause where possible | `traceability.md` |
| Property tests of every parser and decoder | `*_properties` test binaries; NFR-08 |
| Fuzz targets for the same parsers | `fuzz/`, `scripts/fuzz.sh` |
| Multi-process tests: start order, kill of a peer, cross-language | FSR-25, FSR-27, FSR-01 |
| Line coverage gate of 80 % for ten core crates | `scripts/coverage.sh`; NFR-10 |
| Tests through the installed SDK | `scripts/test-installed-sdk.sh` |

Gaps:

- G-23. Line coverage of 80 % is below what SIL 2 expects. The standard asks for
  full coverage of entry points and statements, with branch coverage recommended, or
  a justification for each uncovered part. There is no branch measure, no list of
  uncovered code, and no justification.
- G-24. `hypha_node` (about 11 000 lines, the node runtime) is not in the coverage
  gate. The C++ SDK has no coverage measure at all.
- G-25. Coverage, Miri, the fuzz targets and the sanitizers run in CI nightly on
  `develop` (the `qualification` job, its report an artifact), not on a pull request:
  they judge a change up to a day after it is merged.
- G-26. No test specification document: the tests are the specification. An assessor
  accepts this when each test states its requirement; the trace is by test binary,
  not by test case.
- G-27. Integration with hardware (7.5) is not applicable to hypha alone and must be
  restated as integration on the reference platform (4.4).

### 4.9 Validation (7.7)

Asked: evidence that the software meets its safety requirements on the target
configuration, including timing and capacity.

Gap G-28: no measurement of latency, of rated load or of endurance on Linux
(NFR-03, NFR-04, NFR-06), and no measurement of memory under sustained overload
(NFR-05, second half). This is the largest technical gap.

### 4.10 Modification (7.8)

Asked: a procedure for changes: impact analysis, the lifecycle phase to return to,
re-verification, and a record.

Has: the rule that an ABI change updates the header, `abi.revision`, the SDKs and the
tests together (`AGENTS.md`), enforced in part by `check.sh`. Gap G-29: no impact
analysis record per change and no rule for which tests a change must repeat beyond
"the full gate".

### 4.11 Verification (7.9)

Asked: for each phase, a record that its output was verified against its input, by
whom and with what result; static analysis; review of the code.

| Has | Evidence |
|---|---|
| Static analysis on every change | clippy with the pedantic set and the denied lints, `cargo deny` |
| Dynamic analysis | Miri, sanitizers, fuzzing (by hand) |
| A machine-made result per requirement | `scripts/qualify.sh`, `target/qualification/report.md` |

Gaps:

- G-30. No review records: not of the requirements, the architecture, the code or
  the tests.
- G-31. The qualification report is not yet attached to a release.

### 4.12 Pre-existing software (7.4.2.12, 7.4.2.13)

Asked: for every element not developed under the standard, a specification of what
it must do, evidence that it does it, and an analysis of its failure modes and of the
functions it has that are not needed.

`libhypha_ffi` links about 60 third-party crates. The ones that matter:

| Element | Role | State |
|---|---|---|
| iceoryx2 0.10.1 (28 crates), with one vendored patch | The whole transport: shared memory, lock-free queues, discovery, events | Treated as a black channel for data (4.14). Not assessed for the properties hypha relies on: no blocking, bounded memory, crash cleanup, read-only mapping. |
| `crc32c` | The frame checksum | Not assessed. |
| `toml`, `serde`, `winnow` | Configuration parsing at start-up | Fuzzed through `config_load`. Not assessed. |
| `libc`, `rustix`, `nix`, `memmap2` | Operating-system calls | Not assessed. |
| Rust `std`, `core`, `alloc` | Runtime library | Not assessed (see G-15). |
| C++ standard library | The SDK | Not assessed. |
| Linux, the MMU, POSIX shared memory | Platform | Outside the item; an assumption of use. |

Gaps:

- G-32. No specification of what hypha requires of iceoryx2, and no evidence beyond
  hypha's own tests. The black-channel argument covers corruption, loss, repetition,
  reordering and delay of data. It does not cover a transport that blocks the
  publisher, exhausts memory, corrupts the receiver's own memory, or fails to release
  the resources of a dead process. iceoryx2 contains its own `unsafe` code and
  lock-free algorithms in the address space of every node.
- G-33. Whether a safety-certified variant of iceoryx2 exists or is planned, for
  which standard and SIL, and for which version, must be asked of its vendor; hypha
  pins a patched 0.10.1.
- G-34. No list of unused functions of the dependencies and no argument that they
  cannot be reached.

### 4.13 Safety manual (Annex D)

Asked: a manual for the integrator: what the element does, its configuration, its
failure modes and their detection, the constraints on its use, and what the
integrator must do.

Has, spread over six documents: the checks and their limits
(`communication-integrity.md`), the failure behaviour of a cycle
(`execution-model.md` §6.3, §6.4), the trust boundary (`architecture.md` §3.4), the
deployment constraints (`containerized-deployment.md`), the recurring faults
(`engineering-traps.md`), the C ABI contract (`interface-specification.md`).

Gap G-35: no single safety manual. [failure-analysis.md](failure-analysis.md) §4
lists the assumptions of use it must contain. The manual must also carry the
identification of the item (version, ABI revision, toolchain, platform) and the
systematic capability claimed.

### 4.14 Communication (IEC 61784-3, black channel)

Asked: a measure for each communication error, and a calculated residual error rate
that fits the budget of the safety function.

| Error | Measure | State |
|---|---|---|
| Corruption | CRC-32C over header and payload | Has |
| Unintended repetition | Sequence per publisher | Has |
| Incorrect sequence | Sequence per publisher | Has |
| Loss | Sequence gap counted; silence limit on a cycle input | Has; detection only |
| Unacceptable delay | Send time on the host monotonic clock, `silence_ms` | Has for cycle inputs; a plain subscriber is not supervised |
| Insertion | Publisher identity and sequence inside the CRC | Partly: no list of permitted senders |
| Masquerade | A frame must be sealed for the topic it arrives on | Partly: any process of the domain may publish on a topic |
| Addressing | The CRC starts from a code of the topic; a frame of another topic is refused | Has (2026-11-20) |

Gaps:

- G-36. No residual error rate calculation: none for the CRC at the frame sizes in
  use (the Hamming distance of a 32-bit CRC falls as the frame grows, and payloads
  reach tens of megabytes), and none for the sequence and time checks.
  A first-order estimate, to be checked against IEC 61784-3 and with an assessor:
  the standard bounds the rate of undetected errors by Λ = R × v × m (R the
  probability that a corrupted frame passes, v frames per hour, m receivers) and
  allows communication about 1 % of the budget of the safety function, which for
  SIL 2 is about 10^-8 per hour. With the default assumption for a black channel, a
  bit error probability of 10^-2, and nothing else, almost every frame counts as
  corrupted, R of a 32-bit CRC is about 2^-32, and v × m must stay below about 40
  frames per hour. Established SIL 2 safety layers nevertheless use a 32-bit safety
  code. They meet the budget with three measures hypha lacks: a short, bounded safety
  message, so that the code keeps a high Hamming distance; a polynomial chosen for
  that length; and a **channel monitor** that counts refused frames in a window and
  declares the channel unsafe above a threshold, which bounds the rate of corrupted
  frames that the code ever has to catch. hypha has none of the three: its payloads
  reach tens of megabytes, CRC-32C was chosen for speed, and every refused frame is
  independent (`communication-integrity.md` §5). The choice for step 13 was between
  that design (bounded safety topics and a channel monitor, 32 bits kept) and a
  64-bit checksum for every frame. The owner took neither (2026-11-19): no payload
  limit, no 64-bit checksum, no channel monitor. G-36 stays open: the calculation
  is to be written, and a frame-size or rate bound that it needs becomes an
  assumption of use for the integrator, not a change of the frame.
- G-37. Closed for addressing on 2026-11-20: the CRC starts from a code of the
  topic, so a frame is bound to its topic. Still open: nothing binds a frame to its
  sender (`communication-integrity.md` §5).
- G-38. The first frame of a publisher is trusted, and there is no reaction to
  repeated corruption.
- G-39. The closed-system assumption (one UID is one trust domain) is a security
  assumption a safety case must import from a security assessment (IEC 62443).

The alternative to closing G-36 to G-38 inside hypha would be to call hypha a
non-safety channel under an application safety layer. The owner decided against it
(section 8, step 1): hypha is the safety communication layer, so G-36 to G-38 are
work to do.

### 4.15 Non-interference (Annex F)

Asked: when elements of different integrity share a computer, evidence that one
cannot disturb another in memory, in time or through shared resources.

Has: one process per node; no shared mutable memory between hypha nodes except the
connection segments; a publisher never waits for a reader; a full queue drops for its
own subscriber; a killed peer does not disturb the survivors (FSR-27, tested with
SIGKILL of a publisher, a reader and a loan holder).

Gaps:

- G-40. One `/dev/shm` volume for the domain: a volume that fills kills the writer
  with SIGBUS, and the writer can be the safety-related node.
- G-41. Callbacks of one `WaitSet` share one thread; a slow callback delays every
  other. A stalled callback is not detected inside the process.
- G-42. The kill tests do not cover a publisher killed with an unsent loan, or a
  process killed between two sends of one cycle step (FSR-27 gap).

## 5. Techniques

The techniques of Annex A and Annex B, and what hypha applies. The selection for
SIL 2 must be justified against the tables of the standard (section 1).

| Technique | hypha | Evidence |
|---|---|---|
| Strongly typed language | Yes | Rust; C++23 for the SDK |
| Language subset | Rust: yes, by lints. C++: no | `Cargo.toml` lints; G-19 |
| Certified or proven translator | No | G-15 |
| Coding standard | Rust: yes. C++: partly | `docs/rust-style.md`; G-19 |
| Defensive programming | Yes | Checked arguments and explicit status at the C ABI |
| Modular approach, information hiding | Yes | Crate layering, `check-deps.sh` |
| Fault detection and diagnosis | Yes | Frame checks, counters, `~/state` |
| Error detecting codes | Yes | CRC-32C |
| Failure assertion programming | Partly | Fail-stop on internal fault; no plausibility checks of payload values |
| Graceful degradation | Application's | Verdicts `Usable`, `Stale`, `NeverArrived`; the step decides |
| Static resource allocation | Partly | Fixed queues and pools; growable messages and heap use remain |
| Static analysis | Rust: yes. C++: no | clippy; G-19 |
| Dynamic analysis | Yes, by hand | Miri, ASan, UBSan, TSan |
| Boundary value analysis | Yes | `hypha_ffi/abi_boundary_values`, property tests |
| Equivalence classes, input partition | Partly | Property tests of parsers |
| Fuzz testing (error guessing, fault injection) | Partly | Parsers only; no fault injection into shared memory in the repository |
| Structural coverage | Partly | Lines, 80 %, ten crates; G-23, G-24 |
| Performance testing | No | G-28 |
| Interface testing | Yes | The C ABI test binaries of `hypha_ffi`, the C test `test_c_abi.c` |
| Formal or semi-formal methods | No | None |
| Forward and backward traceability | Requirement to test: yes. Requirement to design and code: no | `traceability.toml` |
| Software FMEA | First issue | `failure-analysis.md` |
| Impact analysis on modification | No | G-29 |
| Configuration management | Yes | git, lock files, pinned toolchain |

## 6. What paperwork cannot close

1. **Independence.** An assessment needs an assessor who did not take part in the
   work, and verification needs a second competent person. Neither can be written.
2. **A planned process for the existing code.** The lifecycle was not planned before
   the code was written. Route 3S exists for this case, and it costs more evidence.
3. **The compiler.** Either a qualified toolchain that covers what hypha uses, or an
   argument from tests with far stronger coverage.
4. **iceoryx2.** Either a certified transport, or an assessment of it as
   pre-existing software, or an architecture in which its failure cannot violate a
   safety requirement undetected. The third is the cheapest and needs G-36 to G-38
   and a detection of a blocked publish.
5. **Timing.** hypha gives no deadline guarantee and does not want one. A SIL 2
   function with a response time therefore always needs an external watchdog. This is
   an assumption of use, and it limits where hypha can be used.
6. **Measured evidence.** Latency, load and endurance must be measured on a reference
   platform.

## 7. Requirements: what was corrected and what is open

Corrected on 2026-11-19 (the change log of
[requirements-specification.md](requirements-specification.md) §5 has each one): the
class column S or N in place of a priority that was "Reliability" everywhere; the
withdrawn IDs; the sentence of §1 on named languages and formats; the contradiction
between NFR-08 and NFR-09; FSR-22 against FSR-02; dated corrections moved out of the
requirement text; a number for FSR-03; the wording of FSR-14; the condition of NFR-05;
the unverifiable sentence of NFR-13; NFR-03, NFR-04 and NFR-06 marked as targets.

Still open:

| ID | Open point |
|---|---|
| all | The owner confirmed the class of each requirement on 2026-11-21. An integrator may still need another split. |
| NFR-02 | Asks that the `unsafe` part is independently audited. No audit exists (G-18). |
| NFR-10 | 80 % line coverage; below a SIL 2 expectation and not applied to `hypha_node` or the C++ SDK (G-23, G-24). |
| NFR-03, NFR-04, NFR-06 | Need a reference platform and a reference workload. The rated load of NFR-04 is still the figures of one application. |
| FSR-06 | Holds for a topic with a payload limit; a deployment mode that refuses a topic without one would turn AoU-06 into a check. |

## 8. Work, in order

The order puts first what decides whether the rest is worth doing.

| Step | Work | Closes | State |
|---|---|---|---|
| 1 | Decide the claim: a safety-related channel (hypha's checks are the measure) or a plain channel under an application safety layer (4.14). | Scope of G-36 to G-39 | Decided by the owner, 2026-11-19: iceoryx2 is the plain channel, hypha is the safety communication layer. Step 13 is therefore required. |
| 2 | Talk to an assessor with these documents. One day of their time fixes the route and the scope before more is written. | G-02, section 6 | Open; external |
| 3 | Requirements: the corrections of section 7, the classification, withdrawn IDs. | G-07, G-09 | Done, first issue; the open points of section 7 remain |
| 4 | Safety manual from the assumptions of use. | G-35, G-14 | Drafted: [safety-manual.md](safety-manual.md) |
| 5 | Safety plan: roles, lifecycle, verification, modification, defects; release policy. | G-01, G-04 to G-06, G-29 | Drafted: [safety-plan.md](safety-plan.md), [release-policy.md](release-policy.md). The roles of verifier, validator and assessor are vacant. |
| 6 | Tool classification and justification. | G-16 | Drafted: [safety-plan.md](safety-plan.md) §8. Found G-43. |
| 7 | Dependency assessment: requirements on the transport and the tests that show them. | G-32, G-34 | Drafted: [dependency-assessment.md](dependency-assessment.md). TR-11 and TR-12 have no evidence. |
| 8 | Reference platform and workload; load harness; latency, overload and endurance runs. | G-10, G-11, G-28 | Open; engineering |
| 9 | Coverage: add `hypha_node` and the C++ SDK, measure branches, justify the rest. | G-23, G-24 | Open; engineering |
| 10 | CI jobs for coverage, Miri, fuzzing and the sanitizers; the report attached to a release; tool versions pinned in the builder image. | G-25, G-31, G-43 | CI job nightly on `develop` (#14, #71); the report attached to a release (release-policy.md §4); the rest open; engineering |
| 11 | Audit of the `unsafe` code by a second person; a C++ subset and a checker for the SDK. | G-18, G-19 | Open; engineering, external |
| 12 | The remaining kill tests; a fault-injection test that corrupts shared memory. | G-42, TR-04, TR-11 | Open; engineering |
| 13 | The missing parts of the safety communication layer. The owner chose the topic-bound checksum only (2026-11-19): no payload size limit, no 64-bit checksum, no channel monitor. Left: the residual error calculation, a binding of the frame to its sender, a rule for the first frame of a publisher. | G-36 to G-38 | Topic-bound checksum done (G-37 for addressing); the rest open; engineering |
| 14 | Qualified compiler, or the argument from tests. | G-15, G-17 | Open; external |
| 15 | Review records for every document and for the code. | G-30 | Open; needs a verifier |

Every document drafted in steps 3 to 7 has one author and no review. Each counts as
evidence only after step 15.
