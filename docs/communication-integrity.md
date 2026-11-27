# Communication integrity

Which check of a hypha frame covers which fault, what the receiver does with it,
what the application must still do, and the limits of the design.

Readers: an engineer who integrates hypha nodes and must argue what a receiver
detects, and an engineer who changes the frame checks.

## 1. Status

- hypha is SIL 0 middleware. This document is not a safety manual in the sense of
  IEC 61508 and claims no compliance with IEC 61784-3. It records the
  engineering design so that a safety argument made elsewhere can cite facts.
- The transport is treated as an untrusted channel inside a closed system: every
  process that can reach the shared memory is trusted (a black channel, see
  [requirements-specification.md](requirements-specification.md)). hypha detects
  faults. It does not defend against a deliberate attacker.
- The normative requirements are FSR-09, FSR-10 and FSR-39 of
  [requirements-specification.md](requirements-specification.md). The frame layout
  is in [interface-specification.md](interface-specification.md#wire-frame), the
  supervision rules in [execution-model.md](execution-model.md) §6.4.

## 2. What every frame carries

The publisher seals the header at send. No configuration removes a field or skips
its check.

| Field | Set by | Purpose |
|---|---|---|
| `magic` | library | Frame format version. Another value is refused. |
| `schema_hash` | library, from the message MD5 | The message definition the sender used. It also names the frame's stream (section 3). |
| `publisher_id` | library, per publisher instance | Which sender the sequence belongs to. |
| `sequence` | library, at each send | Order of the sender's frames. Wraps to zero. |
| `transport_sent_nanos` | library, host monotonic clock | When the frame was sent. |
| `framework_sent_nanos` | library | The step's time: simulated time, wall time, or a replayer's stamp. |
| `crc` | library | CRC-32C over every field above and the payload, started from the code of the topic. The topic name is not sent. |

The payload is the ROS 1 serialized message. The library never decodes it.

The CRC-32C is computed by the `crc-fast` crate: the CPU's CRC-32C instructions fused
with carry-less multiplication (PMULL on aarch64, PCLMULQDQ on x86_64), chosen at run
time, and a table on a CPU with neither. Every path gives the standard CRC-32C:
`hypha_wire/crc32c_check_value` pins it to the published check values and to
`crc32c` 0.6.8, the implementation before, so frames of either build verify in the other.

The code costs one read of the payload at each end: the publisher's at each send, and
each subscriber endpoint's at each frame it takes. Two subscribers of one topic in one
process each verify the frame. A cycle input verifies a frame when it drains it and
decodes it later from its history with no second pass.

| Payload, from memory | Apple M1 Pro, macOS | Linux aarch64, same machine |
|---|---|---|
| 64 bytes | 19 ns | LINUX64 |
| 15 MiB | 0.33 ms (`memcpy` of it: 0.39 ms) | LINUX15 |

Release build, `cargo bench -p hypha_wire --bench integrity`. x86_64 is not measured.

## 3. Faults and the check that covers each

The fault list is the communication errors of IEC 61784-3. "Refused" means the frame never reaches the
application and a counter of the subscriber grows. The counters are the
`integrity.*` metrics; C++ reads them with `Subscriber::metrics()` and
`Subscriber::missingSequences()`.

| Fault | Check | Receiver | Seen as |
|---|---|---|---|
| Corruption | CRC-32C over header and payload | Refused | `corrupt_payload` |
| Unknown or truncated frame | `magic`, sizes | Refused | `invalid_frame` |
| Wrong message definition: a frame whose `schema_hash` is not its stream's | `schema_hash` against the subscription | Refused | `schema_mismatch` |
| Repetition | `sequence` equal to the last accepted of that publisher | Refused | `sequence_gap` |
| Re-sequencing | `sequence` older than the last accepted | Refused | `sequence_gap` |
| Deletion | `sequence` jumps forward | Accepted; the skipped values are counted | `missingSequences()` |
| Delay | age from `transport_sent_nanos`, on the one monotonic clock of the host | A cycle input older than `silence_ms` is stale and not usable | `stale()`, `age_rejections_total`, `~/state` |
| Sender stopped | no frame within `silence_ms` | The cycle input turns stale | `stale()`, `~/state` |
| Send time in the future | `transport_sent_nanos` later than now | Refused | `invalid_timestamp` |
| Relayed source stopped while its relay sends | a life sign in the payload, `setAlive()` with `stagnation_ms` | The cycle input turns stagnant | `stagnant()`, `~/state` |
| Wrong address: a frame reaches a subscriber of another topic | the CRC starts from the code of the topic | Refused | `corrupt_payload` |
| Insertion, masquerade by a process of the domain that publishes on the topic | none | Accepted | — |

Notes:

- A topic name carries one **stream** for each schema (topic name overloading): the
  publishers of one MD5 send on the stream of that MD5, and a subscriber that names an
  MD5 reads that stream alone ([interface-specification.md](interface-specification.md#streams)).
  A frame of another message type, or of another version of the same type, is on another
  stream: it never reaches the subscriber, so it is no fault and no counter grows. A
  frame that does reach it with another `schema_hash` came from a sender that stamped a
  schema other than its stream's, and is a `schema_mismatch`. A publisher of another
  definition is detected all the same: the subscriber counts the other definitions of its
  topic that a live publisher sends (`topic.other_schemas`) and its `~/state` row names
  them (`RequiredInput.other_schemas`), also while its own definition is published too,
  within a second while the node runs and "not checked" before; while its own stream has
  no publisher it also logs them after its no-publisher delay (cpp-api.md §5).
- Each check covers its own faults, and none stands in for another. The CRC says
  nothing about order, the sequence nothing about age, and the send time nothing
  about content.
- A payload that does not change is not a fault. A periodic sender may publish the
  same value for ever; its sequence advances and its frames are current.
- The sequence is tracked for each publisher. The first frame of a publisher is
  accepted as it is, and a loss before it is not counted. A publisher that restarts
  is a new publisher. A sequence half the range or more ahead is refused as not
  newer.
- Under simulated time (development only) a cycle input that loses a sample fails the
  run: a transport gap, or a history eviction of a sample the input still needed; for a
  `Latest` input, an evicted sample that a newer one supersedes is no loss. A
  replay that lost data is not faithful. A queued input's `perStep` drops the oldest
  samples of a step on purpose: counted in `work_limit_drops_total`, no failure.
- In the replay lockstep a participant drains its cycle inputs before it acknowledges
  each `/clock` ([replay-flow-control.md](replay-flow-control.md) §4.2, §6.7). A queue
  of a topic that only the player publishes therefore does not overflow at an input of
  a participant the player waits for: the player splits its chunks to fit the queue.
  A topic that only a clock driver publishes holds the outputs of one driver step,
  which nothing bounds: the queue does not overflow when they fit it. On those paths the
  loss of a publisher's first frames is prevented, not detected. It stays undetectable
  elsewhere: on a topic with another publisher, for that publisher's frames; at a
  participant the sender does not wait for (a node with only plain callbacks, a node
  not yet heard in the first chunk without `--nodes`, a `WaitSet` that has not reported
  yet, the open loop); at the plain callback subscriptions of any node, which are not
  drained before its report; for frames sent before the participant's first turn; and
  on host time.
- A frame is bound to its topic, not to its sender: a trusted process that publishes
  the right message type on a topic is accepted. The deployment's access control is
  the measure against that.

## 4. What the application must do

The library detects; the application decides what a detected fault means.

1. Give every input that counts for health a `silence_ms`. A plain `hypha::Node`
   subscriber has no age limit: the node reads the send time of the newest message
   (`Subscriber::latestSample()`) and judges it itself.
2. Read the verdict before the value: `usable()` or `*port` of a cycle input. A
   stale input keeps its last value in `lastKnown()`; using it is a decision of the
   step, not a default.
3. Declare a life sign with `setAlive()` only for data that reaches hypha through a
   relay (a device driver, the ROS bridge, a gateway). Between hypha cycles the
   frame sequence and `silence_ms` already show a sender that stopped.
4. Size the queue for the traffic (`maxDepth` of the topic's policy). A queue that
   overflows loses frames; the loss is counted, not prevented.
5. Watch the counters in operation: `~/state` of each node, and the subscriber
   metrics. A counter that grows is a finding even while the outputs look right.
6. Do not carry a second age limit or sequence check in the node. One limit for one
   fact: two limits drift apart.

## 5. Limits

- No source authentication: the CRC binds a frame to its topic, not to a sender or a
  topology (section 3). Two topic names share a topic code with a probability of
  2^-32; the schema hash must then match as well for a misdirected frame to pass.
- No inhibit after repeated corruption. Every refused frame is independent.
- No acceptance window: a forward jump of any size below half the sequence range is
  a counted loss, not an error.
- The first frame of a publisher is trusted, and a loss before it is not counted. The
  replay lockstep prevents that loss only on the paths of section 3.
- A 32-bit CRC does not detect every corruption. Random corruption of a frame
  passes with a probability of about 2^-32.
- A plain subscriber is not supervised: silence and the life sign exist for cycle
  inputs only.
- The checks cover the path between hypha endpoints on one host. Data that leaves
  through a bridge or a gateway is covered by that component's own protocol.

## 6. References

- IEC 61508:2010, Functional safety of electrical/electronic/programmable electronic
  safety-related systems (background).
- IEC 61784-3, Functional safety fieldbuses: the communication errors and the black
  channel (background).
