# Safety manual

What an integrator must know and do to use hypha in a system with a safety function.
Draft, first issue, not assessed.

Readers: the engineer who writes the safety case of such a system.

## 1. Status of the claim

- hypha is SIL 0 software. No assessor has examined it, and this manual claims no
  compliance with IEC 61508 or any other standard.
- The manual has the form IEC 61508-3 Annex D asks of a compliant item, so that an
  assessment can start from it. [sil2-readiness.md](sil2-readiness.md) lists what is
  missing for systematic capability SC 2.
- Until an assessment exists, an integrator who uses hypha in a safety function
  carries the whole argument: hypha is then software of unknown pedigree in that
  system, and this manual is engineering information.

## 2. The item

| | |
|---|---|
| Item | The library `libhypha_ffi`, its C interface (`hypha/hypha_ffi.h`) and the C++ SDK headers (`hypha/`, `hypha_msgs/`) |
| Version | The release tag; the C ABI revision of `abi.revision` is part of the library file name |
| Platform | Linux on amd64 or arm64, one host; see [release-policy.md](release-policy.md) for the tested distribution and toolchain |
| Function | Publish-subscribe communication between processes of one host over shared memory, with a checked frame around each message, periodic cycles with supervised inputs, timers, and node parameters |
| Not in the item | The tools (`hypha echo`, `record`, `play`, `launch` and the others), the ROS 1 bridge, the Foxglove adapter, simulated time and replay, the Rust SDK of the tools, the examples |

hypha has no safety function. It detects communication and execution faults and
reports them. What a fault means, and the safe state, belong to the application.

## 3. What the item does about faults

The full analysis is [failure-analysis.md](failure-analysis.md). In short:

| Fault | hypha | The application sees |
|---|---|---|
| A frame is corrupted, truncated, of another format or definition, sealed for another topic, repeated, out of order, or stamped in the future | Refuses the frame before the application can read it | A fault counter; no data |
| A frame is lost | Accepts the next one | A counted gap |
| The data of a supervised cycle input is older than its limit, or its sender is silent | Marks the input stale | The verdict `Stale`; a read reports "unavailable" |
| A cycle step throws, or one of its outputs is invalid | Sends none of the outputs of that step | A failed iteration; consumers see silence |
| No free sample, or a message above the topic's limit | Refuses the publish at once | A status; never a blocked call |
| A callback throws | Catches it at the C interface | A counter; the run continues |
| A peer process dies | Nothing; the survivors are not disturbed | Silence on its topics |
| A fault inside the library | Ends the process | Nothing; other nodes see silence |

hypha does **not** detect: a process or callback that hangs, wrong content sent with
a valid checksum, a message from the wrong sender on the right topic, a change of
the data after the check, and anything about a plain subscriber's data age.

## 4. Conditions of use

Every condition is mandatory for a safety function. The IDs are those of
[failure-analysis.md](failure-analysis.md) §4, where each is traced to the failure
modes it covers.

### 4.1 System

| ID | Condition |
|---|---|
| AoU-01 | The system has a watchdog outside the hypha processes and a safe state that is reached without any message from hypha. It covers a process that hangs, dies, or is late. hypha gives no deadline guarantee. |
| AoU-16 | A supervisor outside the item restarts or stops failed processes and limits restarts. |
| AoU-18 | All processes of a domain run on one host and one kernel. |
| AoU-12 | Only hypha processes of one system run under the domain's UID, and nothing else can open its shared memory. |
| AoU-13 | No tool, bridge or replay runs in the domain of a safety function in operation, or a system-level argument shows it cannot affect the function. |
| AoU-17 | Simulated time is not enabled in operation. |
| AoU-19 | One domain runs one hypha version. A version change stops every process and resets the shared memory. |

### 4.2 Sizing

| ID | Condition |
|---|---|
| AoU-06 | Every topic of the domain has a payload limit, and the shared-memory volume holds the sum of all segments at their limits ([containerized-deployment.md](containerized-deployment.md) §3). |
| AoU-07 | Queue depth and step budget are sized for the worst-case rate. A counted loss on an input of a safety function is a fault of the system. |

### 4.3 Application design

