# Software Requirements Specification (SIL0 engineering requirements)

These requirements define tested framework behavior. They are not safety-integrity requirements and do not claim SIL certification.

## 1. Scope and Engineering Context

This specification defines the functional and non-functional requirements for `hypha`.

`hypha` has these goals:
- Carry ROS 1 messages over iceoryx2 shared memory.
- Give node developers a simple interface in C++ and Rust.
- Supply command-line tools and introspection of a running system.
- Support simulated time.
- Support both event-driven and polling execution.
- Provide caller-owned periodic processing cycles with managed inputs and outputs.
- Report input silence and stagnation for supervised cycle inputs.
- Isolate independent systems that share one host.
- Bridge topics between `hypha` and a live ROS 1 graph.
- Expose live topics to Foxglove clients.
- Launch and supervise a configured set of node processes.
- Provide configurable, synchronous logging with rotating file sinks.

A feature that does not serve one of these goals is not in scope.

`hypha` targets safety, not security. It detects faults and contains a faulty
or crashed process; it does not defend against a deliberate attacker. Every
process that can reach a system's shared memory is trusted, and the deployment's
access control keeps all other processes out (a closed system: the
transport is a black channel that only trusted processes reach). A protection against a hostile process inside
that boundary is not in scope when it costs complexity or data-path performance.

The following standards are background references only. They do not define project compliance:
- **IEC 61508-3:2010 § 7.2** (Software Safety Requirements Specification, background reference)
- **IEC 61784-3** (Functional Safety Fieldbuses: communication errors and the black channel, background reference)

The integrity column records engineering priority. It does not assign a Safety Integrity Level.

Requirements in this specification are stated as observable behavior and measurable
outcomes, not as a description of a particular implementation. No requirement below
names a programming language, a library, an internal data structure, a function
signature, or an error-code identifier, so that conformance is assessed against the
delivered system's externally observable behavior rather than against a chosen
implementation.

### Verification Methods (V-Model)
- **T (Test)**: Automated test execution in CI gates.
- **A (Analysis)**: Formal engineering calculation or static analysis measurement.
- **I (Inspection)**: Manual code review, schema inspection, or artifact audit.
- **D (Demonstration)**: Operational verification on a reference target.

---

## 2. Functional Requirements (FSR)

### 2.1 Communication

| ID | Functional Requirement | Priority | Verify |
|---|---|---|---|
| **FSR-01** | The software **shall** provide inter-process publish-subscribe communication across processes on a single host. | Reliability | T |
| **FSR-02** | The time complexity of publishing a message **shall** be $O(s)$ in the number $s$ of active subscribers, with no copy of the payload per subscriber: the transport hands each subscriber connection one offset into the one shared sample (corrected 2026-11-15; it read $O(1)$). Work to populate the message representation and calculate its required checksum may be $O(n)$ in payload size, where $n$ is the payload length. | Reliability | T, A |
| **FSR-03** | Local message transmission between a publisher and a subscriber on the same host **shall not** incur an intermediate buffer-copying penalty beyond what is required to populate the message representation itself, and **shall** add framing overhead per message that is a small, fixed number of bytes. | Reliability | T, A |
| **FSR-04** | The software **shall** carry message payloads in the ROS 1 serialization format, byte for byte. Each transmitted message **shall** carry a compact identifier of its data definition, sufficient for a receiver to detect a mismatched message definition without requiring the full definition to accompany every message. | Reliability | T, I |
| **FSR-05** | The software **shall** verify data-definition compatibility at runtime; a message whose data definition differs from what a subscriber expects **shall** be rejected and recorded in an error count. | Reliability | T |
| **FSR-06** | When a publisher has no message capacity remaining to accept a new message, the publish call **shall** return an explicit, distinct failure indication immediately, without blocking the caller. | Reliability | T |
| **FSR-07** | A publisher **shall** optionally retain its last message, so that a subscriber which connects later receives that message (latched topic). | Reliability | T |

### 2.2 Frame integrity and data validity

