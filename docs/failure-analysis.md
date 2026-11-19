# Failure analysis

The failure modes of hypha as a software element, what hypha does about each, and
what the integrator must do about the rest. First issue, not reviewed by a second
person.

Readers: an integrator who writes the safety case of a system that uses hypha, and
an assessor (see [sil2-readiness.md](sil2-readiness.md)).

## 1. Method and limits

- **Item.** `libhypha_ffi`, the C ABI and the C++ SDK, on one Linux host. The tools,
  the ROS 1 bridge, simulated time and replay are outside the item; they appear below
  only as causes.
- **Method.** A failure mode and effects analysis by function. The failure modes of
  communication are the errors of IEC 61784-3; the others come from the guide words
  no action, wrong action, too early, too late, wrong value, applied to each function
  of the item.
- **No severity.** hypha has no hazard: the severity of "the receiver sees no data"
  depends on what the receiver controls. The analysis therefore classes each failure
  mode by what hypha does, and leaves severity and risk to the integrator's hazard
  analysis.
- **Sources.** Each measure is taken from the specification documents and names its
  requirement and a test. A row marked *not analysed* is an open item of section 5.
- **Systematic faults** of hypha itself, of the compiler and of the dependencies are
  not failure modes of a function. They are treated in
  [sil2-readiness.md](sil2-readiness.md) §4.6 and §4.12.

Classes:

| Class | Meaning |
|---|---|
| C | Contained: hypha detects it and the wrong data or state never reaches the application. Counted. |
| R | Reported: hypha detects it and reports it. The application or the system must react. |
| F | Fail-stop: the process ends. Other nodes see silence. |
| U | Undetected by hypha. An assumption of use (section 4) must cover it. |

## 2. Assumed system

1. One host, one Linux kernel, one monotonic clock.
2. Every process of a domain runs under one dedicated UID; no other process can open
   the domain's shared memory (a closed system).
3. A safety function has a safe state that is reached when its inputs go silent, and
   a watchdog outside the hypha processes.
4. The safety-related processes run with the scheduling setup the integrator chose.
   hypha creates no thread and sets no priority.

## 3. Failure modes

### 3.1 Sending

| ID | Failure mode | Cause | Effect and measure | Evidence | Class | Covered by |
|---|---|---|---|---|---|---|
| FM-01 | No free sample for a publish | Readers hold samples; the publisher outruns its pool | The call returns `PoolExhausted` at once and never blocks. In a cycle the run fails, unless the output is declared `diagnostic`: then the message is dropped and counted. | FSR-06; `hypha_ffi/publisher_payload_capacity`, `hypha_ffi/cycle_transport_drops` | R | AoU-05 |
| FM-02 | Message larger than the topic's limit | Wrong sizing | The publish is refused. In a cycle the iteration fails and sends nothing. | FSR-06, NFR-12; `hypha_iceoryx2/payload_capacity` | R | AoU-05 |
| FM-03 | Memory of a topic grows | A publisher of a variable-size message without a payload limit | The segment grows in powers of two. Nothing compares it with the volume. Ends as FM-50. | NFR-05 holds for bounded topics only | U | AoU-06 |
| FM-04 | Wrong content is sent with a valid checksum | The application filled the loan wrongly, or a second thread of the publisher changed it before the send | The checksum is computed at the send over what the loan holds. | `architecture.md` §3.4 | U | AoU-10 |
| FM-05 | Part of the outputs of one step is sent | The transport fails between two sends of a cycle step | The run fails. The outputs already sent are not withdrawn. | `execution-model.md` §6.3 | R | AoU-11 |
| FM-06 | A step produces no valid output | The step throws, or an output fails validation | No output of that step is sent: every staged message is checked before the first send. The iteration fails. | FSR-38; `cxx:test_cycle_step_faults` | C | AoU-01 |

### 3.2 The channel

