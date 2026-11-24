# Software Interface Specification (SIS) — C ABI

## 1. Scope and Standards Compliance

This specification defines the C Application Binary Interface (C ABI) for `hypha` according to:
- **IEC 61508-3:2010 § 7.4.4** (Software Detailed Design, background reference)

This is the boundary of the shared library `libhypha_ffi.so`. All C++ and Rust client implementations access middleware functions exclusively through these entry points.

Audit the defined exports on the built artifact:
```sh
nm -D --defined-only target/release/libhypha_ffi.so | grep ' T hypha_'
```

This document holds the conventions that every call follows. The contract of each
function, type and constant (its null behaviour, buffer sizes, ownership, destruction,
ordering rules and the statuses it returns) is its comment in the generated header
`crates/hypha_ffi/include/hypha/hypha_ffi.h`. `scripts/gen-ffi-header.sh` copies that
comment from the `///` comment of the Rust wrapper (`crates/hypha_ffi/src`) or C type
(`crates/hypha_node/src`). The run loop, cycles and input supervision are in
[execution-model.md](execution-model.md); the C++ view, parameters and logging are in
[cpp-api.md](cpp-api.md).

## Return convention

Every call that can fail returns `HyphaStatus`:

| Value | Name | Meaning |
|---|---|---|
| 0 | `Ok` | The call succeeded. |
| 1 | `InvalidArgument` | A pointer was null, or a length made no sense. |
| 2 | `InvalidTopic` | The topic name was empty or refused. |
| 3 | `PoolExhausted` | No buffer was free, or the publisher's loan budget (the loans its caller holds) is full. The caller drops this message. |
| 4 | `TooLarge` | The requested buffer is larger than the transport carries. |
| 5 | `Disconnected` | The endpoint is closed. |
| 6 | `Backend` | The transport failed for a reason of its own: a send that fails inside the transport, or an internal loan failure. Each cause has its own backend code in `crates/hypha_iceoryx2/src/lib.rs`. |
| 7 | `Panic` | A foreign callback threw, or returned this status. What the callback did before it threw stays done. A Rust fault inside the library never returns this status: it aborts the process. |
| 8 | `BorrowLimit` | The subscriber already holds its effective maximum. No sample was consumed. |
| 9 | `InvalidFrame` | The received bytes are not an hypha frame. Accessor outputs are unchanged. |
| 10 | `InvalidOperation` | A subscriber was used in a second delivery style: owning receive or `WaitSet` dispatch. No sample was consumed. |
| 11 | `NotAvailable` | The requested value exists in principle but the backend cannot supply it right now (e.g. an unresolved schema). |
| 12 | `InvalidSchema` | A schema record failed validation, or a lookup found a record that does not match the requested identity. |
| 13 | `ResourceLimit` | A fixed internal capacity (schema publishers, resolver subscribers, the command queue) is exhausted. |
| 14 | — | Retired. No function returns it. |
| 15 | `CorruptPayload` | The frame CRC-32C does not match the sample's header fields, payload and topic. |
| 16 | `SequenceGap` | The frame sequence was not newer than the last one from its publisher. |
| 17 | `QosMismatch` | iceoryx2 refused to open the service for this endpoint: its payload or header type or its overflow behaviour does not fit the existing service, its declared `HyphaQos` policy (name, `max_publishers`, `max_subscribers` or `max_depth`) differs from the service's stamp, or its `hypha.qos` attribute is missing or names no policy of this build (another hypha build or a stale segment made it). An endpoint that declares no policy adopts the service's policy and is never refused for it. |
| 18 | `Timeout` | A wait did not finish within its caller-supplied timeout. |
| 19 | — | Retired (`ServerUnavailable`, unused since application RPC was removed). No function returns it. |
| 20 | `InvalidState` | The call is not valid in the current state: a configuration call during a run, a nested run, a cycle call after its commit, an emission reservation left open. |
| 21 | — | Retired (`Stale`; the verdict `HYPHA_CYCLE_VERDICT_STALE` names a stale input). No function returns it. |
| 22 | `ClockRewound` | Simulated time went back under a timer or cycle of a `WaitSet`, or a node on simulated time read a `/clock` lower than its last one: a backward seek. Nothing rewinds in place; `runNode` exits 75 and `hypha launch` restarts the process. |
| 23 | `ApplicationFailed` | The application asked for its run to fail (`hypha_waitset_request_failure`, C++ `failRun()`). The run report carries it with `HYPHA_RUN_REASON_RUNTIME_FAILED`. |