| ID | Functional Requirement | Priority | Verify |
|---|---|---|---|
| **FSR-09** | Every transmitted message **shall** carry a sender identity, a sequence number that increases per sender, the host monotonic send time, and an error-detecting checksum over the whole frame. No configuration **shall** remove these fields or skip their verification. | Reliability | T |
| **FSR-10** | A subscriber **shall** verify every received frame before the application can read it. A frame that is corrupted, truncated, of an unknown format, sent in the future, or a duplicate or reordered sequence **shall** be rejected and counted. A managed cycle input **shall** mark data older than its configured maximum transport age as stale, exclude it from usable input, and count the age rejection. A low-level subscriber without an age limit does not reject data by age. | Reliability | T, A |
| **FSR-11** | For each managed cycle input, the software **shall** report whether its most recent captured data is usable, stale, or has never arrived. A read of a stale or absent value **shall** report an explicit unavailable condition rather than return it as current. | Reliability | T |

### 2.3 Execution

| ID | Functional Requirement | Priority | Verify |
|---|---|---|---|
| **FSR-12** | The software **shall** let the application receive messages by deterministic, non-blocking polling at a time of its choosing, with no dependency on operating-system signals or interrupts on the cyclic delivery path. | Reliability | T, I |
| **FSR-13** | The software **shall** let the application wait for and dispatch messages event-driven, on a thread the application owns. The application **shall** select polling or event-driven delivery per endpoint. Every publisher **shall** wake the event-driven subscribers of its topic on each send, so a subscriber never has to fall back to polling. | Reliability | T |
| **FSR-14** | The middleware core and default SDK operation **shall not** create execution or housekeeping threads. A node **shall** run completely on the caller's main thread. Every middleware callback **shall** run on a thread the application created. A standalone tool, adapter, or explicitly requested optional facility **may** own worker threads. | Reliability | T, I |
| **FSR-15** | The software **shall** provide periodic timers with fixed cadence and report an overrun whenever a cycle deadline expires without the associated work having executed. | Reliability | T |
| **FSR-38** | The software **shall** let an application define periodic processing cycles with application state, input ports, and output ports. Each iteration **shall** capture its inputs at one instant, invoke the application step on an application-owned thread, and publish no output when the step or output validation fails. | Reliability | T, I |
| **FSR-39** | A cycle input **shall** optionally report silence after its configured limit and stagnation when its configured alive value does not change. An ignored fault **shall** remain observable as a fault while it does not count against the cycle. | Reliability | T |

### 2.4 Time

| ID | Functional Requirement | Priority | Verify |
|---|---|---|---|
| **FSR-16** | The software **shall** provide a monotonic time source for process-internal use and permit an externally controlled time source to be substituted during verification and replay. | Reliability | T |
| **FSR-17** | A node **shall** optionally take its time from the `/clock` topic. Its timers, sleeps and loop rates **shall** then follow that time, including a pause or a jump, and **shall** run faster than real time when the clock does, with no change to the node's code. Transport timeouts **shall** stay on the host monotonic clock: a wait for a message, a bounded run and the transport age of a message. A paused `/clock` therefore never suspends these timeouts. A node on `/clock` waits for its clock: when the clock driver dies, its participants wait without a fault, and the process that supervises the driver detects its end. This **shall** be the same for every supported host language. | Reliability | T |
| **FSR-37** | A node **shall not** rewind in place when its framework time goes back: a backward jump of `/clock` (a recording replayed again, a replay that seeks back) **shall** end the run of the timer or cycle that sees it and restart the process. A step back of the host wall clock is not a rewind: it **shall not** restart the process, and host timers continue on the monotonic clock. After a backward seek the restarted process **shall** rejoin the replay before it opens a port. A wait for a time or a deadline in a loop the application owns that started before the jump **shall** end and report the jump. No timer, wait or loop rate **shall** stay blocked until the time again reaches its value before the jump. | Reliability | T |

### 2.5 Configuration

| ID | Functional Requirement | Priority | Verify |
|---|---|---|---|
| **FSR-18** | The software **shall** manage node parameters through an immutable, human-readable configuration description, and **shall** abort node initialization with an explicit, actionable error if a mandatory parameter is absent or malformed, or if an unknown parameter is present. | Reliability | T |

### 2.6 Developer interface