| ID | Failure mode | Cause | Effect and measure | Evidence | Class | Covered by |
|---|---|---|---|---|---|---|
| FM-10 | Corruption | Memory fault, a stray write into shared memory | The CRC-32C over header and payload fails; the frame is refused and counted (`corrupt_payload`). A corruption passes with a probability of about 2^-32. | FSR-09, FSR-10; `hypha_core/frame_validation` | C | AoU-09 |
| FM-11 | Truncated or unknown frame | A peer of another version; a damaged header | Refused and counted (`invalid_frame`). | FSR-10; `hypha_core/frame_validation` | C | |
| FM-12 | Wrong message definition | Publisher and subscriber built from different definitions | Refused and counted (`schema_mismatch`). | FSR-05; `cxx:test_typed_schema_identity` | C | |
| FM-13 | Repetition | Transport fault | A sequence equal to the last accepted one of that publisher is refused and counted. | FSR-10; `hypha_wire/sequence_validator` | C | |
| FM-14 | Wrong order | Transport fault | A sequence older than the last accepted one is refused and counted. | FSR-10; `hypha_wire/sequence_validator` | C | |
| FM-15 | Loss | The reader's queue overflowed; a refused send | The next frame is accepted and the skipped sequence numbers are counted (`missingSequences()`). The lost data is not recovered. | FSR-10; `hypha_ffi/subscriber_missing_sequences` | R | AoU-04, AoU-07 |
| FM-16 | Delay | Load, scheduling | A cycle input older than its `silence_ms` is `Stale` and not usable; the age rejection is counted. A plain subscriber has no age limit. | FSR-10, FSR-11; `hypha_cycle/input_judgement` | R (cycle input), U (plain subscriber) | AoU-02, AoU-03 |
| FM-17 | The sender is silent | The sender crashed, hangs or was never started | A cycle input turns `Stale` after `silence_ms`, or stays `NeverArrived`. A plain subscriber sees nothing. | FSR-11, FSR-39; `hypha_cycle/input_supervision` | R (cycle input), U (plain subscriber) | AoU-01, AoU-02 |
| FM-18 | Send time in the future | Corrupted time field | Refused and counted (`invalid_timestamp`). | FSR-10; `hypha_core/frame_validation` | C | |
| FM-19 | A relay sends, its source is dead | A driver, bridge or gateway repeats its last value | Not visible in the frame. With a life sign declared (`setAlive`, `stagnation_ms`) the input turns stagnant. | FSR-39; `cxx:test_input_supervision` | R when declared, else U | AoU-08 |
| FM-20a | Wrong address: a frame reaches a subscriber of another topic | Transport fault, damaged connection state | Refused and counted (`corrupt_payload`): the CRC starts from the code of the topic. Two topic names share a code with a probability of 2^-32. | FSR-10; `hypha_wire/frame`, `hypha_core/frame_validation` | C | |
| FM-20 | Insertion, masquerade | Another process of the domain publishes the same message type on the topic; a tool; a misconfigured topic name | Accepted. Nothing binds a frame to its sender. | `communication-integrity.md` §5 | U | AoU-12, AoU-13 |
| FM-21 | Change after the check | A write into the publisher's segment while the reader uses the sample (delivery is zero-copy) | Not detected. A cycle input copies the payload right after the check. | `architecture.md` §3.4 | U | AoU-12, AoU-10 |
| FM-22 | Loss or error before the first frame of a publisher | Start-up, restart of a publisher | The first frame of a publisher is accepted as it is. A restarted publisher is a new publisher with a new sequence. | `communication-integrity.md` §3 | U | AoU-14 |
| FM-23 | Damage to the transport's own shared state | A stray write into a connection segment, which every participant can write | *Not analysed.* The frame checks cover the data, not the queue and discovery structures of iceoryx2. | — | U | Open item O-1 |

### 3.3 Receiving and execution

| ID | Failure mode | Cause | Effect and measure | Evidence | Class | Covered by |
|---|---|---|---|---|---|---|
| FM-30 | A callback throws | Application defect | Caught at the C ABI and counted (`node.callback_failures`); the run continues. | NFR-09; `hypha_ffi/node_health`, `cxx:test_callback_failure` | R | AoU-04 |
| FM-31 | A callback never returns | Application defect, a blocking call | Every callback, timer and cycle of that `WaitSet` stops, and so does the node's `~/state`. Nothing inside the process notices: hypha creates no thread. Other nodes see FM-17. | FSR-14; `execution-model.md` §6.3 | U in the process | AoU-01 |
| FM-32 | A callback is late | Another callback of the same `WaitSet` runs long; a burst on another topic | Dispatch is bounded per turn, so one busy topic cannot starve the rest; a slow callback is counted and logged. No deadline is guaranteed. | `hypha_core/dispatch_batch_turn`; `execution-model.md` §5, §6.2 | R | AoU-01, AoU-15 |
| FM-33 | A timer or cycle period is missed | Overload, scheduling | The miss is counted (`Timer::missedCount()`, `CycleTime::missed`). The period is not made up. | FSR-15; `hypha_core/timer_missed_count` | R | AoU-01, AoU-04 |
| FM-34 | More input than a step takes | A publisher faster than the cycle | The oldest pending samples are discarded and counted; the newest are examined. | NFR-05; `hypha_cycle/cycle_history` | R | AoU-07 |
| FM-35 | Internal fault of the library | A defect in hypha or a dependency | The process aborts. No status is returned; destructors and log flushes do not run. Other nodes see FM-17. | NFR-09; `architecture.md` §3.3 | F | AoU-01 |
| FM-36 | A stale or absent value is used as current | Application reads the value without the verdict | A read of a stale or absent cycle input reports "unavailable"; the last value is reachable only through `lastKnown()`. A plain subscriber gives no verdict. | FSR-11; `hypha_cycle/input_judgement` | C (cycle input), U (plain subscriber) | AoU-02, AoU-03 |

