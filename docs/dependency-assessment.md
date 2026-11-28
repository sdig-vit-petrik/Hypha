# Dependency assessment

The software in `libhypha_ffi` that hypha did not write: what hypha needs from each
part, what shows that it delivers, and what is not shown. It has the form IEC 61508-3
asks for pre-existing software (7.4.2.12, 7.4.2.13). First issue, not reviewed.

Readers: an assessor, and the engineer who updates a dependency.

## 1. Status

- None of the dependencies has been assessed against a safety standard by this
  project, and hypha claims nothing for them.
- The list is the output of `cargo tree --locked -p hypha_ffi -e normal` at C ABI
  revision 1. `Cargo.lock` pins every version; `cargo deny` checks advisories and
  licences on every change.
- A dependency update follows [safety-plan.md](safety-plan.md) §6 and updates this
  document in the same change.

## 2. The parts

| Part | Crates | Runs in a node | Role |
|---|---|---|---|
| iceoryx2 0.10.1 | 28 (`iceoryx2`, `iceoryx2-cal` vendored with a patch, `iceoryx2-bb-*`, `iceoryx2-pal-*`, `iceoryx2-log*`) | Yes, always | The transport: shared-memory segments, lock-free queues, discovery, events, cleanup of dead processes |
| Pulled in by iceoryx2 | `flatbuffers`, `postcard`, `cobs`, `sha1_smol`, `tiny-fn`, `enum-iterator` | Yes | Its service configuration files and naming |
| Operating-system calls | `libc`, `rustix`, `linux-raw-sys`, `bitflags` | Yes | System calls and memory mapping |
| Checksum | `crc-fast` 1.10.0; `spin`, and `digest`, `crypto-common`, `generic-array`, `typenum`, which its `std` feature pulls in and hypha does not call | Yes, on every send and receive | The CRC-32C of the frame |
| Configuration | `toml` 0.9.12 with `toml_parser`, `toml_datetime`, `toml_writer`, `serde_spanned`, `winnow`; `serde`, `serde_core` | At start-up | Parsing the node's parameter file |
| Transforms | `transforms` 2.3.0, `approx`, `num-traits`, `libm` | When a node uses the transform buffer | Interpolation of coordinate transforms |
| Containers and errors | `hashbrown`, `foldhash`, `equivalent`, `allocator-api2`, `thiserror`, `log` | Yes | Maps, error types, the log facade |
| Build time only | `proc-macro2`, `quote`, `syn`, `unicode-ident`, `serde_derive`, `thiserror-impl`, `enum-iterator-derive`, `iceoryx2-bb-derive-macros` | No | Procedural macros; they generate code at compile time |
| Rust runtime | `core`, `alloc`, `std` of the Rust version pinned in `rust-toolchain.toml` | Yes | The language runtime |
| C++ runtime | The standard library of the builder image's GCC | In the application | The SDK headers use it |
| Platform | Linux, the MMU, POSIX shared memory, the monotonic clock | — | Outside the item; an assumption of use |

## 3. What hypha requires of the transport

iceoryx2 is the part whose failure matters most: its code and its shared state are in
every node. The frame checks of hypha make it a black channel for the *data*
([communication-integrity.md](communication-integrity.md)): corruption, loss,
repetition, reordering and delay of a message are detected by hypha whatever the
transport does. The requirements below are the properties the black-channel argument
does not cover.

| ID | hypha requires that the transport | Shown by | Not shown |
|---|---|---|---|
| TR-01 | never blocks a publisher on a slow or dead reader | `hypha_iceoryx2/subscriber_depth`, `hypha_node/peer_kill_isolation` | No analysis of its lock-free algorithms; no test under scheduling stress |
| TR-02 | refuses a loan at once when the pool is empty | `hypha_iceoryx2/payload_capacity`, `hypha_ffi/publisher_payload_capacity` | |
| TR-03 | uses no more shared memory than the declared limits of a bounded topic | `hypha_iceoryx2/shm_capacity`, `hypha_iceoryx2/resource_qos` | Memory over time under sustained overload is not measured |
| TR-04 | lets survivors continue, without loss or refusal, when a participant is killed at any point | `hypha_iceoryx2/stale_resource_reclaim`, `hypha_node/peer_kill_isolation` | |
| TR-05 | reclaims the resources of a dead participant when a node of the same name starts | `hypha_iceoryx2/stale_resource_reclaim` | |
| TR-06 | establishes the same connections whatever the start order | `hypha_iceoryx2/start_order_matrix` | |
| TR-07 | keeps two domains apart | `hypha_iceoryx2/domain_isolation` | |
| TR-08 | creates no thread | `hypha_iceoryx2/housekeeping_creates_no_thread` | |
| TR-09 | wakes a waiting reader on every send | `hypha_iceoryx2/notification`, `hypha_iceoryx2/waitset_wake_cadence` | A failed notification is counted (`transport.failed_notifications`); nothing polls |
| TR-10 | maps a publisher's data segment read-only in its readers | Inspection of iceoryx2 (`AccessMode::Read`), [architecture.md](architecture.md) §3.4 | No test |
| TR-11 | does not corrupt the memory of the process that hosts it, whatever its shared state holds | — | **Nothing.** A stray write into a connection segment, which every participant can write, has not been injected ([failure-analysis.md](failure-analysis.md) O-1) |
| TR-12 | returns from every call in bounded time | — | **Nothing.** No worst-case analysis |

TR-11 and TR-12 decide how far the black-channel argument reaches. Without them the
honest statement is: a fault inside the transport can stop or crash a node, and the
system must then reach its safe state by the watchdog (AoU-01).

The vendored patch (`vendor/README.md`) lowers one limit of `iceoryx2-cal` and
changes no algorithm.

## 4. The other parts

| Part | hypha requires | Shown by | Not shown |
|---|---|---|---|
| `crc-fast` | The CRC-32C (Castagnoli) of a byte sequence, the same on every platform | `hypha_wire/crc32c_check_value` (the published and RFC 3720 check values, and equality with `crc32c` 0.6.8, the earlier implementation, over every length to 1 KiB at every alignment in a word and over arbitrary frames), `hypha_wire/frame` | Only the path the test machine selects is tested: aarch64 with PMULL and SHA3 (macOS, Linux); the x86_64 paths and the table fallback are not run here |
| `toml`, `serde` | A parameter file parses to its values or to an error; no input aborts the process | `hypha_core/parameters`, the fuzz target `config_load` | |
| Operating-system crates | Each call hypha makes behaves as the Linux manual states | The tests that exercise the calls | No list of the calls in use |
| `transforms` | Interpolation between two stamped transforms | `hypha_ffi/transform_buffer`, `cxx:test_transform_buffer` | Numerical limits |
| Procedural macros | Correct generated code | The tests of the code they generate | They are tools of class T3 ([safety-plan.md](safety-plan.md) §8) |
| Rust `std` | — | — | Not assessed; see the compiler in [safety-plan.md](safety-plan.md) §8 |

## 5. Functions that are present and not needed

Not analysed. iceoryx2 is built without its default features, with `std` and `log`
only. hypha uses its publish-subscribe and event services; whether another service
type or a configuration path is linked and unreachable has not been checked.

## 6. Ways to close the gaps

In order of cost:

1. A fault-injection test for TR-11.
2. A list of the system calls in use, from the operating-system crates.
3. For TR-11 and TR-12: an assessment of iceoryx2 as pre-existing software, or a
   certified variant from its vendor if one exists, or the system-level argument
   above (watchdog and safe state) accepted by the assessor.