`Panic` reports a foreign callback that threw; the C++ SDK catches the exception
at the boundary and the call is reported as having had no effect. A fault inside
the Rust library is not a status: a Rust panic aborts the process, so no call
continues after one.

The cycle calls (`hypha_cycle_*`, `hypha_input_source_*`) give these causes:

| Status | Cause |
|---|---|
| `InvalidArgument` | A malformed pointer, option or index. |
| `InvalidOperation` | A competing receive style on a managed source. |
| `InvalidState` | An attachment conflict, a topology change during a run, a destroy during a run. An emission reservation left open or committed with none open. |
| `ResourceLimit` | A fixed resource limit, an allocation failure for an emission, or a publisher whose loan count is below an output's messages per step at bind. |
| `TooLarge` | An emission above the maximum payload of its publisher. |
| `InvalidFrame` | A TF input could not decode a transform (an input port's undecodable sample is reported with `hypha_cycle_image_reject_sample` instead and fails nothing; a plain node's transform buffer counts and logs one and fails nothing). |
| `Panic` | A foreign callback threw. |

## Rules that apply to every call

- **Pointers may be null.** Each function checks and returns
  `InvalidArgument`, or does nothing when it returns no status; it does not
  dereference blindly. Only `Ok` means an output parameter was filled.
- **Strings are pointer plus length**, UTF-8, not NUL-terminated, and borrowed for
  the duration of the call only. A null pointer or text that is not UTF-8 is
  `InvalidArgument`. The header names the few calls that take NUL-terminated text,
  and the null pointers that mean "none".
- **Text and byte outputs use one size-query protocol.** `out_len` receives the
  length. A `capacity` that is too small returns `TooLarge` and writes no byte, so the
  caller retries with `out_len` bytes; `out` may be null only with `capacity` 0. The
  output carries no terminating NUL. The caller owns the buffer.
- **Handles are opaque and owned by the caller.** Every `_create`/`hypha_publish_with_qos`
  /`hypha_subscribe_typed` has exactly one matching `_destroy`. Destroying twice, or
  using a handle afterwards, is a caller fault the library cannot detect. The node's
  `WaitSet` and a cycle are borrowed handles that their owner destroys.
- **A loan must be released.** `hypha_publisher_loan_frame` hands
  out a buffer that is returned by either `hypha_loan_send` or
  `hypha_loan_drop`. Losing the pointer holds a transport buffer until the
  publisher is destroyed.
- **A callback context moves only on success.** A successful attach or cycle
  creation takes the `context` and its `release`, and calls `release` exactly once,
  after no callback can run again. A refused call leaves the context to the caller.
- **Counters wrap.** Every counter the ABI reports, in the metrics, a run report, a
  cycle's health and a timer's missed count, is 64 bits wide and wraps to zero after
  its maximum value. It never stops at the maximum and never makes a call fail. A
  reader computes the difference of two readings modulo 2^64. Every `_nanos` field is
  a signed or unsigned 64-bit count of nanoseconds, and every `uint8_t` flag is zero
  or nonzero.
- **Defaults come from the header.** An SDK passes the `HYPHA_DEFAULT_*` constants
  instead of keeping values of its own.
- **There are no event channels and no application RPC.** A wake-up or a request is
  a small message on a topic; a reply is a message on a second topic that carries the
  request's identifier.

## Process and node

The library creates no thread. Nothing is delivered and no timer fires until the
application runs a `WaitSet` on a thread it owns. The `WaitSet` of a node is the one
execution mechanism; its limits and dispatch order are in
[execution-model.md](execution-model.md), sections 3 and 5.

Simulated time, the logging configuration and the stop request belong to the
process, not to one node:

