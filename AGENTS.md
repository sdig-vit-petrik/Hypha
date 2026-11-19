# hypha agent instructions

hypha is a SIL0 robotics middleware framework. Rust owns the core and C ABI;
C++ is the primary application SDK. `CLAUDE.md` links to this file, which
overrides parent agent instructions. Claude Code, Codex, GitHub Copilot, Gemini
CLI and Antigravity use these same rules.

## Work

- Start with the task and the relevant code. In this indexed checkout, use `codegraph explore` before text search when locating code; skip it when `.codegraph/` is absent.
- Make the smallest complete change. Read the linked document only when its topic applies. Do not run a review protocol, install skills, or delegate by default.
- Optional `hypha-rust` and `hypha-cxx` specialists may take bounded, independent assignments. The orchestrator sets file ownership and the C ABI contract first, then owns integration and the final checks.
- Preserve the user's unrelated changes. Do not commit unless asked.
- Update affected API or operating documentation in the same change. Report what changed, the checks run, and any remaining risk.
- Stay inside this repository. A path outside it needs the owner's line in `.agent-allowed-paths` or `.agent-allowed-paths.local`; ask for it.
- Hooks enforce part of these rules (`scripts/agent-hooks/README.md`). A blocked call means: follow the message; never work around it. Once per checkout: `git config core.hooksPath .githooks`; Codex needs its hooks trusted in `/hooks`; do not use Antigravity's allow-all mode here. The Claude Code hooks are tested; the other agents' are written from their docs and untested.

## Architecture

- `crates/hypha_node` alone depends on `hypha_iceoryx2`, and no Rust crate depends on `hypha_ffi`; `scripts/check-deps.sh` checks the rules. It holds the node runtime as safe Rust functions; `crates/hypha_ffi` holds only the C wrappers over them. Nodes, apps, tools, examples, and the C++ SDK must use framework APIs.
- Rust tools depend on `hypha_node` alone and use its Rust SDK, `hypha_node::sdk` (message types through `hypha_node::sdk::msgs`), not core or transport crates. `hypha_msggen` is the message generation exception.
- Shared runtime logic belongs below the C ABI in Rust. Public C++ headers use the C ABI through `include/hypha/backend/`; C++ stays a thin facade. Keep the dynamic `libhypha_ffi` as the deliverable.
- C++ SDK ergonomics and performance take priority. Keep existing Rust bindings correct when the ABI changes, but new capabilities need no matching Rust facade in the same change.
- Replace old mechanisms rather than add compatibility paths. An ABI change updates the header, `abi.revision`, affected SDKs, and tests together.
- One process uses one SDK: never load the C++ SDK's `libhypha_ffi` and link the Rust SDK in the same process. Each copy would hold its own simulated clock, log engine and signal handler.
- Each installed include namespace has one owner. `hypha-cxx` owns `hypha_msgs/`; applications generate their own messages with `hypha_generate_messages()`.
- Fail explicitly on invalid or stale data. Do not invent values or silently discard a fallible result. Rust code under `crates/` forbids unsafe: `unsafe` only in `hypha_ffi` (the C boundary), `hypha_sys` (OS calls), the test-only module `hypha_testkit/src/env.rs` and the C boundary module `hypha_testing_mcap/src/ffi.rs` of the test-only MCAP reader; the Rust SDK `hypha_node::sdk` calls its safe functions (`hypha_node::safe`) and forbids `unsafe`; shipped paths obey the workspace panic and arithmetic lints.
- For iceoryx2, use the thread-safe service, shared QoS builders and bounded service-open retry. Build a `WaitSet` once per run loop. Wildcard subscriptions resolve once at creation.

## Rust style

- Rust code is written in idiomatic Rust: iterators, combinators, `?`, `let-else` and closures where they read naturally. This overrides any global "embedded C" or lambda-restriction style rules for Rust in this repository.
- House rules, lint configuration and the SCRC mapping: `docs/rust-style.md`.

## Quality checks

- For a bug, reproduce the wrong behavior before changing it. Keep a focused regression test when it can catch the failure again. Test the relevant error path as well as success when behavior changes.
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

- During edits, run the smallest relevant test or build. Use `./scripts/format.sh` to format and `./check.sh` for style and generated-file checks.
- For a local code change, run its affected build and test. Run `./test.sh rust` for Rust workspace or FFI changes. For integration, transport, ABI, build-system, or release changes, run the full gate: `./test.sh --clean` and `./check.sh`. Report skipped checks; reserve “all green” for the full gate.
- Transport tests need writable iceoryx2 shared-memory paths. On Linux these include `/dev/shm` and `/tmp/iceoryx2`; on macOS, `/tmp/iceoryx2`. After a capacity change, clear stale segments with `./clean-shm.sh`.
- Verify behavior with a binary or bus traffic when the change affects runtime behavior. A source review alone has missed replay ordering, topic rates, and stalled UI loops here.
