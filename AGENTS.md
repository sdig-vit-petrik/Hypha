# hypha agent instructions

hypha is a SIL0 robotics middleware framework. Rust owns the core and C ABI;
C++ is the primary application SDK. `CLAUDE.md` links to this file, which
overrides parent agent instructions. Claude Code, Codex, GitHub Copilot, Gemini
CLI and Antigravity use these same rules.

## Work

- Start with the task and the relevant code. In this indexed checkout, use `codegraph explore` before text search when locating code; skip it when `.codegraph/` is absent.
- Make the smallest complete change. Read the linked document only when its topic applies. Do not run a review protocol, install skills, or delegate by default.
- Before writing code for a feature, new file, or ABI change, answer four placement questions: (1) which existing crate, module, and type owns this (verify with `codegraph explore` when `.codegraph/` is present), (2) why extending it will not suffice if adding a sibling type, module, or symbol, (3) which layer holds the logic vs. the thin wrapper, and (4) what existing helpers are reused and what old code is removed. Structure needs the owner's approval before editing (a new module, a new public C ABI symbol, or more than 200 net production lines, tests and docs excluded); implementation and behavior choices inside an approved plan stay delegated.
- Guard against bloat: prefer extending an existing or table-driven test over adding a new standalone test file or duplicating setup helpers; when a large test file or reference doc reaches its cap, split it by topic (`<suite>/<topic>.rs`, `docs/<topic>/<subtopic>.md`) rather than growing a monolith. Keep bug-fix regression tests focused; keep `docs/design/` notes ≤300 lines and fold or delete them once implemented. File-size ceilings and boundary rules are enforced by `scripts/check-size-and-boundaries.sh` via `./check.sh`.
- Optional `hypha-rust` and `hypha-cxx` specialists may take bounded, independent assignments. The orchestrator sets file ownership and the C ABI contract first, then owns integration and the final checks.
- Preserve the user's unrelated changes. Do not commit unless asked.
- Work belongs to a GitHub issue. Name the branch `<type>/<issue>-<slug>` and end each commit message with `Refs #N`, or `Fixes #N` when it finishes the issue (`docs/release-policy.md` §7). Keep commit subjects imperative, ≤72 characters, in `<scope>: <what>` form (plain technical prose).
- Update affected API or operating documentation in the same change. Report what changed, the checks run, and any remaining risk.
- Stay inside this repository. A path outside it needs the owner's line in `.agent-allowed-paths` or `.agent-allowed-paths.local`; ask for it.
- Hooks enforce part of these rules (`scripts/agent-hooks/README.md`). A blocked call means: follow the message; never work around it. Once per checkout: `./scripts/agent-hooks/install.sh` (links vendor configs and sets `core.hooksPath`); Codex needs its hooks trusted in `/hooks`; do not use Antigravity's allow-all mode here. The Claude Code hooks are tested; the other agents' are written from their docs and untested.

## Architecture

- `crates/hypha_node` alone depends on `hypha_iceoryx2`, and no Rust crate depends on `hypha_ffi`; `scripts/check-deps.sh` checks the rules. It holds the node runtime as safe Rust functions; `crates/hypha_ffi` holds only the C wrappers over them. Nodes, apps, tools, examples, and the C++ SDK must use framework APIs.
- Rust tools depend on `hypha_node` alone and use its Rust SDK, `hypha_node::sdk` (message types through `hypha_node::sdk::msgs`), not core or transport crates. `hypha_msggen` is the message generation exception.
- Shared runtime logic belongs below the C ABI in Rust. Public C++ headers use the C ABI through `include/hypha/backend/`; C++ stays a thin facade. Keep the dynamic `libhypha_ffi` as the deliverable.
- C++ SDK ergonomics and performance take priority. Keep existing Rust bindings correct when the ABI changes, but new capabilities need no matching Rust facade in the same change.
- Replace old mechanisms rather than add compatibility paths. An ABI change updates the header, `abi.revision`, affected SDKs, and tests together.
- One process uses one SDK: never load the C++ SDK's `libhypha_ffi` and link the Rust SDK in the same process. Each copy would hold its own simulated clock, log engine and signal handler.
- Each installed include directory has one owner. Message headers sit at `<package>/<Name>.h`: `hypha-cxx` owns the directory of each standard package it ships (`std_msgs/`, ..., `hypha_msgs/`); an application owns the directories of the packages it declares with `hypha_generate_messages()`. An application adds to a package of the SDK with that call too (`<package>_additions`): its headers go to its own output directory, never into the SDK's.
- Fail explicitly on invalid or stale data. Do not invent values or silently discard a fallible result. Rust code under `crates/` forbids unsafe: `unsafe` only in `hypha_ffi` (the C boundary), `hypha_sys` (OS calls), the test-only module `hypha_testkit/src/env.rs` and the C boundary module `hypha_testing_mcap/src/ffi.rs` of the test-only MCAP reader; the Rust SDK `hypha_node::sdk` calls its safe functions (`hypha_node::safe`) and forbids `unsafe`; shipped paths obey the workspace panic and arithmetic lints.
- For iceoryx2, use the thread-safe service, shared QoS builders and bounded service-open retry. Build a `WaitSet` once per run loop. Wildcard subscriptions resolve once at creation.

### Single-owner map