- **Simulated time.** The environment variable `HYPHA_USE_SIM_TIME` alone puts a node
  on simulated time; the parameter file has no switch for it. `1` or `true` is
  simulated time in the replay lockstep; `follow` is simulated time outside it: the
  node reports no progress on `/clock_progress`, so no player or node waits for it, it
  waits for no other node's report, and its plain callbacks run as their samples
  arrive while its timers follow `/clock` (execution-model.md §11.2); `0`, `false` or
  unset is host time, and any other value fails node creation.
  `HYPHA_REPLAY_LOCKSTEP=0` (or `false`) turns the lockstep off for the whole launch: a
  node that `1` or `true` puts in the lockstep follows `/clock` outside it, as with
  `follow`, and `hypha play` waits for no node (replay-flow-control.md §4.6). `1`,
  `true` or unset keep the lockstep, and any other value fails node creation, also on
  host time. The first node of a
  process that starts on simulated time writes one `WARN` line, target `hypha`, after
  its `[log]` table is applied:
  `simulated time is on (HYPHA_USE_SIM_TIME): this node follows /clock`. The first
  sim node starts the clock (at a placeholder zero until `/clock` arrives), a later sim
  node keeps its reading, and destroying a node never turns simulated time off.
- **Stop signals.** SIGINT and SIGTERM ask every node of the process to stop
  (`hypha_node_ok`). The first `hypha_node_create` of a process installs the handler
  for both signals (`crates/hypha_sys/src/signal.rs`), so every SDK and tool on
  the library shares one handler. It does one atomic store and nothing else, so it is
  async-signal-safe. A handler the process installed before still runs after hypha's,
  and without one a second signal ends the process with status 128 plus the signal
  number. A signal the process ignores stays ignored.
- **Domain.** The environment variable `HYPHA_DOMAIN` selects the domain once per
  process, before the first transport call. Every topic, service and node of the process
  lives in it, processes in another domain do not see them, and topic names do not
  change: two domains can each carry `/odom_speed`. The value is `[a-z][a-z0-9_]`, at
  most 32 characters. An invalid value, also one that is not UTF-8, refuses the first
  call that needs the transport and names the variable; it never falls back to no
  domain. `./clean-shm.sh` wipes every domain; `./clean-shm.sh --domain <prefix>`
  only the domains named `<prefix>*` that no live process uses.
- **Restart when time goes back.** A timer or cycle of a `WaitSet` that sees simulated
  time go back, and a `WaitSet` of a node on simulated time (lockstep or follow, not a CLI
  tool) that reads a `/clock` lower than the node's last one, end the run with `ClockRewound`; the process exits 75 (C++ `runNode`)
  and its supervisor starts it again, `hypha launch` with `HYPHA_REJOIN_AFTER_NANOS`
  set. In the new process the first node created on simulated time waits for a `/clock`
  sample sent after that time, for at most 30 s (`Timeout`), and fails with
  `InvalidState` when the process is asked to stop first.
  [replay-flow-control.md](replay-flow-control.md) section 4.4 gives the protocol.

## Transform buffer of a plain node

A node without a cycle looks transforms up through a `HyphaTransformBuffer`
(`hypha_transform_buffer_create`, `_lookup`, `_destroy`). The buffer, its lookup
semantics and its `HYPHA_TF_*` results are those of a cycle's TF input
([execution-model.md](execution-model.md), section 6.7); only the ingestion differs.

- **Create.** `hypha_transform_buffer_create(node, tf, tf_len, tf_static,
  tf_static_len, qos, cache_nanos, out)` subscribes both topics as `tf2_msgs/TFMessage`
  with the policy `qos` (the C++ SDK passes `QosOf<tf2_msgs::TFMessage>`), at the
  deepest queue it allows and without a no-publisher warning, and attaches both to the
  node's default `WaitSet`. `InvalidArgument`: a null pointer, an empty topic, the same
  topic twice, `cache_nanos` 0. `QosMismatch`: a topic of another policy.
  `InvalidState`: the default `WaitSet` runs. A refused create writes null and leaves
  nothing attached and nothing subscribed.
- **Ingestion.** The run of the default `WaitSet` ingests each received message on its
  thread, whatever the clock: no step, no cut. A latched `/tf_static` sample published
  before the create arrives once the node runs. A message that does not decode, or a
  transform the buffer refuses, is counted and logged at most once a second; a loss on
  either topic is the subscriber's missing-sequence count. Neither fails the run.