### 3.4 Processes and resources

| ID | Failure mode | Cause | Effect and measure | Evidence | Class | Covered by |
|---|---|---|---|---|---|---|
| FM-40 | A process dies | Crash, kill, abort (FM-35), SIGBUS (FM-50) | No surviving node fails, loses data or has a publish refused. The dead node keeps its ports until a node of the same name starts. Not tested: a publisher killed with an unsent loan, a kill between two sends of one step. | FSR-27; `hypha_node/peer_kill_isolation`, `hypha_iceoryx2/stale_resource_reclaim` | F | AoU-01, AoU-16 |
| FM-41 | A process restarts for ever | A fault that returns at every start | Outside the item. hypha's own launcher gives up after repeated restarts, and it is a tool. | — | U | AoU-16 |
| FM-42 | Two nodes take one name | Deployment error | The second node is refused while the first is alive. | `hypha_node/node_identity` | C | |
| FM-43 | Nodes start in another order | Deployment, restart | No effect on what connects. | FSR-25; `hypha_iceoryx2/start_order_matrix` | C | |
| FM-50 | The shared-memory volume is full | Volume too small; FM-03 | The writer is killed by SIGBUS, with a line that names the cause. It can be any node of the domain, the safety-related one included. | `hypha_ffi/bus_error_line`; `containerized-deployment.md` §3 | F | AoU-06 |
| FM-51 | Too many publishers or subscribers on a topic | Deployment error; tools attached | The open is refused at start (`ResourceLimit`). Three subscriber slots of each topic are reserved for tools. | `hypha_iceoryx2/resource_qos` | C at start | AoU-13 |
| FM-52 | Two nodes declare different limits for one topic | Build or configuration mismatch | The second is refused (`QosMismatch`), whatever the start order. | `hypha_iceoryx2/resource_qos` | C at start | |
| FM-53 | The process runs out of heap | Leak, growth | *Not analysed.* | — | U | Open item O-2 |

### 3.5 Time

| ID | Failure mode | Cause | Effect and measure | Evidence | Class | Covered by |
|---|---|---|---|---|---|---|
| FM-60 | The wall clock steps | NTP, operator | No effect on timers, ages or timeouts: they use the host monotonic clock. | FSR-16, FSR-37; `hypha_time/clock` | C | |
| FM-61 | A node runs on simulated time in the field | The simulated-time setting left on | The node waits for `/clock`. Without a clock driver its timers and cycles never run, and it reports no fault. | FSR-17; `hypha_ffi/sim_time_environment` | U | AoU-17 |
| FM-62 | Send time and receive time are on different clocks | Processes on different hosts or kernels | Outside the assumed system. On one host the monotonic clock is one. | `hypha_time/cross_process_monotonic` | C by assumption | AoU-18 |

### 3.6 Configuration and build

| ID | Failure mode | Cause | Effect and measure | Evidence | Class | Covered by |
|---|---|---|---|---|---|---|
| FM-70 | A parameter is missing, malformed or unknown | Configuration error | Node creation fails with an explicit error. | FSR-18; `hypha_core/parameters` | C at start | |
| FM-71 | Application and library have different ABI revisions | Deployment of mixed builds | Refused before any other call. | `cxx:abi_revision_mismatch_is_refused` | C at start | |
| FM-72 | Two hypha versions in one domain | Rolling update | A frame of another format is refused (FM-11). Other effects are *not analysed*. | — | U | AoU-19, open item O-3 |
| FM-73 | A node is in the wrong domain, or uses a wrong topic name | Configuration error | An invalid domain name is an error. A valid wrong name connects to nothing: the input stays `NeverArrived`, and a subscriber without a publisher is logged. | FSR-40; `hypha_ffi/subscriber_no_publisher_warning` | R | AoU-02 |
| FM-74 | A tool acts in the domain of a safety function | `hypha play`, the bridge, a publishing tool started by mistake | It can publish on any topic (FM-20) and uses CPU and shared memory of the host. | — | U | AoU-13 |
| FM-80 | Logging delays a thread | A slow or full disk; logging is synchronous | The thread that logs waits for the write. Repeated lines are throttled. | FSR-36 | U | AoU-20 |

