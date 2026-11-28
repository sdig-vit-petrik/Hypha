# Hypha Documentation

Hypha is a SIL0 single-host shared-memory middleware framework built on `iceoryx2` with ROS 1-compatible message serialization. A Rust core exposes one C ABI (`libhypha_ffi`), consumed by the C++23 SDK (`hypha-cxx`) and CLI tools. Network connectivity (ROS 1 XML-RPC/TCPROS, Foxglove) runs in separate bridge processes ([apps/hypha_ros_bridge/README.md](../apps/hypha_ros_bridge/README.md)).

Background standards (reference only; no compliance claim): IEC 61508-3:2010, IEC 61784-3.

---

## 1. Developer Guides

| Document | Topic |
|---|---|
| [getting-started.md](getting-started.md) | Build, install, CMake integration, shared-memory cleanup |
| [cpp-api.md](cpp-api.md) | C++23 SDK task guide (`CycleNode`, ports, loans, timers, config, TF) |
| [execution-model.md](execution-model.md) | `WaitSet`, cycles, callbacks, timers, simulated time |
| [ros1-migration-guide.md](ros1-migration-guide.md) | Migrating ROS 1 C++ nodes to Hypha |
| [tools.md](tools.md) | CLI tools (`hypha topic`, `top`, `record`, `play`, `launch`, `intercept`, `clean`, `load`) |
| [replay-flow-control.md](replay-flow-control.md) | Lockstep and open-loop replay on `/clock` |
| [containerized-deployment.md](containerized-deployment.md) | Docker / Compose setup (`/dev/shm`, `/tmp/iceoryx2`, UIDs, domains, restarts) |
| [features.md](features.md) | Public capability inventory, surface rules, and triage |
| [engineering-traps.md](engineering-traps.md) | Known transport, execution, and integration pitfalls |
| [rust-style.md](rust-style.md) | Rust house rules, lint policy, and SCRC mapping |
| [design/native-services.md](design/native-services.md) | Design for native `.srv` services (#132; Stage 1a transport implemented) |
| [autoware-ground-filter-abs.md](autoware-ground-filter-abs.md) | Upstream bug note for `autoware_ground_filter` (`int abs(int)`) |
| [Doxyfile](Doxyfile) | Optional C++ header API docs (`doxygen docs/Doxyfile` -> `target/cpp-doc/`) |
| [../AGENTS.md](../AGENTS.md) | Architecture map and rules for coding agents |

Rust crate API docs: `cargo doc --workspace --no-deps --open`.

---

## 2. Specifications & Safety Artifacts

| Document | Topic |
|---|---|
| [requirements-specification.md](requirements-specification.md) | Software Requirements Specification (FSR / NFR) |
| [traceability.md](traceability.md) | Requirement-to-test matrix (generated from `traceability.toml`) |
| [architecture.md](architecture.md) | Software Architecture Specification (layering, C ABI boundary) |
| [interface-specification.md](interface-specification.md) | C ABI contract, status codes, ownership, QoS, wire frame format |
| [communication-integrity.md](communication-integrity.md) | Frame integrity checks, covered faults, and application responsibilities |
| [failure-analysis.md](failure-analysis.md) | Failure modes and integrator assumptions of use |
| [safety-manual.md](safety-manual.md) | Integrator conditions of use (draft) |
| [safety-plan.md](safety-plan.md) | Roles, verification lifecycle, defect handling, tool classes |
| [sil2-readiness.md](sil2-readiness.md) | Gap assessment against IEC 61508-3 SC 2 (no compliance claim) |
| [dependency-assessment.md](dependency-assessment.md) | Third-party dependency review |
| [release-policy.md](release-policy.md) | Versioning, release-line freezes, qualification, upgrade/rollback |
| [releases/](releases/) | Per-release notes |

---

## 3. Operating Constraints at a Glance

Details live in [architecture.md](architecture.md), [execution-model.md](execution-model.md), [interface-specification.md](interface-specification.md), and [containerized-deployment.md](containerized-deployment.md).

| Area | Constraint |
|---|---|
| **Host & IPC** | Single host only (shared memory, little-endian). Communicating processes must share UID/GID with `rw` access to `/dev/shm` and `/tmp/iceoryx2`. |
| **Domains & names** | Isolate systems with `HYPHA_DOMAIN` (`[a-z][a-z0-9_]`, ≤32 chars). Node names (`[a-z][a-z0-9_]`, ≤64 chars) are unique per domain; CLI tools share `_hyphacli_<tool>`. |
| **Dead nodes & SHM** | Crashed nodes keep their slots until a node with the same name restarts and reclaims them, or `hypha clean` wipes the stopped domain. Clear SHM after capacity changes. |
| **ABI & wire lock** | `abi.revision` versions `libhypha_ffi`; C++ SDK refuses mismatched revisions at start. Wire frames are magic-versioned with CRC-32C. |
| **Threading** | The library creates no execution threads and sets no scheduling/affinity. Progress happens only while the application drives a `WaitSet`; attachments freeze during a run. |
| **Signals & logging** | First node creation installs the process-wide `SIGINT`/`SIGTERM` stop handler (below C ABI) and locks in the single `[log]` config (each node keeps its own `slow_callback_ms`). Logging is synchronous. |
| **Data & freshness** | Fail-restrictive: stale/missing/invalid inputs return `NOT_AVAILABLE` (`std::nullopt`), never invented defaults. Every supervised cycle input requires `silence_ms` (or `unsupervised`). |
| **Queues & loans** | Queued inputs drop oldest beyond their per-step share. Owning subscribers hold at most `min(buffer, 2)` simultaneous loans (`BorrowLimit`). Wildcard subscriptions resolve once at creation. |
| **Clocks & counters** | Transport age and timeouts use host monotonic time; timers follow `/clock` under sim time. All counters are 64-bit wrapping unsigned integers. |
| **Build & tools** | Root CMake requires an explicit `CMAKE_BUILD_TYPE`. `hypha launch` requires `tmux ≥ 3.0`; `./clean-shm.sh` requires `lsof` (unless `--force`). |