| ID | Condition |
|---|---|
| AoU-02 | Every input of a safety function is a supervised cycle input with a `silence_ms`. The step reads the verdict before the value and treats `Stale` and `NeverArrived` as defined cases. |
| AoU-03 | A plain `hypha::Node` subscriber is not used for an input of a safety function, or the application judges age and loss itself from the sample's send time and `missingSequences()`. |
| AoU-05 | A refused publish is a defined case of the application. No output of a safety function is declared `diagnostic`. |
| AoU-08 | Data that reaches hypha through a relay carries a life sign in its payload, declared with `setAlive()` and `stagnation_ms`. |
| AoU-11 | Values that must be consistent are sent in one message. |
| AoU-14 | A restart of a producer is a defined event for its consumers. `Usable` means a fresh sample arrived, not that the producer has converged. |
| AoU-15 | The work of a safety function runs on its own `WaitSet` and thread, or in its own process, with no callback of lower integrity beside it. |
| AoU-20 | A thread of a safety function does not log to a sink that can block, or its log level keeps the hot path silent. |
| AoU-21 | A callback subscription whose data a safety function needs has a check of its own that the data keeps arriving (a cycle input with `silence_ms`, or a watchdog): hypha does not detect a sample left unread by a lost notification. |
| AoU-22 | A `WaitSet` is used by one thread at a time: between runs by any thread, during a run by the thread that runs it and, from other threads, only by the calls the C header allows during a run. A call or a run that breaks this is refused with `InvalidState` and changes nothing (FM-37, #83); the application treats that status from a `WaitSet` call as a fault of its own threading. |

### 4.4 End-to-end protection

| ID | Condition |
|---|---|
| AoU-09 | The residual error probability of the 32-bit checksum for the frame sizes in use fits the budget of the safety function, or the payload carries its own check. |
| AoU-10 | A safety function that needs protection from the data source to the data sink puts its own checksum, sequence and source identity in the payload and checks them on receipt. |

The transport (iceoryx2 and the shared memory) is the plain channel. hypha is the
safety communication layer above it: its frame checks are the measures against
corruption, repetition, wrong order, loss, delay and a wrong address: a frame is
bound to its topic. Two parts of that layer are missing today
([sil2-readiness.md](sil2-readiness.md) G-36, G-37): nothing binds a frame to its
sender, and no residual error rate has been calculated. Until both are closed, a
safety function whose integrity depends on the message content applies AoU-09 and
AoU-10.

### 4.5 Operation

| ID | Condition |
|---|---|
| AoU-04 | The system reads the counters and `~/state` of every safety-related node in operation and treats a growing fault counter as a finding. |

## 5. Configuration

- Build the application and the library from one release. A different C ABI revision
  is refused at start.
- Declare the limits of a topic (payload limit, queue depth, number of publishers
  and subscribers) in one header that every node of the system includes. Two nodes
  that declare different limits for one topic do not connect; the second is refused.
- A node refuses to start on a missing, malformed or unknown parameter. Treat that
  refusal as a deployment fault, not as a case to retry.
- Give each system on a host its own domain and its own UID.

## 6. What to verify in the system

The integrator's validation should show, on the target:

1. Each safety-related input turns `Stale` within its limit when its producer is
   killed, stopped (SIGSTOP) and restarted.
2. The watchdog reaches the safe state when a safety-related process hangs.
3. Under the worst-case load, no loss counter of a safety-related input grows and the
   cycle misses no period beyond the system's tolerance.
4. The shared-memory volume is not exhausted with every topic at its payload limit.
5. No tool or bridge process is present in the domain in operation.

## 7. Known limits and open points

- The points [failure-analysis.md](failure-analysis.md) §5 leaves open: damage to the
  transport's own shared state, allocation failure, two hypha versions in one domain.
- The transport (iceoryx2) and the compiler are not assessed
  ([dependency-assessment.md](dependency-assessment.md),
  [safety-plan.md](safety-plan.md) §8).
- No latency, load or endurance figure has been measured on a reference platform.

## 8. Reporting a defect

Report a suspected safety-relevant defect as [safety-plan.md](safety-plan.md) §7
describes. State the release, the ABI revision, the platform and the counters of the
affected node.