## 4. Assumptions of use

What the integrator must ensure. Each is a condition of any claim made for hypha;
together they are the core of a safety manual.

| ID | Assumption |
|---|---|
| AoU-01 | The system has a watchdog outside the hypha processes and a safe state that is reached without any message from hypha. It covers a process that hangs, dies, or is late. hypha gives no deadline guarantee. |
| AoU-02 | Every input of a safety function is a supervised cycle input with a `silence_ms`. The step reads the verdict before the value and treats `Stale` and `NeverArrived` as defined cases. |
| AoU-03 | A plain `hypha::Node` subscriber is not used for an input of a safety function, or the application judges age and loss itself from the sample's send time and `missingSequences()`. |
| AoU-04 | The system reads the counters and `~/state` of every safety-related node in operation and treats a growing fault counter as a finding. |
| AoU-05 | A refused publish is a defined case of the application. No output of a safety function is declared `diagnostic`. |
| AoU-06 | Every topic of the domain has a payload limit, and the shared-memory volume holds the sum of all segments at their limits. |
| AoU-07 | Queue depth and step budget are sized for the worst-case rate. A counted loss on an input of a safety function is a fault of the system. |
| AoU-08 | Data that reaches hypha through a relay carries a life sign in its payload, declared with `setAlive()` and `stagnation_ms`. |
| AoU-09 | The residual error probability of the 32-bit checksum for the frame sizes in use fits the budget of the safety function, or the payload carries its own check (AoU-10). |
| AoU-10 | A safety function that needs protection from the data source to the data sink puts its own checksum, sequence and source identity in the payload and checks them on receipt. |
| AoU-11 | Values that must be consistent are sent in one message. |
| AoU-12 | Only hypha processes of one system run under the domain's UID, and nothing else can open its shared memory. |
| AoU-13 | No tool, bridge or replay runs in the domain of a safety function in operation, or a system-level argument shows it cannot affect the function. |
| AoU-14 | A restart of a producer is a defined event for its consumers. `Usable` means a fresh sample arrived, not that the producer has converged. |
| AoU-15 | The work of a safety function runs on its own `WaitSet` and thread, or in its own process, with no callback of lower integrity beside it. |
| AoU-16 | A supervisor outside the item restarts or stops failed processes and limits restarts. |
| AoU-17 | Simulated time is not enabled in operation. |
| AoU-18 | All processes of a domain run on one host and one kernel. |
| AoU-19 | One domain runs one hypha version. A version change stops every process and resets the shared memory. |
| AoU-20 | A thread of a safety function does not log to a sink that can block, or its log level keeps the hot path silent. |

## 5. Open items

| ID | Item | From |
|---|---|---|
| O-1 | The effect of damage to iceoryx2's connection and discovery segments on a surviving node: can it block, crash or deliver wrong data past the frame checks? Needs a fault-injection test. | FM-23 |
| O-2 | Behaviour when allocation fails, in the library and in the C++ SDK. | FM-53 |
| O-3 | Behaviour of two hypha versions in one domain beyond the frame format: service creation, discovery, limits. | FM-72 |
| O-4 | The two untested kill cases: a publisher with an unsent loan, a kill between two sends of one step. | FM-40 |
| O-5 | The repository has no test that corrupts shared memory from another process. The frame checks of FM-10 to FM-14 and FM-18 were exercised that way by hand only (`traceability.md`, FSR-10). | 3.2 |
| O-6 | Failure modes of the C++ SDK layer on its own (decoding, the transform buffer, the parameter views) are not broken down here. | 1 |
| O-8 | Closed 2026-11-20. Destroying a started cycle node from a callback of the running `WaitSet` leaked about 84 kB: the handle of an input source was refused during a run although its cycle still owned the source, and the C++ SDK then dropped it. The handle of a bound source is now released at any time (`hypha_ffi/waitset_cross_thread_refusals`). | Sanitizer run |
| O-9 | Closed 2026-11-20 as false reports. ThreadSanitizer reported two data races on memory allocated inside the library; both blocks are guarded by a mutex in safe Rust, which the sanitizer cannot see because the library is not instrumented. `scripts/tsan-suppressions.txt` ignores calls made from inside the library. Open: no ThreadSanitizer run with an instrumented Rust build exists. | Sanitizer run |
| O-7 | This analysis has one author. It needs a review by a second person and by an integrator with a real hazard list. | 1 |