Put new code in the one layer that owns its concern; never bolt parallel policy onto a caller or wrapper.

| Concern | Single owning crate / module | Never put in |
|---|---|---|
| Wire frame, codec, CRC | `hypha_wire` | `hypha_iceoryx2`, `hypha_node`, `hypha-cxx` |
| Time, duration, cadence, clocks | `hypha_time` | ad-hoc clock math in `hypha_ffi` / `hypha-cxx` / `apps/` |
| `.msg`/`.srv` parsing, MD5, codegen | `hypha_msgdef`, `hypha_msggen` | runtime crates or `apps/` |
| Frame validation, params, ports, introspection | `hypha_core` | `hypha_ffi`, `hypha-cxx`, `apps/` |
| Cycle history, capture, input supervision | `hypha_cycle` | `hypha_ffi`, `hypha-cxx` |
| Schema cache, replay rules, TF buffer | `hypha_runtime` | `hypha_ffi`, `hypha-cxx`, `apps/` |
| Logging engine | `hypha_log` | `hypha_ffi`, `apps/` |
| iceoryx2 services, QoS, loans | `hypha_iceoryx2` | any other crate or `apps/` |
| Node runtime, WaitSet, safe Rust API & SDK | `hypha_node` (`safe`, `sdk`) | `hypha_ffi` (C wrappers only) |
| C ABI shims & raw-pointer conversion | `hypha_ffi` (thin wrappers, ≤300 lines/file) | policy, state machines, or retry loops |
| C++23 ergonomics | `crates/hypha-cxx` (thin facade, ≤400 lines/header) | runtime logic or OS threads |
| CLI tools, loadgen, ROS bridge, OS threads | `apps/*` (via `hypha_node::sdk` only) | `crates/*/src/` (no `thread::spawn`) |

## Rust style

- Rust code is written in idiomatic Rust: iterators, combinators, `?`, `let-else` and closures where they read naturally. This overrides any global "embedded C" or lambda-restriction style rules for Rust in this repository.
- House rules, lint configuration and the SCRC mapping: `docs/rust-style.md`.

## Quality checks

- For a bug, reproduce the wrong behavior before changing it. Keep a focused regression test when it can catch the failure again. Test the relevant error path as well as success when behavior changes.
- **Testing pyramid (onion layers, `docs/architecture.md` §7):** each ring tests only what it adds — test logic matrices and state machines once at the lowest pure layer (`hypha_wire`, `hypha_time`, `hypha_core`, `hypha_cycle`, `hypha_runtime`), using table-driven tests or `proptest` over copy-pasted scenarios. Keep `hypha_ffi` and `hypha-cxx` tests to boundary contracts (null/invalid args, status mapping, ownership/drop) and thin wiring smoke tests; do not repeat the same permutation matrix across layers. Reuse shared fixtures in `hypha_testkit` and lean on fuzzing, Miri, and sanitizers for malformed-input and memory-safety coverage.
- For a C ABI change, specify buffer sizes, ownership, null behavior, and destruction beside the affected declarations and wrappers. Exercise the affected C++ and existing Rust bindings; use the installed SDK flow when installation or headers change.
- For transport and concurrency changes, check service creation races, loan lifetimes, and shared-memory state. Use the relevant entries in `docs/engineering-traps.md` before changing these paths.
- Inspect the final diff for unintended API, dependency, and documentation changes. Do not weaken lint rules to make a change pass.
- Always run tests outside a snadbox environment

## Where to look

| Task                                          | Read when needed                                              |
| --------------------------------------------- | ------------------------------------------------------------- |
| Layering or C ABI                             | `docs/architecture.md`, `docs/interface-specification.md` |
| A new public API, key, message field or flag  | `docs/features.md` (one question, one answer)             |
| C++ API or message generation                 | `docs/cpp-api.md`, `docs/getting-started.md`              |
| Execution, cycles, timing                     | `docs/execution-model.md`                                   |
| Transport, IPC, schema or a recurring failure | `docs/engineering-traps.md`                                 |
| Tools and integration                         | `docs/tools.md`, `scripts/test-installed-sdk.sh`          |
| Safety and requirements                       | `docs/requirements-specification.md`                        |

## Verification

- **Fast local loops:** `./check.sh` (seconds, no build), `cargo test -p <crate>` (pure core crates run in <2 s), `ctest --test-dir build -R <regex>` (single C++ suite), `./scripts/mutants.sh --diff` (PR mutation check), `./scripts/repo-health.sh` (health dashboard).
- During edits, run the smallest relevant test or build. Use `./scripts/format.sh` to format and `./check.sh` for style and generated-file checks.
- For a local code change, run its affected build and test. Run `./test.sh rust` for Rust workspace or FFI changes. For integration, transport, ABI, build-system, or release changes, run the full gate: `./test.sh --clean` and `./check.sh`. Report skipped checks; reserve “all green” for the full gate.
- Transport tests need writable iceoryx2 shared-memory paths. On Linux these include `/dev/shm` and `/tmp/iceoryx2`; on macOS, `/tmp/iceoryx2`. After a capacity change, clear stale segments with `./clean-shm.sh`.
- Verify behavior with a binary or bus traffic when the change affects runtime behavior. A source review alone has missed replay ordering, topic rates, and stalled UI loops here.