| ID | Functional Requirement | Priority | Verify |
|---|---|---|---|
| **FSR-19** | The software **shall** expose one interface through which every supported host-language binding operates, so that no binding re-implements core communication, integrity, validity, or timing logic on its own. | Reliability | T, I |
| **FSR-20** | The software **shall** provide a C++ SDK for applications and a Rust SDK for the hypha tools (corrected 2026-11-15: the Rust SDK carries no application nodes). | Reliability | T |
| **FSR-21** | The software **shall** generate the message and service types from ROS 1 `.msg` and `.srv` definition files for the C++ and Rust bindings. | Reliability | T |

### 2.7 Tools and introspection

| ID | Functional Requirement | Priority | Verify |
|---|---|---|---|
| **FSR-22** | The software **shall** provide non-intrusive introspection tools capable of inspecting live nodes, active topics, message rates, bandwidth, and payload contents, without requiring any inspected component to be rebuilt, restarted, or reconfigured. Attaching or detaching such a tool **shall not** degrade the $O(1)$ publish-complexity guarantee of active publishers. | Reliability | T, D |
| **FSR-41** | A node **shall** provide its application with a snapshot of its identity, the configuration it read, and its active endpoints and cycles with their effective policies. | Reliability | T |
| **FSR-23** | A tool **shall** be able to obtain the full data definition of a live topic at runtime, from the publishing node, without compiled-in knowledge of the message type. | Reliability | T |
| **FSR-24** | The software **shall** record live topics to a self-describing file. It **shall** support MCAP chunk compression. It **shall** replay a recording in timestamp order while it publishes the recorded time on `/clock`. | Reliability | T, D |

### 2.8 Robustness

| ID | Functional Requirement | Priority | Verify |
|---|---|---|---|
| **FSR-25** | The order in which nodes are started **shall not** affect whether communication is established or whether the system behaves correctly once all nodes are running. | Reliability | T, D |
| **FSR-26** | Normal termination of a node **shall** release every resource it holds — message buffers, communication endpoints, operating-system-level handles, and any registry entries describing it — such that no trace of the terminated node constrains the resources available to the rest of the system. A node that did not end normally (a crash, a kill) stays registered as dead, holding its ports, until a node of the same name starts and reclaims it or `clean-shm.sh` wipes the stopped domain (added 2026-11-15). | Reliability | T |
| **FSR-27** | Abnormal termination of one node **shall not** corrupt the communication path shared with other nodes, and **shall not** cause a surviving node to fail, regardless of how closely the terminated node's resources were associated with the surviving nodes' (Freedom from Interference). | Reliability | T, D |
| **FSR-40** | A process **shall** be able to select a host-local communication domain. Nodes in different domains **shall not** discover or communicate with each other, and an invalid domain name **shall** be reported as an error. | Reliability | T |

### 2.9 External integration

| ID | Functional Requirement | Priority | Verify |
|---|---|---|---|
| **FSR-28** | The software **shall** bridge message traffic in both directions between `hypha` topics and a live ROS 1 graph. The bridge **shall** preserve each ROS 1 serialized payload byte for byte. | Reliability | T, D |
| **FSR-29** | The ROS 1 bridge **shall** support automatic graph discovery and an explicit route description. It **shall** reject an invalid route or an inconsistent message definition before it forwards traffic. | Reliability | T |
| **FSR-30** | The ROS 1 bridge **shall** preserve message identity and latched-topic behavior. It **shall** prevent a message that it forwarded from returning through the opposite direction as a duplicate. | Reliability | T, D |
| **FSR-31** | The software **shall** expose live topics through the Foxglove WebSocket protocol. It **shall** advertise each topic with its ROS 1 type and full data definition, and **shall** stream the original ROS 1 serialized payload and its source timestamp. | Reliability | T, D |
| **FSR-32** | The Foxglove adapter **shall** obtain message definitions at runtime without compiled-in message types. It **shall** withhold a topic whose definition is not available and report the condition explicitly. A client that subscribes after a latched value arrives **shall** receive that value. | Reliability | T |

### 2.10 Retained developer utilities

