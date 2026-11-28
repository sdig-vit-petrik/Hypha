# hypha documentation

hypha is SIL 0 shared-memory middleware for one host, built on `iceoryx2`, with ROS 1 message
serialization. A Rust core is exposed through one C ABI (`libhypha_ffi`), used by the C++23 SDK
(`hypha-cxx`) and the CLI tools. Network traffic (ROS 1 XML-RPC and TCPROS, Foxglove) runs in
separate bridge processes ([apps/hypha_ros_bridge/README.md](../apps/hypha_ros_bridge/README.md)).

IEC 61508-3:2010 and IEC 61784-3 are background references; hypha claims compliance with neither.

## 1. Developer guides

| Document | Topic |
|---|---|
| [getting-started.md](getting-started.md) | Build, install, CMake integration, shared-memory cleanup |
| [cpp-api.md](cpp-api.md) | The C++23 SDK by task: `CycleNode`, ports, loans, timers, parameters, TF, services |
| [execution-model.md](execution-model.md) | `WaitSet`, cycles, callbacks, timers, simulated time |
| [ros1-migration-guide.md](ros1-migration-guide.md) | Porting a ROS 1 C++ node |
| [tools.md](tools.md) | The CLI tools (`hypha topic`, `top`, `record`, `play`, `launch`, `intercept`, `clean`, `load`, ...) |
| [replay-flow-control.md](replay-flow-control.md) | Lockstep and open-loop replay on `/clock` |
| [containerized-deployment.md](containerized-deployment.md) | Docker and Compose: `/dev/shm`, `/tmp/iceoryx2`, UIDs, domains, restarts |
| [features.md](features.md) | The public capabilities, the surface rules, triage |
| [engineering-traps.md](engineering-traps.md) | Known transport, execution and integration pitfalls |
| [rust-style.md](rust-style.md) | Rust house rules and lint policy |
| [design/native-services.md](design/native-services.md) | The design of native `.srv` services (#132) |
| [autoware-ground-filter-abs.md](autoware-ground-filter-abs.md) | An upstream bug in `autoware_ground_filter` (`int abs(int)`) |
| [Doxyfile](Doxyfile) | Optional C++ header docs (`doxygen docs/Doxyfile`, into `target/cpp-doc/`) |
| [../AGENTS.md](../AGENTS.md) | The architecture map and rules for coding agents |

Rust crate docs: `cargo doc --workspace --no-deps --open`.

## 2. Specifications and evidence

| Document | Topic |
|---|---|
| [requirements-specification.md](requirements-specification.md) | The requirements (FSR, NFR) and their class |
| [traceability.md](traceability.md) | Each requirement and the tests that verify it (generated from `traceability.toml`) |
| [architecture.md](architecture.md) | Layers and the C ABI boundary |
| [interface-specification.md](interface-specification.md) | The C ABI contract: status codes, ownership, QoS, the wire frame |
| [communication-integrity.md](communication-integrity.md) | The frame checks, the faults they cover, and the application's part |
| [failure-analysis.md](failure-analysis.md) | Failure modes, assumptions of use, and how to use hypha in a safety function |
| [dependency-assessment.md](dependency-assessment.md) | What hypha requires of its dependencies, and what shows it |
| [release-policy.md](release-policy.md) | Versions, what a release line freezes, upgrade and rollback, branches |
| [releases/](releases/) | Release notes |

## 3. Operating constraints

Details: [architecture.md](architecture.md), [execution-model.md](execution-model.md),
[interface-specification.md](interface-specification.md),
[containerized-deployment.md](containerized-deployment.md).

| Area | Constraint |
|---|---|
| Host and IPC | One host (shared memory, little-endian). Processes that talk share a UID and GID with read-write access to `/dev/shm` and `/tmp/iceoryx2`. |
| Domains and names | `HYPHA_DOMAIN` separates systems (`[a-z][a-z0-9_]`, up to 32 characters). Node names (`[a-z][a-z0-9_]`, up to 64) are unique per domain; CLI tools share `_hyphacli_<tool>`. |
| Dead nodes | A crashed node keeps its slots until a node of the same name starts and reclaims them, or `hypha clean` wipes the stopped domain. Clear shared memory after a capacity change. |
| ABI and wire | `abi.revision` versions `libhypha_ffi`; the C++ SDK refuses another revision at start. Wire frames carry a format version and a CRC-32C. |
| Threads | The library creates no thread and sets no scheduling or affinity. Nothing runs unless the application runs a `WaitSet`; attachments freeze during a run. |
| Signals and logging | The first node installs the process-wide SIGINT and SIGTERM stop handler and fixes the one `[log]` configuration (each node keeps its own `slow_callback_ms`). Logging is synchronous. |
| Freshness | Missing, stale or invalid inputs reach the step as verdicts, never as invented values. Every enabled cycle input states `silence_ms`, or is tagged `unsupervised`. |
| Queues and loans | A queued input drops its oldest samples beyond its per-step share. A subscriber keeps one received sample past its callback; one slot stays free for the next delivery. Wildcard subscriptions resolve once, at creation. |
| Clocks and counters | Transport ages use host monotonic time; timers and cycles follow `/clock` on simulated time. Every counter is a 64-bit unsigned integer that wraps. |
| Build and tools | The root CMake needs an explicit `CMAKE_BUILD_TYPE`. `hypha launch` needs tmux 3.0 or newer; `./clean-shm.sh` needs `lsof`, unless `--force` removes without a holder check. |