- **Lookup.** `hypha_transform_buffer_lookup` takes the arguments of
  `hypha_cycle_image_lookup_transform` after the handle. Any thread may call it at any
  time, concurrently with the run and with other lookups: one lock guards the buffer
  for one message's ingestion or one lookup, and no lookup waits on the transport.
- **Destroy.** `hypha_transform_buffer_destroy` detaches both registrations, which
  releases both subscribers; null does nothing. Create and destroy follow the attach
  rules: on the configuring thread, not during a run from another thread
  (`InvalidState`, the handle stays live); a destroy from a callback of the run defers
  the detach to the end of the run. A destroy after `hypha_node_destroy` only frees the
  handle, which names its node weakly.

## Clocks and send times

Two clocks drive the framework, and the split is deliberate:

| Framework time (`/clock` under simulated time) | Host monotonic time |
|---|---|
| `hypha_waitset_attach` (timer kind) under simulated time | `hypha_waitset_attach` (timer kind) on host time |
| `hypha_waitset_timer_reset` and `until_due_nanos` of a timer under simulated time | the same for a timer on host time |
| | `hypha_waitset_run` limits |
| | schema-lookup timeouts |
| `node.last_sweep_end_nanos` metric | |
| | transport age and slow-callback measurement |

A transport timeout detects a fault in the transport or in a peer. A paused
`/clock` must not suspend that detection, and playback faster than real time
must not make a healthy peer look late, so every transport timeout counts
host time.

