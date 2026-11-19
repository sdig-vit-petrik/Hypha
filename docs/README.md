# hypha Documentation

`hypha` is a SIL0 robotics and sensor middleware framework.
It provides zero-copy shared-memory IPC (`iceoryx2`) and ROS 1-compatible message payload serialization.
Its memory-safe Rust core exposes a unified C ABI (`libhypha_ffi.so`).
The apps' [ROS 1 bridge](../apps/hypha_ros_bridge/README.md)
provides ROS graph integration through XML-RPC and topic transport through TCPROS.

Reference standards (background only; no compliance claim):
- **IEC 61508:2010 Part 3**: Software Requirements for Functional Safety.
- **IEC 61784-3**: Functional Safety Fieldbuses (communication errors, black channel).

---

## 1. Specifications

| Document | Content |
|---|---|
| [requirements-specification.md](requirements-specification.md) | **Software Requirements Specification (SRS)**: functional and non-functional requirements. |
| [traceability.md](traceability.md) | Each requirement and the test or analysis that verifies it; the gaps. Generated from `traceability.toml`; `scripts/qualify.sh` writes the result of every requirement for a tree. |
| [failure-analysis.md](failure-analysis.md) | Failure modes of hypha as a software element, what hypha does about each, and the assumptions of use for an integrator. First issue. |
| [safety-manual.md](safety-manual.md) | What an integrator must know and do to use hypha in a system with a safety function: the item, the faults it detects, the conditions of use. Draft. |
| [safety-plan.md](safety-plan.md) | Roles, lifecycle, verification, modification, defect handling and tool classes. |
| [release-policy.md](release-policy.md) | What a release line freezes, the supported platforms, what a release must pass, upgrade and rollback. |
| [dependency-assessment.md](dependency-assessment.md) | The third-party software in the library: what hypha requires of each part and what shows it. |
| [sil2-readiness.md](sil2-readiness.md) | Self-assessment against IEC 61508-3 for systematic capability SC 2: what exists, the gaps, the work in order. Claims no compliance. |
| [architecture.md](architecture.md) | **Software Architecture Specification (SAS)**: layering, the C ABI boundary, freedom from interference. |
| [interface-specification.md](interface-specification.md) | **Software Interface Specification (SIS)**: the C ABI, return codes, memory ownership, threading rules, resource QoS and the wire frame. |
| [communication-integrity.md](communication-integrity.md) | Which frame check covers which fault, what the application must still do, and the limits. Not a safety manual: no compliance claim. |

---

## 2. Developer Guides

| Guide | Content |
|---|---|
| [getting-started.md](getting-started.md) | Build and install the framework; find the apps and examples. |
| [execution-model.md](execution-model.md) | How your callbacks run: the caller-owned `WaitSet`, `run`, `runFor`, notifications, timers and polling. Written for engineers who come from ROS 1 (noetic). |
| [ros1-migration-guide.md](ros1-migration-guide.md) | Agent procedure: migrate one ROS 1 node to a production-ready hypha cycle node configured by TOML, with a small diff, and report hypha gaps. |
| [cpp-api.md](cpp-api.md) | C++23 SDK task guide: per task, what to write and which example does it. The class and method reference is the public header comments. |
| [containerized-deployment.md](containerized-deployment.md) | Running a system in Docker containers over several Compose projects: the two shared tmpfs volumes, UID and domain rules, `/dev/shm` sizing, restarts, updates and reboots, troubleshooting. |
| [features.md](features.md) | Every public capability once, with the question it answers and its users; the rules that keep hypha lean, the triage, and the owner's decisions. `./check.sh` fails on a C ABI function or public header that has no row. |
| [Hypha Apps tools](tools.md) | Command-line tools maintained with their application source. |
| [replay-flow-control.md](replay-flow-control.md) | Replay on simulated time: how `hypha play --clock` releases each chunk once the nodes have finished it, the recorder as a participant (`hypha record --sync`), the upstream hold of chained nodes, and its limits. |
| [Doxyfile](Doxyfile) | Optional HTML rendering of the public header comments (`doxygen docs/Doxyfile`, into `target/cpp-doc/`); no build or check runs it. |
| [../AGENTS.md](../AGENTS.md) | Shared working instructions for Codex, Claude Code, and GitHub Copilot. |
| [engineering-traps.md](engineering-traps.md) | Observed transport, execution, and integration failure modes; consult relevant entries when needed. |
| [rust-style.md](rust-style.md) | How the Rust code is written: sources in order of precedence, house rules beyond the lint configuration, and the lint-to-SCRC mapping. |

