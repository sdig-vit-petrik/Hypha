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
| `schema_hash` | library, from the message MD5 | The message definition the sender used. |
| `publisher_id` | library, per publisher instance | Which sender the sequence belongs to. |
| `sequence` | library, at each send | Order of the sender's frames. Wraps to zero. |
| `transport_sent_nanos` | library, host monotonic clock | When the frame was sent. |
| `framework_sent_nanos` | library | The step's time: simulated time, wall time, or a replayer's stamp. |
| `crc` | library | CRC-32C over every field above and the payload. |

The payload is the ROS 1 serialized message. The library never decodes it.

## 3. Faults and the check that covers each

The fault list is the communication errors of IEC 61784-3. "Refused" means the frame never reaches the
application and a counter of the subscriber grows. The counters are the
`integrity.*` metrics; C++ reads them with `Subscriber::metrics()` and
`Subscriber::missingSequences()`.

| Fault | Check | Receiver | Seen as |
|---|---|---|---|
| Corruption | CRC-32C over header and payload | Refused | `corrupt_payload` |
| Unknown or truncated frame | `magic`, sizes | Refused | `invalid_frame` |
| Wrong message definition | `schema_hash` against the subscription | Refused | `schema_mismatch` |
| Repetition | `sequence` equal to the last accepted of that publisher | Refused | `sequence_gap` |
| Re-sequencing | `sequence` older than the last accepted | Refused | `sequence_gap` |
| Deletion | `sequence` jumps forward | Accepted; the skipped values are counted | `missingSequences()` |
| Delay | age from `transport_sent_nanos`, on the one monotonic clock of the host | A cycle input older than `silence_ms` is stale and not usable | `stale()`, `age_rejections_total`, `~/state` |
| Sender stopped | no frame within `silence_ms` | The cycle input turns stale | `stale()`, `~/state` |
| Send time in the future | `transport_sent_nanos` later than now | Refused | `invalid_timestamp` |
| Relayed source stopped while its relay sends | a life sign in the payload, `setAlive()` with `stagnation_ms` | The cycle input turns stagnant | `stagnant()`, `~/state` |
| Insertion, masquerade | none beyond `schema_hash` | Accepted | — |

Notes:

- Each check covers its own faults, and none stands in for another. The CRC says
  nothing about order, the sequence nothing about age, and the send time nothing
  about content.
- A payload that does not change is not a fault. A periodic sender may publish the
  same value for ever; its sequence advances and its frames are current.
- The sequence is tracked for each publisher. The first frame of a publisher is
  accepted as it is, and a loss before it is not counted. A publisher that restarts
  is a new publisher. A sequence half the range or more ahead is refused as not
  newer.
- Under simulated time (development only) a sample that a queue drops fails the run:
  a replay that lost data is not faithful.
- Insertion and masquerade are outside the design: a trusted process that publishes
  the right message type on a topic is accepted. The deployment's access control is
  the measure.

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

- No source authentication, and no seed that binds a frame to its topic or
  topology (section 3, insertion and masquerade).
- No inhibit after repeated corruption. Every refused frame is independent.
- No acceptance window: a forward jump of any size below half the sequence range is
  a counted loss, not an error.
- The first frame of a publisher is trusted.
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