Every frame carries two send times (see [Wire frame](#wire-frame)), which
`HyphaSampleIdentity` reports. The **transport send time** is the host monotonic
time of the send; a `WaitSet` delivery also reports the age it measured
(`HyphaDelivery.transport_age_nanos`).
The **framework send time** is simulated time on a sim-time node, wall time
otherwise, or the value a replayer passed to `hypha_loan_send`. Every frame a cycle
step sends carries the step's time, and every frame a replayed plain callback sends
carries its event's time (execution-model.md §11.2): a send without a value reads
framework time on the sending thread. Values from different clock sources are not
comparable. On simulated time a replay cycle ages each input by the framework send
time: a step at deadline `D` admits the samples sent before `D`, and their age is `D`
minus the framework send time. Any other cycle ages inputs by the transport send time.

An input binding's silence limit decides, below the ABI, whether a retained value is
still usable; it reaches the binding only through `hypha_cycle_bind_input_supervision`.
Subscribers do not reject frames by age. A cycle judges its bindings at one instant on
the host monotonic clock that `hypha_clock_steady_now_nanos` reads: `NeverArrived`
before the first frame, `Usable` inside the limit, and `Stale` after it or when the
frame was sent after that instant. A binding without a limit never expires. Node
health judges the same bindings with the same rule, so no SDK keeps an age rule of
its own.

## Metrics

`hypha_node_metrics` reports named counters, and the Rust SDK's
`RawSubscriber::metrics` the counters of one subscriber. Each call emits every metric
in the fixed order of the table below; `hypha_node_metrics` emits them through one
`HyphaMetricCallback`. The header states the callback, null and partial-output rules.

`hypha_subscriber_counters` writes the eight subscriber counters as one
`HyphaSubscriberCounters` struct (one `uint64_t` per metric, owned by the caller); the
metrics are emitted from that struct, so the C, C++ and Rust numbers cannot differ. Any
thread may read it, also during a run that dispatches the subscriber.
`integrity.sequence_gap` counts refused duplicate or reordered frames; a lost frame is
counted by `hypha_subscriber_missing_sequences`.

- **Node counters.** Every counter starts at node creation, and every `WaitSet` on
  the node adds to the same counters: the callbacks the executor called, the
  callbacks that reported a failure (a nonzero status or the fail action), the status
  of the last failure, the slow callbacks, and the framework time at which the last
  dispatch sweep ended. A fail action without a status is recorded as `Backend`.
- **Values.** Each counter wraps. `node.last_callback_status` is the status of the
  last failed callback; a negative code is reported as `0`. `node.is_sim_time` is `1`
  while framework time is simulated, else `0`. `node.sweeps` counts the dispatch
  sweeps that ended, one per `WaitSet` turn; `node.last_sweep_end_nanos` is framework
  time and is `0` until a sweep ends.

| Source | Metrics, in order | Emitted by |
|---|---|---|
| Node execution | `node.callbacks_invoked`, `node.callback_failures`, `node.slow_callbacks`, `node.sweeps`, `node.last_sweep_end_nanos`, `node.last_callback_status`, `node.is_sim_time` | `hypha_node_metrics` with a node |
| Transport (process-wide) | `transport.failed_notifications` | `hypha_node_metrics` with a node: sends of this process whose wake notification failed; their samples wait for the next notification of the topic |
| Schema | `schema.invalid_records`, `schema.unavailable_lookups`, `schema.resource_limit_failures` | `hypha_node_metrics` with a node |
| Log (process-wide) | `log.lines_written`, `log.write_failures`, `log.rejected_lines` | `hypha_node_metrics`, with or without a node |
| Subscriber | `integrity.invalid_frame`, `integrity.corrupt_payload`, `integrity.schema_mismatch`, `integrity.sequence_gap`, `integrity.invalid_timestamp`, `loan.budget`, `loan.held`, `loan.refused` | `RawSubscriber::metrics` (Rust SDK); `hypha_subscriber_counters` (C ABI, as a struct) |

`loan.budget` is the effective per-subscriber budget of owning samples; `loan.held`
is the number of samples the subscriber holds; `loan.refused` counts receives refused
because the budget was full.

## Service limits and wildcards

A topic's service limits are its policy (`crates/hypha_iceoryx2/src/qos.rs`): the
publishers and subscribers it admits and its `max_depth`. The creator declares the
policy (`HyphaQos`) or takes hypha's `default` policy, stamps it on the service, and an
opener that declares a different one is refused with `QosMismatch`. No endpoint option
sets a service's depth or counts beyond its own declaration, so a service has the same
shape whatever the start order. The domain's iceoryx2 `Config`
(`crates/hypha_iceoryx2/src/domain.rs`) is iceoryx2's defaults with hypha's overrides;
it holds only the defaults that hypha does not set per service. They follow
`rmw_iceoryx2`, the ROS 2 layer on iceoryx2.

The domain also turns iceoryx2's dead-node cleanup off: `global.node.cleanup-dead-nodes-on-creation`,
`global.node.cleanup-dead-nodes-on-destruction` and `global.service.cleanup-dead-nodes-on-open` are
`false` whatever a host file sets. A node that starts reclaims the nodes iceoryx2 reports as dead
that have its own name, the runs a crash or a kill ended, and every dead CLI tool node
(`_hyphacli_<tool>`). The dead node of an application stays listed as dead until then, so the
tools show it, and keeps its ports; `./clean-shm.sh` wipes a stopped domain.
No host `iceoryx2.toml` is read, in `./config/`, the user directory or the system directory. The
root path, the prefix and the timeouts are iceoryx2's defaults, or the prefix `HYPHA_DOMAIN` selects,
in every process, whatever the working directory.

A service of a topic that is not a framework topic admits its policy's `max_subscribers` declared
subscribers and three more undeclared ones (`qos::RESERVED_OBSERVER_SUBSCRIBERS`), which monitoring tools
and bridges use. A declared subscriber is refused only when its policy's count of declared subscribers is
connected, so an undeclared subscriber never takes a slot a declared one is entitled to. An undeclared
subscriber beyond the three is refused with `ResourceLimit`. Framework topics keep their fixed limits and
reserve nothing.

| Limit | Domain value | Note |
|---|---|---|
| `subscriber_max_buffer_size` | 32 | The `default` policy's `max_depth`: the largest subscriber queue, and the queue of a subscriber that sets none. Every hypha service sets its own |
| `subscriber_max_borrowed_samples` | 2 | hypha copies a sample and releases it at once |
| `max_subscribers` / `max_publishers` / `max_nodes` | 16 / 8 / 24 | The `default` policy's limits, which apply to a service that hypha does not set. Every hypha service sets its policy's `max_subscribers` and `max_publishers`, and `max_nodes` as their sum (`crates/hypha_iceoryx2/src/qos.rs`) |
| `publisher_history_size` | 1 | The ROS 1 latch: the last sample of each publisher |
| `enable_safe_overflow`, `backpressure_strategy` | true, `DiscardData` | A full queue drops its oldest sample; a publisher never blocks |
| `subscriber_expired_connection_buffer` | 8 | Connections of departed publishers that still hold undelivered samples; one per publisher of the `default` policy. A declared policy may admit more publishers than this, and this is a domain value that the builder does not set |
| `publisher_allocation_strategy` | `Static` | A default only: a bounded publisher sets `Static` with its payload limit, a growable one `PowerOfTwo` |
| event `max_notifiers` / `max_listeners` / `max_nodes` | 128 | The graph event, which every node of the domain joins. A topic's event service takes its data service's `max_publishers` / `max_subscribers` / `max_nodes` (the policy's own limits, `default` 8 / 16 / 24), and a `WaitSet` control service 1 / 1 / 1: a notifier keeps about 1.3 KB of heap for every listener its service allows |

A framework topic (`/clock`, `/clock_progress`) holds at most 32 publishers,
32 subscribers and 64 nodes; every node that subscribes `/clock` or reports on
`/clock_progress` counts. Any other topic holds the limits of its policy. After a change of
these values run `./clean-shm.sh`: a stale segment keeps its old limits.

A wildcard subscriber attaches only to a publish-subscribe service that already exists.
It never creates one, it never follows a request-response service of the same name,
and it resolves the pattern **once**, when the subscription is created. Matching a pattern
enumerates every service on the machine, and the backend builds each service's
node states while it does so, so the cost grows with the whole machine rather
than with the subscription: 57 ms at 40 topics, 175 ms at 120. Paying that on
every receive stalled callers, so it is paid once, where a caller expects the
cost of opening an endpoint.

A node that must follow a topic created later lists topics itself and
subscribes to them by name. The matching rule is `hypha_core::port::topic_matches_pattern`,
which the Rust SDK re-exports as `hypha_node::sdk::topic_matches_pattern`, so both routes select
the same set without a node repeating the rules; the C ABI carries no copy of it.

## Wire frame

Every publish-subscribe sample uses one fixed format. No topic selects an
optional integrity profile, and no receive path skips the checks. The
transport is treated as an untrusted channel. Every integer is little-endian.

A sample is two parts. The **header** travels as the iceoryx2 user header of
the data service, in the same shared segment. The **payload** is the message
alone, as ROS 1 serialized bytes, so a payload has no header in front of it.

| Field | Width | Meaning |
|---|---:|---|
| `magic` | 32 bits | Version-specific hypha frame magic |
| `flags` | 32 bits | Bit 0, `FLAG_LATCHED`: the publisher latches this sample |
| `schema_hash` | 64 bits | First eight bytes of the ROS MD5 digest |
| `publisher_id` | 64 bits | Publisher-instance identifier |
| `sequence` | 64 bits | Per-publisher send sequence. It wraps to zero after the maximum value |
| `transport_sent_nanos` | 64 bits | Host monotonic send time |
| `framework_sent_nanos` | 64 bits | Framework send time: simulated on a sim-time node, wall otherwise, or the value set on the loan |
| `crc` | 32 bits | CRC-32C (Castagnoli) that starts from the topic code and runs over `magic`, `flags`, the five 64-bit fields above in order, then the payload |
| payload | variable | ROS 1 serialized message bytes, the sample's user payload |

The magic is `0x3658_4F49` (`IOX6`). It changes when the header layout changes,
and a receiver refuses any other value. Release 0.2.0 introduced the topic-bound CRC.
It kept the 0.1.0 layout and its magic, so a frame from 0.1.0 fails its CRC and is
refused as `corrupt_payload`: the two release lines do not share a domain.

The topic code is the CRC-32C of the bytes of the topic name. It is the start value
of the frame CRC and is not sent: the publisher takes it from the topic it sends on,
the receiver from the topic the sample arrived on. A frame that reaches a subscriber
of another topic therefore fails its CRC. Two topic names give the same code with a
probability of 2^-32.

The transport sets `FLAG_LATCHED` on every sample of a latched publisher, before
the CRC is computed, so the flag cannot be sent without its integrity code. A
subscriber that joins after a sample was sent receives that sample only when the
flag is set. See the latch in `docs/engineering-traps.md`.

The publisher seals the header at send, after the caller has finished the
payload. A replay publisher sets the framework send time to the recorded time
before it sends, and the transport send time and CRC are still written at send.
Source time belongs in the payload.

Frame integrity is mandatory on every publish-subscribe frame, with no option to
select it. Before any accessor or callback sees a payload, the receive path refuses a
frame for these conditions:

| Condition | Status |
|---|---|
| Unsupported magic | `InvalidFrame` |
| CRC-32C mismatch: a header field or a payload byte changed, or the frame was sealed for another topic | `CorruptPayload` |
| Send time later than the host clock | `InvalidFrame` |
| Sequence that is not newer than the last one from its publisher | `SequenceGap` |

Every refusal is counted. The `integrity.*` subscriber metrics (`RawSubscriber::metrics`) read the
counters. The forward gaps that the receiver accepts are counted by
`hypha_subscriber_missing_sequences`.

A publisher never stops for lack of sequence numbers: after the maximum
value the sequence wraps to zero. The receiver therefore measures "newer"
modulo 2^64. A sequence less than 2^63 ahead of the last accepted one is
newer, so the sequence after a wrap is accepted as the next in order. A
duplicate, an older sequence, and a sequence 2^63 or more ahead are refused.

Threat mapping (the communication errors of IEC 61784-3, background reference). The table states
what the code does. It is not a residual-risk argument.

| Threat | Defense | Limit |
|---|---|---|
| Corruption | CRC-32C over the header fields, the latch flag and the payload. | Residual error probability is not assessed. |
| Loss | Sequence counter. | The receiver checks order modulo 2^64, not continuity. A forward gap is accepted. Its count is read through `hypha_subscriber_missing_sequences`, which misses a loss before the publisher's first accepted frame. |
| Duplication | Per-publisher sequence. | A repeated or older sequence from a known publisher is refused. A subscription tracks the 4096 publishers that sent most recently, so its memory stays bounded while publishers restart for years. A publisher that stayed silent while 4096 others sent is forgotten, and its next frame is accepted as its first. |
| Insertion, wrong address | Publisher id and sequence are inside the CRC, and the CRC starts from the topic code: a frame of another topic is refused. | No publisher allow-list and no authentication. Two topics share a code with a probability of 2^-32. |
| Delay | Host monotonic send time and each input binding's maximum age at capture. | Transport age only. Both ends must share one host. |
| Masquerading | A frame must be sealed for the topic it arrives on. | A CRC is not authentication: a process of the domain that publishes on the topic is accepted. |

## Logging engine

The engine behind `hypha_log_*` and the C++ `HYPHA_LOG_*` macros. It has no thread and
no user-space buffer; each line is written on the thread that logs.

- **`[log]` table.** Node creation applies it before the transport starts. It refuses
  with `InvalidArgument`, and one line on standard error naming the file, table and
  key, an unknown table or key, a wrong type, an unknown level, a sink with no or an
  empty `path`, `rotation_mb` (1 to 1024) or `rotation_count` (1 to 100) out of range,
  two enabled sinks on one absolute path, an invalid module name, or a sink file that
  cannot be created or opened. It gives `InvalidState` when another process holds a
  sink's lock or the process already has another configuration. A refused table
  changes nothing. Node creations that overlap are serialised. Lines written before a
  node applies its table use the default (console on at `trace`, level `debug`, no
  file) and are not routed again. A `[log.module.<name>]` entry for a name that never
  registers is not detected.
- **Modules.** `default` is registered at start as identifier 0. The engine keeps at
  most 256 modules; past that a new name (from the C ABI, the Rust `log` facade or a
  framework report) writes under `default`, with no warning. A module's level is
  `log.module.<name>.level`, else `log.level`; `enabled = false` is `off`.
- **Lines.** The prefix is `[<UTC wall time>]`, then under simulated time
  `[sim <seconds>]` (the step's time on the thread of a running cycle step, else the
  clock; a replay that catches up shows each step's own deadline), `[<LEVEL>]`,
  `[<module>]`. Line breaks and carriage returns are kept; an entry has one prefix and
  gets a final newline only when it lacks one. A message above 16 KiB is cut and ends
  with `[truncated <n> bytes]`. The text need not be UTF-8. Only a terminal console is
  coloured. A rejected line (unknown module, invalid level or `off`, null text with a
  length above zero) counts in `log.rejected_lines`; `log.lines_written` counts lines
  that reached at least one target.
- **Writes.** A line is rendered once and written to each target with one `write`
  call, without `fsync`, so a crash loses no written line. Each file sink is an append
  file behind its own mutex, locked through `<path>.lock`, which stays across
  rotations. A failed write counts in `log.write_failures` and writes nothing to
  standard error. Logging never stops a node.
- **Rotation.** The line that takes a file above `rotation_mb` is written whole, then
  `<path>` renames to `<path>.1`, `.1` to `.2`, and `<path>.<rotation_count>` is
  dropped. A failed roll keeps writing the same file, counts once in
  `write_failures`, and is retried by the first line over the limit at least 10 s
  (host monotonic) later. The first failure after a success, or at start, writes one
  `WARN` line, target `hypha`, to every other target: `hypha_log: cannot roll <path>:
  <error>; retrying every 10 s`. A roll that fails part way repeats the chain from the
  top, so a numbered file may be replaced one step early.
- **Console.** On Linux a pipe or terminal on fd 2 is reopened through
  `/proc/self/fd/2` with `O_NONBLOCK` (the parent's flags do not change) and a socket
  (journald) is written with `MSG_DONTWAIT`; a regular file is written as it is. A
  line not written whole is dropped and counted; once standard error is read again one
  line says `<n> log lines dropped: standard error was not read` (a notice not written
  whole is dropped too and the count grows). fd 2 is checked per line, so a later
  redirection is followed. If a pipe or terminal cannot be reopened, console lines are
  dropped and the file sinks get one warning. Off Linux a pipe or terminal is written
  as it is and can block.
- **Rust.** The Rust SDK has no logging API: Rust lines go through the `log` crate,
  whose backend node creation installs, and the `[log]` table applies to them. The C++
  macros do not go through `log`.
  iceoryx2 writes to it at `Warn` and above, under the leading name of its origin
  (`iceoryx2/service`).

## Threading

The library does not serialise calls for the caller. A single endpoint handle
is used from one thread at a time. The node observation calls (`hypha_node_ok`,
`hypha_node_shutdown`, `hypha_node_metrics`) are safe to use while another
thread runs a `WaitSet`, and so are the cycle snapshots `hypha_cycle_health` and
`hypha_cycle_binding`. Call `hypha_node_state` on the thread that runs the node, or
while no thread does. No mutex stays held across user processing.

One thread runs one `WaitSet`. `hypha_waitset_request_stop` and
`hypha_waitset_request_failure` are the calls another thread may make on a
running `WaitSet`, and a stop that arrives before
the run starts is recorded, so the next run returns at once. Any thread may also call
`hypha_transform_buffer_lookup` and `hypha_subscriber_counters` while a run goes on.

Attachments freeze for the duration of a run, with one exception. Create and attach
every subscription and cycle before the run starts; attaching one during a run is
refused with `InvalidState`, and so is any attach, detach or timer call from another
thread. A callback's own detach is deferred to the end of the run (see
`hypha_waitset_detach`). A callback may attach a timer (`HYPHA_ATTACH_TIMER`) when the
node runs on host time or on its own simulated clock: the timer needs no native
attachment, joins the dispatch before the run's next wait, and takes an id above every
id the native set hands out. On a simulated clock driven from outside the node a timer
is polled natively, so the attach is refused. A callback may also cancel, reset and read
a timer (`hypha_waitset_timer_cancel`, `hypha_waitset_timer_reset`,
`hypha_waitset_timer_state`); a cancel or reset holds back a call of that timer still
due later in the same turn, and a reset takes effect before the next wait.
