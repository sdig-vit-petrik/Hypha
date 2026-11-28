# Engineering traps

Read the relevant entry when changing transport, execution, schema, or tools. These are observed failure modes; `AGENTS.md` sets the active working rules.

- **A publisher on wall time starves a simulated-time cycle.** Its samples are
  stamped far past every replay deadline, so no step admits them, and the cycle
  reports `NeverArrived` with no error. The node logs `replay time base mismatch`.
  Run the publisher on the same time base as the replay.
- **A test that asserts how fast the host is fails on a loaded host.** Every test is
  deterministic: it asserts what the code computes or counts, never how soon the
  operating system answers. Assert the wait or deadline the code asks for (a cycle
  step's `scheduled_nanos`, its `missed_periods`), drive time with a simulated clock
  that the test moves (`common::replay::SimulatedClock`), count turns or wakes per
  event, or let a callback end the run and assert the stop reason. A time limit may only
  guard against a hang, in whole seconds; nextest ends a test after 180 s. A latency
  figure is a measurement (`examples/bench`), not a pass/fail test, and a flaky timing
  test is rewritten, never given a wider window or a per-platform one (#64).
- **Every limit has one source: the domain `Config`, a service policy or `EventLimits`. Any other builder that calls a limit setter is a bug.**
  Publish-subscribe, event and RPC limits, safe overflow with `DiscardData`, the one-sample
  history and the expired-connection buffer live in `crates/hypha_iceoryx2/src/domain.rs` and follow
  `rmw_iceoryx2`. A publisher's allocation strategy and payload capacity are its own, not a
  service limit, so the transport sets them on each publisher. Two kinds of service set limits
  that depend on the service, each from one place: a data service takes its policy
  (`qos.rs`: the publishers, subscribers and depth it admits), and an event service takes
  `EventLimits` (`notification.rs`), a topic's from its data service and a `WaitSet` control
  service 1 / 1 / 1. The 128 ports of the `Config` are for the graph event only: a notifier
  keeps 1.3 KB of heap per listener its service allows, which was 160 KB in every publisher
  while topics took the 128. Every process in a domain loads the same `Config`, and every
  creator stamps its policy on the service, so every creator creates the same service
  whatever the start order. A subscriber sets only `buffer_size` (unset: the service's
  `max_depth`, 32 for `default`), and a larger one is refused with `ResourceLimit`.
  A stale segment keeps old limits and fails the open; run `hypha clean` with the domain stopped
  (`./clean-shm.sh` for every domain of the host) after changing a value or upgrading hypha.
- **A growable publisher keeps bookkeeping of 8 bytes per pool sample for each of 32 segments,
  and reserves its pool at its first slot size.** iceoryx2's `PowerOfTwo` allocation sizes its
  bookkeeping for every segment it could create: the publisher's reference counters and segment
  table, and each subscriber connection's segment table and used-chunk list. Upstream allows 255
  segments; hypha patches it to 32 (`vendor/README.md`), since a growth jumps to the next power
  of two that fits. The publisher's share is pool samples × 8 bytes × 32, where the pool is
  `subscribers × (max_depth + 2) + 1 + loans` (`qos::pool_samples`), where `subscribers` is
  `max_subscribers` plus the 3 reserved observer slots (`qos::subscriber_capacity`). For the
  `default` policy that is 649 samples, about 166 KB per growable publisher; for the framework
  topics 1,091 samples, about 280 KB. The data segment itself is pool samples × (first slot
  size + 104 B of headers): 4096 B unless the endpoint names `initial_payload` (C ABI
  `HyphaQos::initial_payload`, C++ `Qos::initialPayload`), so a topic of 200 B messages
  reserves 20 times what it uses. A past-the-limit growth fails the loan (`OutOfMemory`). A variable-size endpoint is growable unless it names a
  byte limit (`max_payload_dynamic`, or `PayloadCapacity::Bounded` in Rust); a bounded one uses
  `AllocationStrategy::Static`, which reserves address space of samples × limit, and only
  written samples are resident. A fixed-size type is bounded to its exact size from its
  definition whatever is asked. Measure before naming a limit on a publisher that does not need
  one, and before making a large-payload publisher growable.
  A growable publisher's growth is not checked against the free space of `/dev/shm`.
- **An undeclared publisher creates the service with the `default` policy, and a subscriber that
  declares nothing never creates one.** A publisher with no declared policy adopts whatever
  service exists, and creates `default` (depth 32, 16 subscribers, 8 publishers) when none does.
  A framework topic (`/clock`, `/clock_progress`) is created with the framework's
  limits (32 / 32 / 32) by whichever process opens it first, because `qos::FRAMEWORK_TOPICS`
  names it: the `/clock_progress` input of a node asks for depth 32 (one report per node), and a
  tool that created that topic with `default` would refuse it. A subscriber that declares nothing
  is refused with `NotAvailable` on any other topic until a publisher opens it, so a tool that
  subscribes first waits until `BusNode::list_topics` lists the topic. The creator's policy then
  binds every later endpoint: an endpoint that declares a different policy is refused with
  `QosMismatch`.
- **macOS has no `/dev/shm`, and the transport still works.** iceoryx2 keeps
  every resource under `/tmp/iceoryx2` there and reaches shared memory through
  `shm_open`, which no directory exposes. Do not read the absent `/dev/shm` as
  a broken environment: bus-backed tests run normally. Look for stale
  `iox2*` under `/tmp/iceoryx2` on macOS, not `/dev/shm`.
- **A node with a few dozen ports needs more than 256 open files.** iceoryx2 holds
  several descriptors per service (shared memory, static configuration, event
  sockets): the MRS `control_manager` holds about 320. A macOS terminal starts
  processes with a soft limit of 256, and past it every service creation fails with
  `InternalFailure`, which names no cause. The first use of the domain therefore
  raises the soft limit to the hard limit, at most 10240 (macOS `OPEN_MAX`), in
  `crates/hypha_iceoryx2/src/preflight.rs`. It warns when that fails. A hard limit
  that is itself too low (`ulimit -n 256` in bash sets both) still fails; a service's
  server or client then reports `OutOfDescriptors` with the open count and the limit
  (`service/mod.rs::explain_descriptors`), a topic still `InternalFailure`. A tmux
  server passes its own limits to every window.
- **A domain prefix must not begin another.** iceoryx2 lists a domain's services by
  file-name prefix. With `iox2_<domain>_`, a default-domain tool (`iox2_`) listed every
  domain's services: long names warned `violates the naming rule.
  [ExceedsMaximumLength]`, short ones were read as its own; domain `t` listed `t_1`.
  A domain's prefix is `iox2-<domain>-` (`domain_prefix` in
  `crates/hypha_iceoryx2/src/domain.rs`): `-` is in no domain name.
- **Stale `/dev/shm/iox2*` survives rebuilds.** A `PortError::Backend(...)` right
  after a capacity or version change is usually stale shared memory, not code.
  Dead-node cleanup is hypha's. The domain `Config`
  (`crates/hypha_iceoryx2/src/domain.rs`) turns `cleanup-dead-nodes-on-creation`,
  `cleanup-dead-nodes-on-destruction` and `cleanup-dead-nodes-on-open` off, and reads
  no host `iceoryx2.toml`. A node that starts reclaims the nodes iceoryx2 reports
  `Dead` that have its own name, and every dead CLI tool node (`_hyphacli_<tool>`;
  `transport.rs::reclaim_dead_nodes`). A dead application node of another name
  stays listed, so the tools show the crash, and keeps its ports: a topic whose
  slots it holds stays full until the node restarts or the domain is wiped.
  A service is the exception: a dead node that holds a service's server or one of its
  clients is reclaimed by the next server or client of that service
  (`service/open.rs::reclaim_dead`), since the name's one server slot, or a client slot,
  would otherwise stay taken for good.
  This does not remove the need for `hypha clean`: a segment whose owning
  process is still alive but wedged is not `Dead`, a node iceoryx2 reports as
  `Inaccessible` is left untouched, and a dead node whose name never returns is
  never reclaimed. A dead node is removed by `hypha clean` of its stopped domain;
  a wedged process keeps its domain in use until it is stopped (or
  `./clean-shm.sh --force` kills it).
  A dead CLI tool node is reclaimed by any node creation in the domain, not only
  in the process that kills it. A test that kills one and inspects the registry
  can therefore be reaped early by another process's node creation in its domain,
  one reason every nextest process runs in a domain of its own (below).
- **Two endpoints opening one service at the same moment is normal, and the
  backend reports it as an error.** iceoryx2 answers `HangsInCreation`,
  `IsBeingCreatedByAnotherInstance`, `IsMarkedForDestruction` or `SystemInFlux`
  while another endpoint is working on the same service. iceoryx2 waits up to its
  own `creation_timeout` inside an attempt (1 s by default in 0.10; the same setting
  bounds connection setup, so hypha leaves it alone). Nodes that start together fail
  together and then retry together, so the wait between attempts is offset per
  endpoint. `crates/hypha_iceoryx2/src/service_open.rs` holds the one retry: no new
  attempt starts 500 ms after the first, and the node lock is held for one attempt
  only. A retry is logged at DEBUG (`hypha/transport`, "is in use by another endpoint,
  attempt N"); a WARN for the normal start of a publisher and a subscriber reads as a
  fault. An open that outlasts the window is logged once at ERROR by its caller, with the
  cause. Every `open_or_create` site goes through it. Do not add an `open_or_create` that
  bypasses it, and do not widen the retry to a state that cannot change — an
  incompatible type or capacity must still fail at once.
- **`ServiceInCorruptedState` on an event service is usually contention, not
  corruption.** A service is created and released in two steps: a static part
  and a dynamic part. An endpoint that reads the pair while another endpoint is
  between those two steps — creating them, or releasing them as its last
  endpoint drops — gets this state, and it clears by itself. It therefore
  belongs in `event_is_transient` and goes through the same bounded retry as
  every other overlap state; only an exhausted retry reports `Backend(22)`.
  Before this, node churn on a shared event service refused roughly one
  endpoint in twenty-five under four concurrent workers, and the failure looked
  like shared-memory corruption.
  `crates/hypha_iceoryx2/tests/event_service_creation_race.rs` holds it as a
  test.
- **A wildcard subscription resolves its pattern once, at creation.** One
  discovery pass calls `iceoryx2::Service::list`, which builds every service's
  node states while it enumerates, so the cost is O(services x nodes)
  file-lock probes and grows with the machine, not with the subscription:
  57 ms at 40 topics, 175 ms at 120. It therefore runs in
  `Iox2Subscriber::new_wildcard` and never on the receive path, which is a
  plain round robin over a fixed set. Do not put discovery back into
  `try_receive` -- not on a timer either. Repeating it at 50 ms cost
  `fluid_view` 99.4 % of a core with 40 particles (95 % inside
  `Service::list`, while the 40 publishers together cost 6 %) and `hypha top`
  61.6 %; pacing it to 2 s traded that for a 766 ms frame stall every
  ceiling. A failed enumeration refuses the subscription, because a partial
  one would silently follow only some matching topics for the life of the
  subscription.
  A node that must follow a topic created later lists topics itself and
  subscribes by name. `hypha_core::port::topic_matches_pattern` owns the matching
  rules and reaches the Rust tools as `hypha::topic_matches_pattern`, so no node
  writes a second matcher. `hypha_core::port::is_valid_topic_pattern` owns which
  patterns are accepted: a `*` must be a whole segment, and `**` only the last
  one. `open_subscriber` refuses any other pattern with `InvalidTopic`. `apps/` owns topic-following
  behavior in tools and the ROS bridge. `crates/hypha_iceoryx2/tests/wildcard_topic_lifecycle.rs`
  holds both halves of the contract.
  A wildcard subscription is itself an endpoint of every service it follows,
  so a service never disappears from a scan while the subscription holds it.
  Do not add code that reaps a topic when its service vanishes: it cannot
  run.
- **Several nodes reclaim one dead node at once.** Two nodes that start
  together can both reclaim the same dead node: two runs of a CLI tool, or any
  two starts and a dead CLI tool node. iceoryx2 lets one process clean it; the
  other waits up to `RECLAIM_TIMEOUT` (1 s) and then finds it clean. In one
  process only one thread reclaims at a time, so `hypha top`'s schema resolvers,
  which start together, wait for each other. A reclaim that fails for the node's
  own dead run stops the node from starting (`Backend`, code 41), because the run
  still holds the name's ports. A dead CLI tool node's failed reclaim is logged at
  `Error` with its name and pid, and the new node still starts. A reclaim that
  fails because another process reclaimed the node first is not a failure.
- **An undeclared subscriber takes only the reserved observer slots.** A monitoring
  tool that opens `**` cannot take the policy's slots of a declared subscriber:
  observers are counted by their port name's prefix (`hypha:observer:`), and a declared
  subscriber is refused only when the declared ones fill the policy (`qos::declared_slots`).
  A crashed publisher is not counted: `publisher_count` drops the ports of nodes the
  registry reports dead, and that listing runs only when a service has a publisher.
- **Contention is not a `QosMismatch`.** Reporting a busy service as a settings
  mismatch sends the operator to inspect a `QoS` that was never wrong. When the
  attempts run out, `service_open.rs` reports `Backend(20)` for a contended
  service, and keeps `QosMismatch` for a service whose fixed settings genuinely
  do not admit the endpoint.
- **Isolated containers share two mounts, not one.** Mount common writable
  storage at `/dev/shm` and `/tmp/iceoryx2`, and use the same numeric UID/GID.
  The deployment recipe is `docs/containerized-deployment.md`.
  The latter path holds metadata and notification Unix sockets. Shared PID,
  IPC, and network namespaces are not required by the current backend.
  The first use of a domain in each process checks both (`preflight.rs`):
  - An entry of `/tmp/iceoryx2` or a `/dev/shm` file of the domain owned by
    another UID refuses the node with `InvalidState` and `domain '<d>' belongs
    to uid <owner>, this process runs as uid <me>`. iceoryx2 itself reports only
    an internal error.
  - A marker file `<prefix>hypha_mount_id` in both places must agree. A process
    with its own `/tmp/iceoryx2` is refused with `InvalidState` and `/dev/shm and
    /tmp/iceoryx2 are not shared by the same processes`; without the check it
    would only find no publisher. A shared `/tmp/iceoryx2` with separate
    `/dev/shm` mounts passes this check (every `/dev/shm` gets the shared id).
  - Give the hypha volumes only to hypha containers, run them under one
    dedicated UID that nothing else uses, and do not mount the host's
    `/dev/shm` into unrelated containers. Any process with that UID can change
    a sample before its publisher seals it or after the subscriber validated it,
    and the frame CRC detects neither (`docs/architecture.md` §3.4).
  `/dev/shm` must hold the sum of the publishers' `.data` segments that gets
  written: a segment is a sparse tmpfs file, and a write past the free space
  stops the process with SIGBUS. On Linux the process first writes one line that names the
  full `/dev/shm` (`crates/hypha_sys/src/signal.rs`). Freed slots are reused last-in first-out, so
  only the samples held at one moment are resident; a full pool is every subscriber
  queue full. A bounded publisher whose full pool exceeds the free space at creation
  logs `may need <n> bytes of shared memory`: slots × (104 B of headers + payload,
  aligned to 8), with slots = subscribers × (depth + 2 borrowed) + history + loans of
  the topic's service policy (`qos::pool_samples`): 649 for `default` (a 64 KiB topic about
  43 MB), and 1,091 for the framework topics (a 64 KiB topic about 71 MB).
  Budget for the whole pool: with subscribers that drain at different rates every slot was
  written, so a large-message topic needs the whole pool resident. Give a variable-size
  topic of known size a byte limit on its endpoints. Growable publishers are not checked.
  A bounded publisher whose full pool takes more than half of the whole `/dev/shm` logs a
  warning too, though it fits what is free: the topics of a domain share it. A bounded
  publisher whose pool is a megabyte or more logs it at start (`publisher pool: <n> samples
  of <b> bytes, <t> bytes`), and `hypha topic pool` gives the number before a node exists.
  The rule and worked numbers for an image or a cloud are in `docs/cpp-api.md` §7.1.
- **A sample's slice is not its message.** Since `IOX7` the message is the first
  `payload_len` bytes of the iceoryx2 slice; the slice keeps the size the publisher loaned,
  so a shrunk loan (`hypha_loan_shrink`) leaves bytes past the message (zeros, never an
  earlier frame's). A new reader, raw or zero-copy (recorder, bridge, probe), takes
  `Iox2Sample::payload()` after `receive_validated*`, never `sample.payload()` of iceoryx2
  or the slice's length. `payload()` before verify gives no bytes for a header whose
  `payload_len` exceeds the slice, but its bytes are not covered by any CRC yet.
- **The transport uses `iceoryx2::service::ipc_threadsafe::Service`, not `ipc`.**
  `ipc` ports are `!Send`, and an executor worker serving an endpoint the
  application created on another thread would then be undefined behaviour.
  `crates/hypha_iceoryx2/src/lib.rs` asserts `Send` on the transport and its
  endpoints at compile time; do not switch the alias back to `ipc`.
- **Nothing polls a listener: a sample wakes its reader only by its notification.**
  A `Trigger::Event` registration runs when its listener is notified, and the run
  loop otherwise waits for its timers, at most `IDLE_BACKSTOP` (1 s) for a signal
  (`tests/waitset_wake_cadence.rs`). A sample whose notification is lost or fails waits
  for the next notification of its topic. iceoryx2 0.10.0 lost notifications (its
  event state stayed `NOTIFIED` with an empty socket); 0.10.1 fixes it (iceoryx2
  #2214), so do not pin an older iceoryx2. A failed `notify()` after a send is logged
  and counted (`transport.failed_notifications`), never retried: the sample is already
  visible. Do not add a periodic read to recover a lost wake: a cycle input reports a
  silent topic as `Stale` after `silence_ms`. Application event ids and counts stay
  queued until their callbacks run, including across a stop.
- **A `WaitSet` is built once per run loop.** Re-creating it per
  cycle measured 1.5 % of a core idle. Attach listeners again only when the set
  of subscriptions changes, and let a build failure refuse the subscription
  rather than retry in a loop nobody can see.
- **Logging is one configuration for each process, and the first `[log]` table
  wins.** Node creation gives the `[log]` table of its parameter file to
  `hypha_log`. A later node with an equal table is accepted. A later node with a
  different table fails with `InvalidState`. The configuration stays after the
  node is destroyed. A relative sink path resolves against the directory of the
  file that wrote it. Another process that holds the lock on a sink file also gives
  `InvalidState`. A second `Node` with another parameter file also throws
  `std::runtime_error` with status `InvalidState` (cpp-api §2). The engine
  creates no thread: a slow disk delays the thread that logs.
- **`unsafe` is `forbid` workspace-wide.** Six crates set their own
  `[lints]` table: `hypha_node` and `hypha_tools` repeat the workspace levels and forbid
  `unsafe` too; `hypha_ffi` (C ABI) and `hypha_sys` (operating-system calls) allow
  `unsafe`, and `hypha_testkit` denies it and allows it in `src/env.rs` alone (the
  tests' `set_env` / `remove_env`). `hypha_testing_mcap`, the test-only MCAP reader,
  denies it and allows it in `src/ffi.rs` alone (its C boundary). `hypha_node` repeats the table and forbids it. The
  SDK `hypha` calls the safe functions of `hypha_node::safe` and forbids `unsafe`
  itself. The per-crate table must repeat every other workspace lint — relaxing
  one must not relax the rest. Do not add another crate without a real reason.
- **`clippy::rc_mutex` is denied.** `Rc<Mutex<_>>` is not a fix for
  `arc_with_non_send_sync`; use a scoped `#[allow]` with a reason instead.
- **Terminal tools in `apps/` must restore the terminal.** Node creation
  installs hypha's SIGINT and SIGTERM handler
  (`crates/hypha_sys/src/signal.rs`): a signal turns `hypha_node_ok` false,
  and a handler installed *before* the node still runs after it. A handler
  installed *after* the node replaces it, and `ok()` then never sees the
  signal. Register yours before the node, or check your own flag, so the app
  reaches its own cleanup instead of dying in raw mode. See
  `hypha topic`. iceoryx2 installs no handler: every node and `WaitSet` that
  hypha builds sets `SignalHandlingMode::Disabled`. Keep it that way, or
  iceoryx2's handler replaces hypha's at the first wait.
- **A data frame carries a schema hash, not a type identity.** `RawSample`'s
  `data_type` and `md5` are empty on receive paths; the wire header holds only
  `schema_hash`. Resolve the hash through `BusNode::resolve_schema_hash` or
  `hypha::SchemaIdentityCache`. Never key deduplication,
  routing, or channel identity on `md5` from a received sample.
- **A `.srv` half carries its own MD5, not the service's.** The request and response
  types are identified by their own message text, as ROS 1 gives them; the service MD5
  hashes both texts together. A publisher checks its MD5 against its definition text,
  so a half given the service MD5 is refused as a topic. A recording of a half made
  before this change keeps the service MD5 and fails the same check on replay.
- **`byte` is int8 and `char` is uint8, as in ROS 1.** The spelling is what the MD5
  hashes, so no hash changes. The generated C++ and Rust types and
  the reflective tools must agree on the sign: a `byte` of 200 reads as -56, a `char`
  of 200 as 200. A constant is checked against the same range when it is parsed:
  `byte X=200` is refused, so C++ cannot wrap it to -56 while Rust refuses to compile.
- **A message constant is checked like a value, and the wire is little-endian.** A
  constant that is not a value of its type fails generation with file and line
  (`line 2: constant value 200 is out of range for byte (-128 to 127)`). An integer is
  decimal or `0x` hexadecimal, a float a finite decimal, a `bool` one of `true`, `false`,
  `True`, `False`, `1`, `0`; a constant cannot be an array, a `time` or a `duration`. The
  tools refuse such a definition from the bus the same way. A C++ publish measures the
  message once and writes it once into the loan, and copies fixed-size numeric arrays
  (`float64[36]`) as one block, so the build refuses a big-endian target. The checked
  writer still throws `StreamOverrunException` on a buffer that is too small.
- **A field or constant name is a ROS 1 name.** It is a letter followed by letters,
  digits or `_` (`[A-Za-z][A-Za-z0-9_]*`, genmsg's rule); any other name, such as `a-b`
  or `2x`, fails generation with file, line and the name. Text after a field name is
  refused, not folded into the name: a ROS 2 default value such as `float64 x 1.0` fails
  as unexpected text after field `x`, so a port must drop it (ROS 2 syntax is #45). The
  tools refuse such a definition from the bus the same way. Keywords are legal names;
  the emitters rename them (`class_`, `r#type`).
- **A definition expands to at most 65 536 nested messages.** Shared types make a
  definition a diamond: 64 levels of two fields are 2^63 paths. Resolution refuses a
  definition past `MAX_EXPANDED_MESSAGES` (the root, plus each message field's count
  times its type's instances; a variable array counts 1), the size walk is memoised
  per type, and a walker that does not resolve first (`msg_reflect`) calls `check_expansion`. A fixed size past `usize` is refused, not
  turned into a growable payload.
- **A wire count reserves nothing it cannot back.** An array reserves room only when
  its element has a known minimum wire width, so the bytes present bound the count. A
  message element has no width: the count is held to the element budget and the
  elements are added as their bytes are read.
- **Every recursive walker over a definition counts its levels against
  `MAX_NESTING_DEPTH` (64).** The one constant lives in `hypha_msgdef` and counts the
  root as level 1. A type reached again on a deeper path is walked again, so a type
  shared by a shallow path and a deep one is checked on the deep path. A recursive
  definition (`Root child`) reads no bytes per level, so only that limit stops a
  walker before the stack does.
- **`crates/hypha_ffi/include/hypha/hypha_ffi.h` is generated, not hand-edited.**
  It comes from `crates/hypha_ffi/cbindgen.toml` via `./scripts/gen-ffi-header.sh`;
  `./check.sh` fails when the committed header is stale. If a Rust ABI change
  needs a header change, run the script and commit the result — do not edit the
  header directly.
- **The attachment registry lives in `crates/hypha_node/src/waitset.rs`, not in
  `crates/hypha-cxx/include/hypha/backend/`.** A subscriber handle is one owner of
  an `Arc<HyphaSubscriber>`: `hypha_waitset_attach` with kind
  `HYPHA_ATTACH_SUBSCRIPTION` takes a second owner for the registration, so the
  subscriber lives until `hypha_waitset_detach` or the destruction of the
  `WaitSet`, whatever the caller does with its handle. The callback context is not
  shared that way: it is borrowed until the registration releases it, so
  `hypha_waitset_detach` still runs before the C++ object that owns the context
  goes. A timer is owned by its attachment: kind `HYPHA_ATTACH_TIMER` takes a
  period, not a timer. `SubscriberHandle`/`TimerHandle` in
  `backend/subscriber_handle.h` and `backend/timer.h` detach before they give up
  their state; keep new handle types doing the same.
- **While a `WaitSet` runs, only its run thread forms `&HyphaWaitSet`; only `Drop`
  forms `&mut`.**
  `hypha_waitset_request_stop` runs on another thread while `hypha_waitset_run`
  holds the same handle: the stop touches only the `WaitSetShared` (`running`, the
  native stop, the failure flag), which the C entry reaches through the raw handle
  without a reference to the `!Sync` `WaitSet`, and the run borrows the `RefCell` fields
  (`native`, `registrations`, `cycles`, `input_sources`) for its length. Every other
  C entry that names a `WaitSet` or a cycle claims it first (`WaitSetShared::claim`,
  through `hypha_ffi` `waitset_here`, whose result holds the claim until the call
  returns) and returns `InvalidState` when another thread holds it; a run claims it for
  its whole length (`waitset_run`). The claim is one compare-and-swap of an owner token,
  so of a call and a run that start at once exactly one gets the `WaitSet` (#83); a
  call from a callback of the run is inside the run's claim. A thread other than the
  holder never forms `&HyphaWaitSet`. A Rust path that reaches a `WaitSet` without a C
  entry (the transform buffer of a plain node, an input source's destroy) claims it
  itself. The cycle snapshots are read
  through the C handle's `Arc<CycleSnapshots>` alone. A configuration call on the
  run's own thread checks `running` first, then `try_borrow_mut`, and reports
  `InvalidState` instead of panicking when a borrow is live. Do not add a field a
  run mutates outside a `RefCell`, and do not form `&mut HyphaWaitSet` anywhere
  but `Drop`.
- **`backend/session.h` includes no adapter, and must not.** Seven adapters include
  it. An umbrella that `session.h` includes re-enters an adapter that is still half
  declared, so the build fails with `unknown type name 'RawPublisherHandle'`. A public
  header includes each `backend/` adapter it uses.


---

- **A cycle output's messages are loans held across the step.** `Output::send()`
  reserves a transport loan of the message's serialized length and writes the
  message into it, once. The loan stays with the step until the step returns. A
  successful step sends the loans in send-call order. A failed step releases them
  unsent. Four rules follow:
  - The output's publisher must hold one loan per message of the step. Its loan
    count is the output's `perStep()`, set at bind. A publisher with fewer loans is refused
    with `ResourceLimit`, because its shortfall would look like a full pool.
  - iceoryx2 reserves the loan count in each publisher's own data segment, so
    raising it changes the segment size. Run `hypha clean` with the domain stopped after changing it.
  - The sequence number is assigned at dispatch, not at reserve. A reserved message
    that is never sent therefore leaves no gap. A message dropped for lack of a loan
    still takes its number, so the subscriber counts it as a loss.
  - Each loan costs one heap allocation inside iceoryx2 (`loan_slice` creates the
    chunk's shared state). hypha's staging and dispatch allocate nothing per message.
    A latched publisher keeps its last sample in the iceoryx2 service history, with
    no copy of its own.
- **History sequence order does not imply timestamp order.** Accepted history
  sequences are contiguous across wrap, so a queued cursor indexes the retained
  deque directly. Framework timestamps can descend after a rewind or when
  publishers interleave. Timestamp binary search is valid only while no descending
  adjacent pair remains in the history. Evicting those pairs restores the fast
  path; unordered histories keep the per-sample replay admission rules. A sample
  stamped exactly at a replay cut belongs to the next step, except on a trigger port of
  a triggered cycle, which admits it at its own step ("replay through T").
- **A wall clock that steps back inside one host turn makes a fresh trigger late.** The
  trigger schedule maps each pending stamp onto the monotonic clock by the turn's one
  wall reading, and a stamp from the future counts as sent now. When a port holds a
  sample stamped before a backward wall step and one stamped after it, the earlier one
  maps to now and the later-sent one before it, so the later-sent one is consumed as
  `Late` and counted although it is fresh. It happens only across a
  backward step of the wall clock.
- **One subscriber does not read several publishers in send order.** iceoryx2 gives each
  publisher its own queue, and a receive takes the first queue with data in the order of the
  subscriber's connection slots; a slot freed by a publisher that left is reused by the next
  one. Two samples of different publishers queued at one read come out in slot order, not in
  the order they were sent. A test must not infer causality from that order: compare what the
  samples carry, as `record_sync.rs` compares the send time a report answers (#92).
- **A test stops a supervisor as an operator does.** `hypha launch unit` passes a SIGTERM on
  to its node's process group and waits for it; a SIGKILL cannot be passed on, and on macOS
  the node keeps running with parent 1. A test ends a supervisor with SIGTERM and a wait, and
  sends SIGKILL only after the supervisor's own grace period (`play_restart.rs`, #92).
- **A managed drain validates and copies each frame once.** Pass the parsed frame
  into history while the transport sample is borrowed. Calling `sample.bytes()`
  again takes another transport shared-state lock. Reuse one host timestamp for
  the drain, refreshing it when a frame's send time exceeds it, before deciding
  whether to reject a future timestamp. Never skip integrity or sequence checks.
- **Cycle callback scratch belongs to the committed cycle.** Output limits are
  fixed at commit, and supervision copies reuse their storage each step. Reset
  every supervision update before invoking the callback, and apply updates only
  if the callback ran; a capture failure must not replay an earlier update.
- **Internal publication keeps its loan on the stack.** `publisher::send_frame_with`
  writes directly into a loan, then seals its header and CRC through the same
  send path as the ABI. A write failure releases the loan unsent. A refused loan
  still consumes its sequence, as the owning ABI loan path does. iceoryx2's own
  loan allocation remains; avoiding the ABI wrapper removes hypha's loan box.
- **A late subscriber gets a latched topic's last sample from iceoryx2 history.** A latched publisher uses `publisher_history_size = 1`, so a late subscriber receives the last sample through the service history. The transport sets `FLAG_LATCHED` in the header of every sample of a latched publisher; a subscriber drops a sample sent before it was created unless that flag is set (`crates/hypha_iceoryx2/src/latch.rs`). The check reads the header only. Keep the flag in the header that the CRC covers, and a changed history size must keep the rule.
- **`/clock` is one topic for every process on the bus.** A test or tool that
  publishes `/clock` moves the clock of every simulated-time cycle that is
  listening, in every process. `/clock_progress` is shared the same way. `.config/nextest.toml` therefore runs every test process through
  `scripts/private-domain.sh`, which gives it the domain
  `t_<nextest pid>_<slot>`: no two running tests share one. The
  next test in a slot reclaims only the dead CLI tool nodes of the one before;
  `test.sh` runs `hypha clean --domain t_` before the suite, which removes the
  test domains of the runs that ended and nothing else. The C++ test
  main (`crates/hypha-cxx/test/test_main.cpp`) gives each gtest shard process,
  which ctest runs in parallel, the domain `t_<pid>` and a
  `TMPDIR` of its own for the parameter files; `hypha_add_test()` gives any other
  ctest test `t_<ctest pid>_<n>`. The tools and peers a test spawns
  inherit the domain. A test that runs a process in another domain must pass the
  variable on itself and name it `t_<pid>_...` after a process of its run: every
  test domain lies under the root `t_`, so a test leaves nothing in the domain of a
  system running on the host, and the clean keeps a domain whose run still lives.
- **A node's zero clock is a placeholder, not a clock value.** The first sim node of
  a process starts simulated time at zero before any `/clock` arrives; a later sim
  node keeps the clock the process has. A replay cycle takes
  its phase from the first real value. If the placeholder counted, the first `/clock`
  sample would look like a forward jump of the whole recording, and every replay
  would run one step per period from zero to the recording's start: a hang. The placeholder is `enable_sim_time_placeholder`.
- **A replay progress report comes from the `WaitSet` that runs the replay cycles.**
  A node whose sim-time cycles sit on several `WaitSet`s can report a clock that a
  cycle on another `WaitSet` has not run yet. A callback that waits on simulated
  time stalls every replay until `hypha play --timeout` stops it. Limits in
  `replay-flow-control.md`, section 6.7.
- **`/clock_progress` limits are uniform domain-wide.** Service limits come from the
  domain's iceoryx2 `Config`, not from the creator's builder, so whichever process starts
  first (player, recorder, or application node) creates the service with the same capacity.
  `apps/hypha_tools/tests/progress_service_limits.rs` checks that a player, recorder, and
  node start in any order without start-order refusals.
- **Read the node registry with no input lock held.** The upstream hold reads the
  publisher ids of `/clock_progress` under the input's lock, then resolves their names
  with `node_names`, which walks every node. It does so after the lock is released
  (`waitset/upstream.rs`, `settle`). The input drain only try-locks, so an input whose
  lock is held looks busy, and its reports are not read until the lock is free. Keep
  the same order in new code that resolves names for a held cycle. A stalled hold
  re-resolves names at a bounded rate (every 250 ms) to drop killed nodes.
- **Judge a report by its send time first.** A clock value repeats in a later pass,
  so a report from an earlier pass can carry a clock that looks current. The
  player accepts a report only when its send time is at or after the chunk's
  floor, and a process that a backward seek restarted counts one only when it
  answers a `/clock` sample sent at or after the one it rejoined on
  (`node_progress::epoch`). Only then is the clock value compared.
- **Time going back restarts; nothing rewinds in place.** A timer or cycle that sees
  simulated time go back, and a node on simulated time that reads a `/clock` lower than
  its last one, whatever it holds and in the open loop too (#130), end the run with
  `ClockRewound`, `runNode` exits 75, and `hypha launch` starts the process again. Do not add rewind handling: the old
  in-place rewind (abandoned samples, re-reads, re-anchored fallbacks) was the main
  source of replay bugs. The restarted process must not open a port before every old
  participant has left, or it can read a latched sample of the old timeline: it waits
  in node creation, and the player sends its `/clock` only once `/clock_progress`
  has no publisher left (`replay-flow-control.md` §4.4).
- **A replay step with a lost input fails.** A step with a sequence gap or an
  evicted sample on a queued input fails its `WaitSet` run with `SequenceGap`. So does a
  `Latest` input under replay with a sequence gap, or with an evicted sample at a step
  that finds no retained sample before the deadline that no reader rejected. Until then
  a newer sample that no reader rejected, retained or held, supersedes the evicted one in
  data time; a rejection of it, by the step or by another reader of the history, ends
  that. Its signs are the `replay not faithful: N samples lost on <topic>` log line and
  `overrun_total`. In host time, a latest input neither fails nor counts a loss, because
  a value it did not read was superseded.
- **A replay report follows a drain of every replay input.** Under simulated time a
  `WaitSet` turn drains the inputs of each of its replay cycles after its steps and
  before it offers progress, on a turn that ran no step too, and a drain takes the
  queues of all publishers. Drain only at a due step, or only one queue's depth, and
  the player's next chunk pushes unread samples out of a queue: a transport gap that a
  selection supersedes fails the step (#222), and a loss before a publisher's first
  accepted frame is not counted at all (#247). Host time keeps its drain at the step.
- **Benchmarks: a green test run misses a slowdown.** A hot-path change (dispatch,
  publish, receive, cycle tick, wire codec) reports its benchmark numbers before and
  after, in the commit message. `cargo bench -p hypha_wire --bench codec` measures the
  primitive codec on a 25-byte header and a 64 KiB `float32[]` message;
  `cargo bench -p hypha_node --bench dispatch` one sample from publish through a
  `WaitSet` turn to the subscription callback; `cargo bench -p hypha_ffi --bench cycle`
  one cycle tick with one input and one output through the C ABI. The two transport
  benchmarks need `/dev/shm` and `/tmp/iceoryx2`. Compare medians run on the same idle
  machine; a difference within the reported spread is noise. They are not a CI gate:
  shared runners are too noisy.
- **Miri runs the Rust that needs no transport, and no node.** `scripts/miri.sh`
  runs the tests of `hypha_time`, `hypha_wire`, `hypha_msgdef`, `hypha_cycle`,
  `hypha_core`, `hypha_runtime`, `hypha_log`, `hypha_msgs` and the node-free tests of
  `hypha_node` under `cargo miri nextest` on the nightly of `docker/rust-nightly`, into
  `target-miri/`, with isolation off (the crates read the wall clock and write temp
  files). It finds undefined behaviour, data races and leaks in Rust only: no
  iceoryx2 shared memory (a node calls `getrlimit` and opens it), no C or C++, no
  spawned process, no `O_NONBLOCK` reopen of fd 2. Property tests run four cases
  under `cfg!(miri)`; a test Miri cannot run names the reason in
  `cfg_attr(miri, ignore = "...")`, and a test that spawns itself through
  `hypha_log`'s `in_own_process` runs in place under nextest. It takes seven to
  eleven minutes on 22 cores, bounded by the slowest tests (about seven minutes for
  `the_depth_and_expansion_limits_refuse_exactly_what_passes_them`); a new
  wall-clock bound or scale test needs the same `ignore`.
- **Fuzzing covers the parsers, not the transport.** `scripts/fuzz.sh [seconds]`
  runs each target of `fuzz/` (its own workspace and lock file, excluded from the
  root one) with `cargo fuzz`: the wire `Reader`, frame verification and the schema
  record, the definition parser and resolution, the configuration loader, two
  generated decoders with nested arrays, the subscriber's frame validation and the
  tools' dynamic walker. Each asserts its property test's invariants. A crash input
  lands in `fuzz/artifacts/<target>/`; minimise it with `cargo fuzz tmin` and keep it
  as a named test in its crate. Seeds are `fuzz/corpus/<target>/seed-*`; the grown
  corpus is ignored. Where an `/etc/ld.so.preload` library breaks AddressSanitizer
  at start-up (a segfault in `__asan_init`), run with `HYPHA_FUZZ_SANITIZER=none`.
  Its lock file is checked with the root `deny.toml`:
  `cargo deny --manifest-path fuzz/Cargo.toml check -A license-not-encountered`
  (the root allow-list names licences the fuzz tree does not use).