| ID | Functional Requirement | Priority | Verify |
|---|---|---|---|
| **FSR-35** | The software **shall** launch and supervise a configured set of node processes. | Reliability | T |
| **FSR-36** | The software **shall** provide configurable logging. Logging **shall** be synchronous: it **shall** create no thread, and a write **shall** run on the thread that logs. File sinks **shall** rotate by size. The software **shall** refuse an invalid logging configuration at node creation and **shall** report the file, the table and the key. | Reliability | T |

---

## 3. Non-Functional Requirements (NFR)

| ID | Quality Requirement | Category | Priority | Verify |
|---|---|---|---|---|
| **NFR-01** | The framework core **shall** be delivered as one library, `libhypha_ffi`, against which the C++ SDK and its applications link; the Rust tools link the same core statically (corrected 2026-11-15: two artifacts of one source until the tools merge). No framework state or control logic **shall** exist outside that artifact — every consumer, regardless of host language, **shall** exercise identical logic through it. | Architecture | Reliability | I |
| **NFR-02** | The core implementation shall be written in Rust, or in modern C++ adhering to a strictly enforced safe-subset profile backed by continuous static analysis and compiler enforcement. Memory safety (freedom from buffer overflows, use-after-free, and data races) shall be guaranteed by default across the codebase through language semantics or strict toolchain gates. Any region deliberately exempted from enforcement shall be a small, explicitly identified, and independently audited fraction of the codebase—never the default posture. | Language Safety | Reliability | T, I |
| **NFR-03** | Dispatch latency from a publisher's send call to the corresponding subscriber's callback entry **shall** be less than 500 microseconds at the 99th percentile under rated load. | Performance | Reliability | T, A |
| **NFR-04** | The software **shall** sustain rated load — 46 topics, 72.5 MB/s aggregate bandwidth, and 9,960 messages/s — without message loss for any consumer that keeps pace with its subscribed topics. | Performance | Reliability | T, A |
| **NFR-05** | Every internal transport buffer and subscriber queue **shall** have a fixed, bounded capacity. Memory utilization **shall not** grow without bound during sustained overload. | Resource | Reliability | T, A |
| **NFR-06** | The system **shall** execute continuously for at least 168 hours under rated load with zero crashes, zero memory leaks, and stable 99th-percentile latency. | Dependability | Reliability | T, A |
| **NFR-07** | The middleware **shall** distribute a published message to every one of its subscribers directly, without requiring a central broker, routing daemon, or discovery server process to relay or mediate the message. | Architecture | Reliability | D |
| **NFR-08** | Every code path exercised in a shipped build **shall** be free of unrecoverable runtime aborts, unchecked assumptions about the presence of a value, unchecked access outside the bounds of a collection, and arithmetic that can silently overflow or underflow. | Safety Coding | Reliability | T |
| **NFR-09** | No fault **shall** unwind across the interface boundary described in NFR-01. An internal fault of the library **shall** end the process (fail-stop, abort), and an exception thrown by a foreign callback **shall** be converted into an explicit, reportable error status before it reaches the library. | Robustness | Reliability | T |
| **NFR-10** | Unit and integration test suites **shall** enforce a minimum of 80% line coverage for each core module. | Verification | Reliability | T |
| **NFR-11** | A newly initialized executable **shall** be capable of successfully publishing its first valid message within $100\text{ ms}$ of process launch, independent of external discovery handshakes, dynamic schema compilation, or registry synchronization phases. | Performance | Reliability | T, A |
| **NFR-12** | The transport layer **shall** support payloads from tens of bytes to tens of megabytes through the same configured transport interface, without per-payload reconfiguration or application-managed chunking. | Transport | Reliability | T, A |
| **NFR-13** | The software **shall** operate continuously for years. Every counter that the software reports, and every sequence number that it sends, **shall** wrap to zero after its maximum value and **shall** continue to count. A wrap **shall not** cause a fault, a refused operation or a restart. A receiver **shall** accept the sequence number after a wrap as the next in order, and **shall not** reject it as a duplicate or a reordered message. Every consumer that compares two values of one counter **shall** calculate the difference modulo the counter range, so that a wrap does not show as a decrease or as a large jump. | Robustness | Reliability | T |