Rust crate documentation is generated from the source code:

```sh
cargo doc --workspace --no-deps --open   # HTML in target/doc
```

---

## 3. Key Operating Constraints

Integrators must satisfy these constraints on the target host:

- **One Logging Configuration For Each Process**: The first node whose parameter file has a `[log]` table sets the logging configuration. A later node with a different table fails with `InvalidState`. A relative sink path resolves against the directory of the file that wrote it. Another process that holds the lock on a sink file also causes `InvalidState`. Logging is synchronous and creates no thread, so a slow disk delays the thread that logs.
- **Single Host**: Transport runs over shared memory on one host. No network transport exists.
- **tmux for `hypha launch`**: The launcher in `apps/` runs a fleet only in a tmux session. The target needs tmux 3.0 or newer. If the tmux server stops, every node that it started stops.
- **One Domain Per System**: Two systems on one host stay apart only when each runs in its own domain. Set `HYPHA_DOMAIN` (`[a-z][a-z0-9_]`, at most 32 characters) in every process of a system, tools included. Processes in two domains see none of each other's topics, services or nodes. An invalid value, including one that is not UTF-8 text, stops the first transport call; it never falls back to no domain.
- **One Name Per Node**: A node name is `[a-z][a-z0-9_]`, at most 64 characters. Node creation refuses a name that an alive node of another process in the same domain holds. The check is not atomic: two processes that start with one name at the same instant can both pass it, so give distinct names (the `node.name` parameter) to instances that can start together. A command-line tool names its node `_hyphacli_<tool>` instead: any number of processes hold that name at once, `hypha top` does not show it, and the node has no `~/` names and runs no cycle.
- **Dead Nodes Stay Until Their Name Returns**: A node that a crash or a kill ended stays in the registry as dead and keeps its ports until a node of the same name starts and reclaims it, or until `./clean-shm.sh` wipes the stopped domain. Any node start reclaims every dead CLI tool node (`_hyphacli_<tool>`); a node whose own dead run cannot be reclaimed does not start. A topic whose subscriber or publisher slots dead nodes hold stays full until then.
- **One Node State**: hypha judges the required inputs of a node below the C ABI (`hypha_node_state`), and `runNode` publishes the result on `~/state` every second of framework time, with the cycle counters, the per-input counters and the log health. The rule reads only the inputs: a missed period, an overrun or a drop does not change the state.
- **Node Creation Owns SIGINT and SIGTERM**: the first node creation in a process installs one handler for both signals, below the C ABI, so every SDK and tool shares it. It requests a process-wide stop, which turns `ok()` false for every node of the process, so the application leaves its loop and runs its own cleanup. A handler installed before the node still runs after it. Without such a handler, a second signal ends the process at once. A signal the process ignores stays ignored. A handler installed after the node replaces hypha's, and `ok()` then does not see the signal unless that handler chains to hypha's. The stop is for good: a node created after it is stopped from the start.
- **Identity Matching**: All communicating processes must execute under the same numeric UID and GID with `rw` access to `/dev/shm` and `/tmp/iceoryx2`.
- **Publisher Lifetime**: A publisher must remain active while subscribers process sent samples. Abrupt process termination can invalidate shared-memory segments before consumption.
- **Clean Shared Memory After Capacity Changes**: Transport capacities are fixed by the first process creating a service. Clear stale segments via `./clean-shm.sh` after changing capacities.
- **`clean-shm.sh` Needs `lsof`**: It finds the processes that still hold iceoryx2 resources with `lsof`. Without `lsof` it stops, unless `--force` removes the resources without that check.
- **Name A Build Type**: The root CMake project refuses an empty `CMAKE_BUILD_TYPE`. Configure with `-DCMAKE_BUILD_TYPE=Release` or `Debug`.
- **Loan Capacity Budget**: Owning receive subscribers hold an effective loan budget of `min(subscriber buffer size, 2)` samples (`MAX_BORROWED_SAMPLES`). Unreleased loans block subsequent sample receipts with `HyphaStatus::BorrowLimit`.
- **Fail Restrictive**: Stale, missing, or invalid data returns `NOT_AVAILABLE` (`std::nullopt`). The framework never invents default values.
- **The Framework Creates No Execution Thread**: Callbacks run on threads the application creates and owns. A node makes progress only while it runs a `WaitSet`. This includes housekeeping: latched topics are not replayed until a `WaitSet` runs.
- **A Pattern Subscription Resolves Once**: A wildcard subscription follows the
  topics that existed when it was created. Matching a pattern enumerates every
  service on the machine, which costs 175 ms at 120 topics, so it is not
  repeated on the receive path. Start a pattern subscription after the system
  it observes is up, or list topics and subscribe by name to follow topics that
  appear later. `hypha record`, `hypha foxglove` and the ROS bridge do the
  latter and pick up new topics within half a second.
- **A Cycle Has No Deadline Guarantee**: A cycle runs its due timers before it dispatches ingress, in registration order. A sample can wait for a later step, and a slow step delays the cycles after it. Measure the delay on the target. A cycle committed before the first simulated time arrives reports a large missed-period count at its first tick (execution model, section 6.2).
- **Attachments Freeze During a Run**: Create every subscription and timer before the run starts. Attaching during a run is refused with `InvalidState`; a callback may detach a registration of its own `WaitSet`, which stops at once and is released when the run returns.
- **Thread Scheduling Is the Application's**: The library sets no scheduling policy, priority, or CPU affinity, because it creates no thread.
- **Notification Compatibility Is Refused, Not Downgraded**: An event-driven subscription needs a notification-enabled publisher and subscriber. The framework refuses an incompatible pairing rather than silently adding periodic polling.
- **Cycle Input Policy**: Every cycle input that counts for health sets `silence_ms` (past it after the sample was sent, the input is stale), or its port is tagged `hypha::unsupervised` in code; a node that has neither does not start (a `CycleNode` under `runNode` exits 78; a composed `hypha::Cycle` started by the application fails its `start()`). Add `setAlive()` and `stagnation_ms` to detect a stuck sender. Ordinary subscribers do not reject old data.
- **Transport Age Is Not Source Age**: Age is measured from the frame's host monotonic send time. A value that needs sensor or event time carries it in its payload. A replay publishes with a new send time.
- **Transport Timeouts Count Host Time**: On simulated time, timers follow `/clock`. Subscriber waits, `runFor` limits and transport age count host monotonic time, so a paused replay never suspends these timeouts. A dead clock driver is detected by whatever supervises it, not by its participants.
- **Transport Age Is Host-Local**: The send time is the host monotonic clock, so both ends must share one host. A simulated clock does not age a value.
- **Frames Are Version Locked**: Every frame carries a CRC-32C and a version-specific magic value. A receiver refuses a frame with another magic, so both ends of a topic must use the same wire version.
- **Counters Wrap**: Every counter the framework reports is 64 bits wide and wraps to zero after its maximum value, so a node runs for years without a frozen count. Compute the difference of two readings modulo 2^64: unsigned subtraction in C++ and `wrapping_sub` in Rust. A plain signed subtraction goes negative across a wrap.
- **Little-Endian Targets Only**: The ROS 1 codec encodes primitives in target byte order. It refuses to build for a big-endian target rather than produce reversed values.
- **Exact ABI Revision**: The root file `abi.revision` sets the one ABI revision of the library and the SDKs. The C++ SDK refuses a library with another revision at start: it returns `InvalidState` and prints a diagnostic on standard error. The Rust crate is linked with its runtime and cannot load another revision. The library file is versioned (`libhypha_ffi.so.<revision>`, `libhypha_ffi.<revision>.dylib`). Rebuild every binary that was built against an earlier library. An older binary has no check, and its loader name does not match the new file.
- **Readiness Is Not Delivery**: `Publisher::subscriberCount()` and `Output::subscriberCount()` show that a subscriber is connected. They do not show that the subscriber received a sample.
- **Queued Inputs Drop The Oldest**: A queued input discards the oldest pending samples beyond its share and examines the newest, counting each discard; no option keeps a backlog. Read the loss counters.
