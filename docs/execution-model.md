# Execution Model

How `hypha` runs your callbacks: the `WaitSet`, `run`, notifications,
timers, and polling.

SIGINT, SIGTERM, and node shutdown stop a running WaitSet at a dispatch boundary.
A callback that is running when the stop arrives can finish. The run then stops
before the next callback. The owner waits for the run to return before
destroying cycles or the node. The node owns its cycles and destroys
them with its `WaitSet`. The framework creates no cleanup thread.

This guide is for engineers who know ROS 1 (noetic). `hypha` uses the same
words as `roscpp` for several functions. Some of them behave differently.
Read section 2 before you port a node.

---

## 1. Who this is for

You know `ros::spin()`, `ros::spinOnce()`, `ros::AsyncSpinner` and
`ros::CallbackQueue`. You want to know what the equivalent parts of
`hypha` do, and where they differ.

Every rule in this document applies to the C++ SDK, the Rust SDK and the
C ABI. The C++ names are used in the examples, because most nodes are C++.

---

## 2. Quick map from ROS 1

| ROS 1 | `hypha` | The difference that matters |
|---|---|---|
| `ros::spin()` | `Node::run()` | It returns a `RunResult` you must check. |
| `ros::spinOnce()` | `Node::runFor(limit)` | There is no one-cycle call. Run for a bounded time instead. |
| `ros::AsyncSpinner(N)` | none | The framework creates no thread. You create and own every thread. |
| `ros::MultiThreadedSpinner` | none | Callbacks of one `WaitSet` never run at the same time. |
| `ros::CallbackQueue` | `WaitSet` | Registrations attach to a `WaitSet`. Samples wait in shared memory. |
| `ros::getGlobalCallbackQueue()` | `hypha_node_default_waitset` | Each node has one default `WaitSet`. |
| `nh.subscribe(topic, 10, cb)` | `node.subscribe<T>(hypha::Topic{topic}, cb)` | A callback subscription wakes on each sample; every publisher notifies. See section 7. |
| `ros::Rate` + `sleep()` | `Node::timer` | A timer is due against the monotonic clock on host time and against `/clock` on simulated time; the run waits until its deadline. |
| `ros::Timer` | `hypha::Timer` | Missed periods do not fire as a burst. See section 11. |
| `ros::ok()` | `hypha::ok()` | Same idea. |
| `ros::shutdown()` | Destroying the `Node` | The bus is released when the `Node` goes out of scope. |
| `const Msg::ConstPtr&` you may keep | typed callback: keep it; raw callback: copy it | See section 10.3. |
| roscore / master | none | Nodes find each other through shared memory. |

---

## 3. The framework creates no thread

This is the rule the whole execution model follows. Every callback runs on a
thread your application created and owns. Nothing progresses in the
background.

```text
  your thread
  ───────────

  run() ─┐
          │  1. build every native attachment, once, at run entry
          │
          │  2. wait on the native WaitSet
          │        wakes on a notification, a timer deadline,
          │        or an operation deadline
          │
          │  3. take a bounded batch from each ready attachment and
          │     call your callback for each accepted sample
          │
          └──4. check stop, progress operations, then go back to 2
```

| Part | What it is |
|---|---|
| **Node session** | One process-wide session. The first `hypha::Node` creates it. |
| **`WaitSet`** | A set of registrations and the thread that runs them. A node has one `WaitSet`, its default one (`hypha_node_default_waitset`); there is no call that creates another. |
| **Attachment** | One registered subscription, timer, cycle, or housekeeping job. It has an id. |
| **Callback** | Your function. It runs on the thread that called `run()`. |

Consequences you must plan for:

- **Housekeeping only progresses while a `WaitSet` runs.** Latched replay and
  schema answers are attachments. A node that never runs one answers no
  schema query and wakes no late subscriber of a latched topic. A latched
  topic's last sample reaches a new subscriber when the node's `WaitSet` hears
  the domain graph event that a subscriber creation fires (`hypha/hk/graph/v1`),
  or when the publisher next sends, and only that subscriber receives it. A publisher that carries a
  message definition invites those queries, so a process that publishes such
  a topic and never runs the executor reports the usage error once per
  publisher as one log line. Readers see the topic as undescribed until the
  publishing loop runs the executor.
- **Attachments freeze during a run.** Create every subscription and timer
  before you call `run()`. A subscription created during a run is refused
  with `InvalidState`.
- **One `WaitSet` has one thread.** Two `WaitSet`s may run at the same time on
  two threads you created. Their callbacks then run concurrently with each
  other, so shared state needs your own synchronisation.

---

## 4. Running: `run`, `runFor`, `requestStop`

```cpp
// Blocks until something stops it.
hypha::Node node("my_node");
const hypha::RunResult result = node.run();
if (!result.ok()) {
    // The call itself can fail, and a call that completed can still
    // report that the run failed. RunResult carries both.
    return 1;
}
```

`Node::run()` runs the node's default `WaitSet` on the calling thread. It returns
when:

- your application called `requestStop()`, or
- a callback asked for a stop, or
- the run failed.

`runFor(limit)` is the same run with a deadline. When the deadline elapses
the run stops the same way `requestStop()` stops it, and reports a normal
stop. A run always takes its first dispatch turn, so it serves the timers that
are due and the samples already queued even when a loaded host spends the
whole limit starting the run. Use it in tests and short tools. The limit is a
span of real time. A limit of zero or less is refused with `InvalidArgument` and runs nothing. A
limit that you compute from framework time can be negative after playback
jumps, and a rewind makes it longer, so compute a limit from `steadyNow()`.

`requestStop()` is the one operation another thread may call on a running
`WaitSet`. It is idempotent while running. It is refused with `InvalidState`
while stopped, so a stop cannot accidentally end the next run.

`RunResult` is `[[nodiscard]]`. Ignoring it is a compiler diagnostic, because
a run that failed and a run that stopped normally are different outcomes.

### 4.1 There is no `spinOnce()`

A public one-cycle call is not provided. The native `WaitSet` and the guards
it borrows live on one run stack, and keeping them alive across separate
calls needs a lifetime design this release does not have.

Replace a `spinOnce()` loop with one of these:

| You wanted | Do this |
|---|---|
| pump callbacks for a moment | `runFor(limit)` |
| run until told to stop | `run()`, and `requestStop()` from elsewhere |
| do periodic work | attach a timer and let it run inside `run()` |
| process several inputs periodically | start a `CycleNode` or a `Cycle` and let `run()` serve it (§6.1) |

### 4.2 Restart

A `WaitSet` may run again after it stops. On the next run:

- Timers count the stopped interval as missed periods in their next call. There
  are no catch-up callbacks for it.
- A cycle's input history does **not** restart. The first image after resume
  judges retained samples against host time, so a value can already be stale.
- Registrations, positive schema caches, and sequence state are kept.

---

## 5. Bounded dispatch

Every turn is bounded, so one busy topic cannot starve a timer, a stop
request, or a timer.

| Limit | Default | What happens at the limit |
|---|---|---|
| Registrations per `WaitSet` | 128 | Attach returns `ResourceLimit` |
| Samples per attachment per turn | 8 | The attachment stays ready and the rotation moves on |
| Work items before a readiness recheck | 64 | The loop rechecks the native `WaitSet` |
| Slow callback warning | 10 ms of host time | Counter, plus a warning at most once per second. Simulated time never makes a callback slow |

These are engineering defaults, not safety-derived limits. Measure them on
your target. A limit is refused, never clamped: a zero limit, or a batch size
above the dispatch budget, returns `InvalidArgument`.

The dispatcher rotates over ready attachments with a cursor. It does not
restart each scan at the first registration. Rejected frames count against
the batch, so a flood of corrupt traffic is bounded like valid traffic is.

---

## 5.1 Asking a question whose answer arrives later

A callback runs on the thread that has to deliver every other answer. A call
that waits for a round trip therefore stops the loop that must complete it.
Blocking schema lookups refuse a call made from a dispatcher callback and
return `InvalidState`. There is no asynchronous submission in the C ABI.

**Resolve before the run.** Call `hypha_schema_lookup` (or, in the Rust SDK,
`BusNode::resolve_schema_hash`) outside a callback, before the run. A later
`hypha_schema_lookup` for the same type returns the definition from inside a
callback, with no round trip. A definition that is not held yet returns
`InvalidState` inside a callback.

---

## 6. Freshness and periodic cycles

These measure different things. One duration never serves two of them.

| Meaning | Reference event | How it is configured |
|---|---|---|
| Cycle input freshness (staleness) | the newest sample's send time, at each capture | `silence_ms` of the port (section 6.4) |
| Input stagnation | the last change of the alive counter, in framework time | `stagnation_ms` of a port that declares an alive counter (section 6.4) |
| Source age | the measurement timestamp | not validated in this release |

Managed sources keep bounded shared history below the C ABI. A cycle captures
the samples visible at its timer turn and judges its bindings together at one
host-monotonic instant (the replay cut under simulated time). Each binding can use
a different silence limit; without one nothing is rejected. The boundary is strictly
greater than, so a sample exactly at its limit remains usable. A source that is not wired or has
never received a sample reports `NeverArrived`; a retained value that exceeds
its binding limit reports `Stale`.

### 6.1 Cycle execution

A cycle is a framework-time periodic registration on the node's default
`WaitSet`. The application keeps mutable state in the members of its
`hypha::CycleNode`, or in the object that a named step functor points to. Its
`Latest`, `Arrivals` and `Output` ports hold the frozen typed capture for each
call:

```cpp
void step(const hypha::CycleTime& time);
```

A `CycleNode` takes its rate from `rate_hz` under `[cycle]` in the parameter file,
or from `setRate()` in `setup()`, and a trigger from `setTrigger(port)`. The
cycle has the node's name. A composed `hypha::Cycle` has its own name and uses
`setRate()`.

The executor runs due timers, including cycles and intervals, before dispatching
new ingress. A cycle capture therefore uses data already ingested by an earlier
turn; samples waiting on the same wake become visible to a later image. Due
cycles run in registration order. This timer-first rule keeps continuous
ingress from postponing timer work, while a slow earlier callback can still
delay later cycles.

`CycleTime` reports the framework time of the step (`now`), the same time as a ROS
header stamp (`stamp`), the time since the previous step started (`dt`) and the
missed periods (`missed`). The first step has no `dt`. `hypha::now()` returns
`time.now` for as long as the step runs. On host time `dt` is the difference of
two monotonic readings, so a step of the system clock never makes it negative;
`now` and `stamp` follow the system clock. A forward gap in host time is reported
through `dt` and `missed` without a catch-up burst. On simulated time a clock that
goes back ends the run with `ClockRewound` and restarts the process, and a cycle
replays instead: it runs every expired deadline, each with its own time and inputs
(section 11.1).
Simulation pauses stop cycles, while ingress and housekeeping continue as the
`WaitSet` runs. In host time, freshness still uses host time, so the first
post-resume image can be stale.

Latest inputs select the newest accepted retained sample. Queued inputs have
an independent cursor per binding, examine up to their arrivals per step within
the cycle budget (section 6.5), and preserve any remaining backlog for a later
iteration. A batch can be
empty while its stream is healthy; the image separately reports its verdict:
`Usable` while the newest sample is within the silence limit, `Stale` after it.
Test `samples().empty()` for "nothing new".
Captured ranges are consumed before application processing, so a step failure
does not replay them. State changes and earlier successful output sends also
remain committed if a later output fails.

The C++ facade decodes samples into the ports before calling the step. A `Latest`
port decodes a value once, when it becomes the port's value; `*port` and `port->field`
read it. A sample that does not decode fails nothing: the port reports it to the
library, which counts it, logs it at most once per 10 s per input, and judges the input
without it; a `Latest` port keeps the value it had (readable through `lastKnown()`; `*port`
reads only a usable value) and an `Arrivals` port delivers the
other samples in order. Each output `send(message)` serializes the message into a transport loan that the
step holds until it returns. Each output has a positive per-step send limit, which
defaults to one, and its publisher holds that many loans. The library checks each
message against the limit when it is reserved. After the step succeeds, it sends the
held loans in send-call order across outputs. If the step fails, the loans are
released unsent and nothing from that step is published. Exceeding a send limit
fails the iteration with `ResourceLimit` and publishes nothing from that step.
The facade owns no scheduler, history, freshness
rule, or cursor. Use `Cycle::health()` and `introspect()` for
runtime diagnostics and configuration; inspection does not advance capture.
Cycles create no thread. The node's `run()` and `runFor()` serve them on the
calling thread.

### 6.2 Cycle timing limits

- The timers-first rule adds a phase delay between the arrival of a sample and
  the step that reads it. A cycle drains its bound input queues immediately
  before capture; the delay depends on the work limits of section 5 and the
  other registrations. Measure it on the target. Do not assume a fixed bound.
- A cycle has no deadline guarantee. Due cycles run in registration order. A
  slow step delays the cycles that follow it.
- `CycleTime::now` is read before the SDK decodes the image. It is not
  the time at which the algorithm runs. Measure elapsed real time with `hypha::steadyNow()`. The duration of a callback includes the
  decode, the step and the encode. The capture time stays separate: the library
  judges the samples once, at capture, and never again at callback entry.
- `CycleTime::missed` counts the periods that passed without a step since the
  previous step: a per-step delta, 0 when the step is on time, never a running
  total (`Cycle::health()` keeps the total). A host timer starts its periods at
  the first run turn, so the time between the attach and the run is not missed.
  A triggered step reports 0. A rewind adds no missed period. A forward gap in host
  time is measured on the monotonic clock, and cannot be told from scheduling
  delay, so the count shows it and no catch-up burst follows. A step of the
  system clock is not a gap: it changes no count. On simulated time a replay step reports 0 missed
  periods: a forward jump of `/clock` runs one step for each period it covers.
  A cycle timer runs on framework time, so no tick is due while simulated time stays at
  zero. Any other cycle committed before the first simulated time arrives reports a large
  count at its first tick; a node that must not see it starts its cycle after simulated
  time arrives.
- A managed source keeps an owned, immutable copy of each payload below the C
  ABI, and releases the transport loan at once. A queued history is therefore
  not zero-copy end to end. The history limit counts samples, not bytes, and it
  does not bound the objects that the SDK decodes from them. Never reinterpret a
  serialized payload as a C++ message that holds vectors or strings.
- Managed cycle inputs do not wake the `WaitSet` between ticks, except the trigger
  of a triggered cycle (section 6.6). Each tick drains
  every bound source's transport queue into its bounded history before capture.
  Cycles sharing a source also share that history; the first tick drains it and
  later ticks see the retained samples. Size a source's transport queue for at
  least one period of arrivals. A queue overflow drops old samples. For a queued
  binding, a forward per-publisher sequence gap contributes to its `overrun_total`
  after the next surviving sample arrives. A latest binding does not count it.
- On simulated time the framework `/clock` subscriber is notification-driven:
  every `/clock` publisher notifies, so the node wakes when time moves. A 50 ms
  audit also samples `/clock`, for a lost wake.
- **Data time.** Every frame carries a framework send time. A publisher stamps
  it with the framework clock, which is simulated time on a sim-time node. A
  replayer stamps it with the recorded time instead. `hypha play` does this for
  every frame it publishes, `/clock` included. Its replay ordering rule is that
  it never publishes `/clock = T` until every message stamped before `T` has
  gone out, so a node that reads `/clock = T` already has that data queued. It
  also waits until every participant has reported, on `/clock_progress`, a
  `clock_sent_nanos` at or after the host send floor taken just before that
  `/clock` sample went out, and a `clock_nanos` at or beyond the sample's value.
  A participant that holds a deadline for an upstream reports less, and that
  does not acknowledge the sample yet.
  On simulated time a cycle judges its inputs in this data time (section
  11.1). In host time, a cycle judges by host transport time, as before.
- hypha has no RPC. A request and its reply are two topics (cpp-api.md §8), so a
  step sends one on an `Output` and reads the other on an `Arrivals` input.

### 6.3 Cycle failure

| Condition | Result |
|---|---|
| An input is missing or stale | The step runs with the explicit verdict. The application chooses its response. The framework never supplies a default value. |
| An output is not marked | Nothing is published for it. |
| The step throws | The iteration fails with `Panic`, or `ResourceLimit` for `std::bad_alloc`. No output is sent. |
| A selected payload passes the CRC and the schema check but does not deserialize | The C++ port fails the iteration with `InvalidFrame` before the step. The consumed ranges stay consumed. The step does not run, so no application state changes. |
| An emission is refused, or a payload is larger than its publisher allows | The library checks the message at reserve time and validates every staged message before it sends the first one. The iteration fails and no output is sent. |
| The transport has no free loan for a message (`PoolExhausted`) | The message is dropped at reserve, and the step continues. The run fails at dispatch unless the output is `diagnostic` (C ABI `DropPoolExhausted`), which drops and counts this refusal only (`output_pool_drops`). A dropped message takes its sequence number, so the subscriber counts the loss. |
| The transport reports `Disconnected` or `Backend`, or a send reports `InvalidArgument` or `InvalidState` | The run fails. The recoverability of the fault is not known. |

- The owning `WaitSet` is one failure domain. A terminal failure stops the
  remaining outputs and the run, other cycles of the same `WaitSet` included.
  Run independent work on separate `WaitSet` objects.
- Sends are not atomic. A later failure cannot undo an earlier send or a state
  change.
- A failed step never replays its captured ranges.
- The framework never calls `exit()` and never invents a safe output. It cannot
  promise that an actuator stop command reaches the bus after a failure. A
  system watchdog covers that case.
- A failed send of the data differs from a failed notification after a
  successful send. The second is a diagnostic degradation.

### 6.4 Input supervision

Every enabled input that counts for health has a silence limit, unless its code tags
it `unsupervised`; the decision is made below the C ABI (`hypha_runtime::port_limits`).
A port with a silence limit checks that its input is current. Two checks exist:
staleness (the silence limit) and stagnation. One limit, `silence_ms`, decides both
whether a value may be used (the `Stale` verdict) and whether the port is stale
(`stale()`); `age_rejections_total` counts the samples it refused. Stagnation is
`stagnant()`.
The cycle runtime does the detection below the C ABI, in `hypha_cycle`. The
application decides what a stale or stagnant input means.

1. The runtime evaluates every port with a limit once per iteration, at the capture
   and before the step. The resolution of every limit is therefore one cycle period.
2. Staleness uses the capture clock: host monotonic time against each sample's
   transport send time, or the replay cut against its framework send time under
   simulated time. Framework time in host mode is the wall clock, which can step,
   so it plays no part. Host scheduling, the timers and the fallbacks, runs on the
   monotonic clock as well, so a step of the wall clock neither delays nor advances
   them.
3. A port is stale when its newest sample was sent more than `silence_ms` before the
   capture. A sample that arrives late, already older than the limit, is refused and
   does not end the staleness: only a fresh sample does. A queued port is judged by
   its newest sample too. An empty step is `Usable` while that sample is young, and
   `Stale`, with `stale()` true, after it. A publisher slower than the cycle gives
   `Usable` steps with no new sample; a publisher that stopped gives `Stale` steps.
4. A port that has received nothing counts from its first evaluation, so it is not
   stale before `silence_ms` has passed. A sample stamped after the capture clock has
   not aged.
5. Stagnation needs a life sign: an integer in the payload that the original source
   advances, declared with `setAlive()`. It is for data that reaches hypha through a
   relay (a device driver, the ROS bridge, a gateway). There the frame sequence
   number proves only that the relay ran: it advances when the relay publishes the
   last payload again. A topic that a hypha cycle publishes needs no life sign: its
   frame sequence advances only when the sender's step ran, and `silence_ms` reports
   a sender that stopped. A payload that does not change is not stagnant; a periodic
   sender may publish the same value for ever. The library never decodes a payload,
   so the C++ port reads the life sign and gives it to the runtime. The runtime
   applies the value after the step returns, with the time of that iteration.

   The checks of a frame are separate, and each covers its own faults: the CRC covers
   corruption; the sequence number covers loss, repetition and reordering; the send
   time covers delay and silence; the life sign covers a relayed source that stopped
   while its relay still sends.
6. The stagnation timer starts at the first alive value the runtime sees. Any
   change of the value restarts it, a wrap included. Stagnation is checked only
   while the port is not stale, so a port is never stale and stagnant at once.
   A port that has accepted no sample is stale, never stagnant. The timer runs on
   framework time, the step's `CycleTime::now`: the wall clock in host mode, simulated
   time in a replay. It is not the capture clock of rule 2, so a backward step of the
   host wall clock holds a stagnation back for as long as the step was; a clock that
   went back has not passed the limit, and a forward step brings the fault forward.
7. Disabled and ignored are two facts. A disabled port (an `Optional` port whose
   `topic` is empty or unset) reads no topic and is neither stale nor stagnant.
   Ignoring (`setIgnored(true)`) hides the condition: `stale()` and `stagnant()`
   are false while a port is ignored, and `ignored()` shows the override.
8. The runtime logs one line for each change of the stale or stagnant state after the
   step returns:

The log, module `hypha`, shows each change of a wired input's judgement once, at the end
of the step that captured it; there is no periodic summary, and the initial wait logs
nothing. An input without a silence limit never logs `never arrived`. Two ports on one
topic log under their own names. `<in>` is `cycle '<cycle>' input '<port>' (<topic>)`,
or `cycle '<cycle>' input (<topic>)` without a table; an advisory input's line ends in
` (advisory)`. Ages and limits are whole milliseconds.

| Change | Level | Line |
|---|---|---|
| first usable sample | INFO | `<in> is usable` |
| usable → stale | WARN | `<in> is stale: newest sample is <age> ms old, silence_ms = <limit>` |
| no sample for the whole limit since start | WARN | `<in> never arrived: no sample for <age> ms, silence_ms = <limit>` |
| alive value stopped changing | WARN | `<in> is stagnant: alive value unchanged for <t> ms, stagnation_ms = <limit>` |
| back to usable | INFO | `<in> is usable again` |
| `setIgnored(true)` while faulted | WARN | `<in> fault is ignored` |
| `setIgnored(false)` while faulted | WARN | `<in> fault counts again` |

### 6.5 Queued work: budget, overflow and loss counters

A queued input (`Arrivals`) examines at most its arrivals per step. A cycle budget
caps all queued inputs together. In each step, the budget is shared among the
queued inputs that have pending samples. An idle input takes none, and a `Latest`
input takes none. Without a budget, no cycle-wide limit applies.

A queued input has one overflow behaviour, and no option selects another: the
pending samples beyond its share are the oldest, so it discards them, examines
the newest, and counts each discard. Only the trigger of a triggered cycle keeps
its pending samples for later steps (section 6.6).

Each cause of loss has its own counter:

1. History overwrite: the history dropped a sample before the input read it.
2. Age rejection: the input read the sample, and the age rule refused it.
3. Overflow discard: the input discarded an old sample beyond its share on purpose.

The transport's cumulative missing-sequence count is read atomically, so health
queries do not take the source-history or sequence-validator locks.

Section 5.4 of the C++ API page gives the fields, the TOML keys and the rules.

### 6.6 Triggered cycles

A cycle runs either on a rate or on a trigger, never both. The trigger is one of
the cycle's `Arrivals` inputs (`Cycle::setTrigger`, C ABI `hypha_cycle_set_trigger`
on a cycle created with period 0). A cycle with both a rate and a trigger, with
neither, or whose trigger has no positive silence limit is refused at start with
`InvalidArgument`, and one log line names the cycle and the reason.

1. **One step per trigger sample**, oldest first. The trigger port holds exactly
   that sample in the step. A turn runs every trigger sample that is due; the
   trigger subscriber's queue depth bounds them, and a queue overflow is a counted
   transport loss as for any input. The arrivals limit, the cycle budget and the
   overflow discard do not apply to the trigger, which keeps each sample for its
   own step; they apply to the other `Arrivals` inputs.
2. **What the step sees.** `CycleTime::now` is the trigger's framework send
   time T. Under simulated time every other input is cut strictly before T, as a
   grid step is cut at its deadline, and the outputs carry T as their framework
   send stamp. In host time the other inputs are captured as a host-time rate step
   captures them.
3. **Fallback.** When no trigger sample was sent for the trigger's silence limit
   (`silence_ms`), counted from the last step, one step
   runs without a trigger sample, with the trigger stale. It repeats
   every silence limit until a trigger arrives. Before the first trigger it counts
   from the cycle's first evaluation. A trigger stamped exactly at the fallback time
   runs instead of the fallback. In host time a trigger stamped before the step that
   ran last, or already older than the limit on the wall clock, runs no step: it is
   consumed and counted in `age_rejections_total`, so steps run in send order.
   The send stamp is aged once, when the trigger is received, and its silence then
   runs on the monotonic clock: a later step of the wall clock moves no fallback. The
   fallbacks that a late turn missed run as one step at the latest due time, as a rate
   cycle counts missed periods instead of a burst. Under
   simulated time each fallback runs, at the last trigger stamp plus multiples of
   the limit, so replays repeat them. A fallback at F waits until every upstream
   has reported at least F, so no trigger stamped F can still come.
4. **Wake-up.** The trigger wakes the `WaitSet` when its publisher notifies, which
   every publisher does. In host time the listener is also audited, so a lost
   notification is read within one audit period, and the `WaitSet` wakes at the
   cycle's next fallback deadline, so a fallback runs on its deadline. A wildcard
   trigger has no listener and is polled every millisecond, as a wildcard subscription
   is. Under simulated time the wake follows the rule of a cycle timer (section 11.1).
5. **Simulated time.** A step at T runs when the node's clock is at least T and
   every upstream of the cycle has reported at least T − 1 ns on `/clock_progress`,
   the rule a grid replay cycle follows. A held step waits for as long as its
   upstream does not report; the player's `--timeout` is the bound. The trigger stamps must
   ascend in arrival order: a trigger stamped before the step that ran last, from a
   publisher that does not report progress or from several publishers whose samples
   arrive out of order, fails the run with `SequenceGap`, and the log names both times.
6. **Progress and loops.** A triggered cycle cannot know when its next trigger
   comes. It reports at most the lowest clock its upstreams reported, and stays
   below its earliest pending trigger or fallback. A feedback loop made only of
   triggered cycles has no delay element: it holds for good, until the player's
   `--timeout` or a stop ends it. A loop needs a delay element, a rate cycle or a clock driver, as an
   algebraic loop does in any block-diagram tool.

### 6.7 The TF input

A cycle can hold a TF input (C++ `hypha::Transforms`, C ABI
`hypha_cycle_bind_transforms`): a buffer of the transforms on `/tf` and `/tf_static`,
below the C ABI, that the step looks up with explicit results and never waits on.

1. **Filled before the step, under the step's cut.** The input is two queued, optional
   bindings. Before the step every captured `TFMessage` goes into the buffer, those of
   `/tf_static` as static. Under simulated time that is every sample sent strictly
   before the step's time, so a replay sees the same buffer on every run; in host time
   every retained sample. A sample that does not decode fails the step with
   `InvalidFrame`. A transform the buffer refuses (a rotation that is not a unit
   quaternion, a non-finite value, a second parent for a frame, a cycle, a frame both
   static and dynamic, a header stamp whose `nsecs` reaches one second) is left out and
   logged, at most once a second per TF input with the count.
2. **Lookups.** A lookup names a target, a source and a time, or asks for the latest
   time common to every transform on the chain (C ABI `latest`, C++ `lookupLatest`).
   Time 0 is a time like any other, since simulated time may start there. Unknown frame, frames not connected, time
   before the buffer and time after it are distinct results; an empty name is refused
   and one leading `/` is dropped. A policy such as "at the stamp, else the newest" is
   the application's (MRS keeps it in its `Transformer`).
3. **A clock that goes back.** The cycle ends its run with `ClockRewound` and the
   process restarts (section 11.1), so the buffer starts empty in the new process.
   This holds for simulated time only. A host step of the wall clock does not end
   the run: transforms keep their wall stamps. A dynamic transform stamped more than
   the buffer length before its frame's newest one, as after a backward step of the
   wall clock, drops that frame's older transforms, so a lookup answers from the new
   timeline at once; the frame's descendants and static transforms keep theirs. The
   node logs it once per frame and step: `hypha: cycle '<cycle>' TF input: '<child>'
   in '<parent>' is stamped <d> s before its newest transform, more than the <c> s
   buffer: its older transforms are dropped`.
4. **Static transforms.** `/tf_static` is read with depth 32, one latched sample for
   each publisher a topic can have, so a cycle that starts late sees every static
   publisher.
5. **Losses.** A `/tf` burst larger than the queue is a counted transport loss of the
   binding and, under simulated time, fails the replay step with `SequenceGap`; the
   lost samples are never interpolated over silently. A replay step takes at most 32
   samples of each topic; one with more pending, sent before its time, fails with
   `SequenceGap` and logs the count instead of dropping the oldest.
6. **Loops.** The TF input makes every `/tf` publisher an upstream of the cycle. A
   triggered cycle that publishes the TF a second triggered cycle reads, while the
   second feeds the first, is the loop of §6.6 item 6.

A plain node with no cycle uses the same buffer through a transform buffer (C ABI
`hypha_transform_buffer_create`; [interface-specification.md](interface-specification.md#transform-buffer-of-a-plain-node)).
The same code ingests and looks up, so the results above hold, but there is no step:
the run of the node's default `WaitSet` ingests every received `TFMessage` on the run
thread as it arrives, under simulated time and in host time alike, with no cut. A
lookup may come from any thread at any time; one lock guards the buffer for one
message or one lookup, and no lookup waits on the transport. A message that does not
decode is counted and logged with the refused transforms and fails nothing, and a
`/tf` loss is the subscriber's missing-sequence count, not a failed run.

---

## 7. Notifications: every publisher notifies, the reader decides

In ROS 1, a publisher and a subscriber connect over TCP. The subscriber
wakes when data arrives. You configure nothing.

In `hypha`, the data path is shared memory, which does not wake anybody. Every
publisher therefore also signals its topic's event service after each send.
Whether that signal wakes anything is decided by **how the topic is read**, not by
a setting on either end:

| Reader | Wakes on a sample? |
|---|---|
| Callback subscription (`node.subscribe<T>(topic, cb)`, `subscribeRaw`, a Rust `WaitSet::attach_subscription`) | Yes. The `WaitSet` opens a listener for it, so `run()` wakes at once. |
| Triggered cycle's trigger input | Yes. The cycle's `WaitSet` listens on the trigger topic. |
| Cycle input of a rate cycle (scan → process → output) | No. It is drained at the cycle's tick; it has no listener, so its publishers signal nobody. |
| Wildcard pattern | No event service spans a pattern, so it is polled (section 8). |

```cpp
// Publisher: always notifies.
auto out = node.advertise<sensor_msgs::Imu>(hypha::Topic{"/imu"});

// Callback subscriber: wakes the run per sample.
auto in = node.subscribe<sensor_msgs::Imu>(hypha::Topic{"/imu"}, onImu);
```

A publisher with no listening reader checks its topic's listener list and sends
nothing; the cost is one small event service per topic. A rate cycle's
schedule is never disturbed by a wake.

### 7.1 How a notification reaches your callback

The `WaitSet` opens one listener per callback subscription when it is attached,
attaches it once at run entry and never rebuilds it during the run. A subscriber
itself owns no listener, so a subscriber read on a schedule costs none.

When the listener fires, the run loop marks that subscription ready, consumes
the pending notifications so an unread event cannot spin the loop, and takes a
bounded batch from it. A notification is a wake hint, not a message count: the
data queue stays authoritative. The 50 ms audit also reads every event-driven
queue, so a sample whose wake was lost still arrives within one audit.

There is no relay thread and no forwarding step. The thread that called
`run()` is the thread that waits on the listener.

---

## 8. Wildcard subscriptions

A wildcard pattern follows several topics and has no single event service, so
its callback subscription is read on a 1 ms cadence: about 1000 wake-ups a
second whether or not a sample arrived, and up to 1 ms of latency. An idle
callback subscription on a literal topic wakes the run about 20 times a second,
for the audit. Subscribe to literal topics in nodes that must stay idle.

A `WaitSet` with no event-driven registration (no callback subscription, no
triggered cycle) has no audit wake of its own: a node whose work is rate cycles
wakes at its cycle rates and timers only, and its framework listeners are audited
on those wakes. Cycle inputs open no listener, so a published sample never wakes a
rate cycle. Without any timer due, the wait lasts at most 1 s.

---

## 9. Waiting

The C++ SDK has no blocking read. The `WaitSet` is the wait.

---

## 10. Callback rules

### 10.1 Which thread runs it

The thread that called `run()` or `runFor()` runs your callback. The
framework never creates a thread of its own.

Callbacks of one `WaitSet` never run at the same time as one another. Two
`WaitSet`s that you run on two threads do run concurrently, so state shared
between them needs your own synchronisation.

### 10.2 What a callback may do

A callback may publish, read a subscriber that no `WaitSet` drives, read the
schema caches, and ask for a stop.

A callback may **not** add to the topology. An attach is refused with
`InvalidState` while the `WaitSet` runs, because the native attachments are
frozen for the whole run. Create every endpoint before you run. A callback may
detach a registration of its own `WaitSet` (a destroyed timer or subscription):
it is never dispatched again, and it is removed and its context released when the
run returns. A detach from another thread during a run is refused.

A callback may **not** block on a reply. A request and its reply are two topics
(cpp-api.md §8); the reply arrives as a later sample.

**Detach a registration before you destroy the object its callback context
points to.** The registration shares the subscriber, so the endpoint itself
stays alive until the detach; the context is only borrowed, and a callback on a
freed context is the caller's use-after-free. A detach from a callback releases the
context only when the run returns: keep the context valid until its `release` runs.

### 10.2.1 What a callback returns

A callback tells the run loop what to do next:

| Action | Effect |
|---|---|
| continue | Keep running. This is the default. |
| continue, with a nonzero status | Keep running. The node health counts the failure. |
| stop | End the run and report a normal stop. |
| fail | End the run and report the status the callback chose. |

### 10.3 How long the data lives

| Callback type | What you receive | May you keep it? |
|---|---|---|
| Typed (`Subscriber<T>`) | a decoded message in a shared pointer | **Yes.** It is an owned copy. |
| Raw (`subscribeRaw`) | `RawSample.bytes`, a span over shared memory | **No.** Copy the bytes first. |

The raw span is valid only for the duration of the call. This is different
from ROS 1, where a `ConstPtr` stays valid after the callback returns.

### 10.4 Exceptions

A subscription or timer callback that throws does not propagate the
exception. The C++ SDK catches it at the C ABI boundary and returns the
continue action with the status `Panic`, so the run counts the failure and
keeps running. The SDK logs the first failure of each callback, then at most
one line every ten seconds, so a callback that throws on every sample does not
flood the log. Your node keeps running.

`hypha::metric("node.callback_failures")` reads the count. The `node.*` metrics report every callback the
executor called, every failure, the status of the last failure, and the
framework time at which the last dispatch sweep ended. A health report of your
own can therefore show a callback that fails on every call.

Handle your own errors. Do not rely on an exception reaching `main`.

### 10.5 A frame that does not parse

A sample that is not an `hypha` frame is still delivered. `payload` is
null, and only the topic is filled in. A node that counts foreign traffic
therefore still sees it.

---

## 11. Timers

`Node::timer(period, callback)` returns a `hypha::Timer` and throws
`std::runtime_error` when the timer is refused.
It attaches a timer to the node's default `WaitSet`. Rust `attach_timer`
attaches one to a `WaitSet` you created; the C++ SDK has no such call. The
`WaitSet` owns the timer. On host time it is due against the monotonic clock,
and the run's wait ends at its deadline; on simulated time it follows `/clock`. There is no timer thread and no timer queue of the SDK's own.

A timer is the one periodic registration: the host-time interval
(`HYPHA_ATTACH_INTERVAL`) is gone, and the C++ SDK has no host-time timer. A
host timer is due on the monotonic clock: the `WaitSet` sleeps until its next deadline,
and a step of the system clock neither delays nor advances it.

A timer callback runs on the same thread as your subscription callbacks,
and never at the same time as one of them.

**A timer does not fire a burst.** If several periods elapsed since the
last sweep, the callback still runs **once** in that sweep. The framework
counts the periods you missed since its previous call instead of replaying
them; the next on-time call reports 0 again. Read the count when you must
detect an overrun. This is the rule for a plain timer and for
a cycle in host time. A cycle on simulated time replays its deadlines
instead (section 11.1).

This matters when a slow callback blocks the sweep. In ROS 1 you might see
a burst of timer callbacks afterwards. Here you see one call and a missed
count.

**A timer does not follow framework time backwards.** On simulated time a
`timer` timer is due against `/clock` time. When `/clock` moves back, the
timer fires nothing and its run ends with `ClockRewound`; `runNode` restarts the
process (section 11.1). A step of the host wall clock is not a rewind, and a host
timer never sees one: it is due on the monotonic clock.

### 11.1 Cycles on simulated time

A simulator can drive `/clock` itself: a rate cycle marked with `Cycle::setClockDriver`
steps only after every participant acknowledged the previous value, as `hypha play` waits
for its chunks. `replay-flow-control.md` §4.5 gives its rules.

A cycle on simulated time is a replay cycle. It runs every period in data
time, so a result does not depend on how fast the clock is moved or on the
host:

- **Deadlines are on the grid.** A deadline is a whole multiple of the cycle's
  period in simulated time. The first is the first multiple strictly after the
  first clock value that the cycle sees, so its phase does not depend on when
  the node started. A clock value is a `/clock` sample, a `setSimTime` or
  `advanceSimTime`, or an `enableSimTime` with a start. The zero that a node
  starts with is a placeholder, not a value, and does not set the phase.
- **One step per expired deadline.** When the clock moves past deadlines
  `D1 < D2 < … ≤ now`, the inputs are drained once, and the cycle then runs one
  step for each deadline. Each step has `now = Di`,
  `missed = 0`, and its outputs carry `Di` as their framework send
  stamp. The header stamp of each step is `Di` too.
- **Each step sees its deadline's data.** A step takes only the samples whose
  framework send time is before `Di`. A queued input stops its cursor at that
  cut, and a latest input takes the newest sample before it. A sample sent at
  `Di` belongs to the next step. A step therefore never sees data that the
  replay has not yet reached.
- **Ages are data-time ages.** A sample is judged by `Di − sent`, and supervision
  (silence and stagnation) is measured against `Di`. A sample 600 ms old at the
  deadline is stale under a 500 ms limit, whatever the host speed.
- **Wake-ups follow the clock.** A node on simulated time wakes its `WaitSet` on
  each `/clock` event, and on a 50 ms audit for a publisher that sends no
  notification. Its timers do not poll at 1 kHz. A run at 50 Hz with idle inputs
  takes about 50 turns a second, plus the audit.
- **Progress is reported per `/clock` sample.** After each turn of a `WaitSet`
  that holds a replay cycle or a triggered cycle (section 6.6), it publishes `hypha_msgs/ReplayProgress` on
  `/clock_progress`: the node's name, a clock value and the host send time of the
  sample it answers. The clock value is the sample's value, lowered to one
  nanosecond below the earliest deadline that has not run. A report is sent for
  each new sample, even when the clock value repeats. The player waits for these
  reports (`replay-flow-control.md`, sections 4.2 and 4.3).
- **A held deadline waits for the upstream nodes.** A cycle reads from the nodes
  that publish its inputs and `/clock_progress`, except itself. Before it runs
  deadline `D`, each of them must report a clock of at least `D - 1 ns`, for a
  `/clock` sample sent at or after the one a restarted process rejoined on. A feedback loop does not deadlock, because each node
  needs only the other's deadlines before `D`. A deadline that waits is held, and
  the cycle still reports its clock up to that deadline (`replay-flow-control.md`,
  section 6.4). A held deadline has no time limit in the node: the player's
  `--timeout` names an upstream that stops reporting.
- **A time base mismatch is reported.** A sample stamped more than 250 periods
  past a step's deadline can never be admitted. This is the usual sign of a
  publisher on wall time feeding a replay. The node logs
  `replay time base mismatch` at most once a second per topic.
- **Forward jumps are stepped, in slices.** A forward jump of `/clock` runs one
  step for each expired deadline, in order, none skipped. A replay cycle runs at
  most 1024 steps, and starts none after 50 ms, in one `WaitSet` turn; the next
  turn follows at once, after the run checked its stop request and its limit and
  reported its progress at the first deadline not run, so a bounded run ends and
  the player's `--timeout` holds. A triggered cycle admits at most 1024 events per
  turn. A jump of more than 1024 periods logs `hypha: cycle '<name>' steps <n>
  periods for a forward jump of /clock from <a> ns to <b> ns` at info level. The
  player's chunks are contiguous, so one replay run makes no jump.
  A second `hypha play` run that starts later in time, against nodes still running
  from an earlier run, does: its first `/clock` jumps the whole gap. Restart the
  nodes between runs.
- **Nothing rewinds in place.** When simulated time goes back under any timer or
  cycle of a `WaitSet`, the run ends with `ClockRewound` and logs the timer or cycle and
  the new time; `runNode` restarts the process. A backward seek of `hypha play` restarts
  the process: `hypha::runNode` exits 75, `hypha launch` starts it again, and it rejoins on the next `/clock`
  (`replay-flow-control.md`, section 4.4). The new process anchors its grid on
  its first clock value, so a pass that starts again runs on the same phase as
  the first one. A plain timer (`Node::timer`) is not a cycle: it restarts one
  period after the new time.
- **Plain timers are unchanged.** A `Node::timer` callback has no time argument,
  so under simulated time it still fires once per sweep and counts the missed
  periods.

A replay is faithful when no sample it should have read was lost. A loss is one
of two things: a transport overflow, which shows as a sequence gap, or a history
eviction that a cursor had not read. A step that has a new loss on an
input (queued or latest) fails, and the `WaitSet` run that holds it fails with
`SequenceGap`. The node logs `replay not faithful: N samples lost on <topic>` at
error level, at most once per topic per second, and a repeated loss is summed
into the next line. The binding's losses also count in its `overrun_total`.
While a latest binding in host time counts neither its transport gaps nor its history
losses because an unread value was superseded in host time, under replay a sample dropped
or evicted while stamped at or after the oldest deadline still to run was never superseded
in data time: the newer value belongs to a later step. When samples have been accepted but none
are admitted because all retained samples are stamped at or after the deadline, the input history
is shorter than one replay chunk; the binding's verdict at the C ABI is `EMPTY` (not
`NEVER_ARRIVED`), or `STALE` for a queued binding past its silence limit. An Arrivals port shows
`Usable` with no new sample, and a Latest port without a value shows `NeverArrived`. When the
history evicted a sample stamped before D, so the value the step was due is gone, a latest
binding's node logs `all N retained samples on <topic> are stamped at or after deadline D: the input history is shorter than one replay chunk`.
An input whose first samples all come after D evicted nothing it was due, and logs nothing.
An input stamped on another clock is not replayed: a topic relayed from another
domain carries that domain's simulated clock, which this node's deadlines cannot cut.
The `hostClock` option (C ABI `hypha_cycle_set_host_clock`, latest inputs only)
captures it as a host-time step does, at every step: the newest sample whatever its
stamp, its silence limit judged by host time (the sender's send time, so on one host),
and no replay loss or time-base mismatch for it. Such an input makes the step depend on
arrival timing, so a replay of the node is no longer deterministic for it.
`hypha play --clock` waits for the nodes, so its replay fails only when a queue is too short
for one chunk of an input's samples (`replay-flow-control.md`, section 6.7). A run at a high
`--rate` is only as faithful as the host keeps up.

---

## 12. One subscriber, one delivery style

One subscriber uses one style. The type you create decides it.

| Style | Type | How you get data |
|---|---|---|
| Callback | `Subscriber<M>` with a callback | The run calls you once for each sample. |
| Latest value | `Subscriber<M>` | The run takes every sample, and `latestSample()` returns the newest. For tests and tools; a step reads a `Latest<M>` port. |

A `Subscriber<M>` attaches to the node's `WaitSet` when you create it.

The runtime applies the same rule. An owning receive (Rust SDK only; the C ABI
has none) and a `WaitSet` attachment each claim the subscriber, and the first
claim decides. Every
other style then returns `InvalidOperation` and takes no
sample. A wait by hand on a subscriber that a `WaitSet` drives returns
`InvalidOperation` too. One `WaitSet` drives a subscriber at a time: a second
attach returns `InvalidState` until the first registration is detached or its
`WaitSet` is destroyed.

---

## 13. What will surprise you

- **Nothing progresses until you run a `WaitSet`.** Schema queries go
  unanswered and latched topics are not replayed until then. A node that only
  publishes still needs a run for its housekeeping.
- **You cannot subscribe during a run.** Attachments freeze at run entry.
- A wildcard subscription runs about 1000 times a second whether or not data
  arrived: a pattern has no event service, so it is polled. See section 8.
- A publisher or an output writes `header.seq` and nothing else. hypha does not write
  or check `header.stamp`. The conventional value is framework time while
  simulated time is active, otherwise Unix wall time. See cpp-api.md §3 for
  the conversion.
- A raw callback's bytes are gone after the callback returns.
- A timer that fell behind does not catch up with a burst. A cycle on
  simulated time runs each expired deadline (section 11.1).
- A loop of triggered cycles alone fails with `Timeout` under simulated time. Put
  a rate cycle in the loop (section 6.6).
- A callback that throws does not reach `main`. The run counts it as a
  failure with the status `Panic` and keeps running. Read the count with
  `hypha::metric("node.callback_failures")`.
- `run()` returns a result. Ignoring it is a compiler diagnostic.
- A subscriber count is not delivery. `subscriberCount()`
  shows that a subscriber is connected. It does not show that it received a sample.

---

## 14. When a callback never runs

Check these in order.

1. **Is a `WaitSet` running?** A callback runs only inside `run()` or
   `runFor()`. Nothing runs in the background.
2. **Did you attach before running?** A subscription created during a run is
   refused with `InvalidState`.
3. **Did you keep the subscriber alive?** If the `Subscriber` object goes out
   of scope, the registration goes with it.
4. **Do the topic names match exactly?** There is no master to report a
   mismatch. A subscriber with no publisher logs one warning after five seconds. The warning names
   the topic and the domain. A subscriber that once had a publisher never warns.
5. **Is the input a callback or a cycle input?** A cycle input is read at its
   cycle's tick, never on arrival. See section 7.
6. **Is the message type the same on both sides?** A frame that does not
   match is delivered with a null payload.
7. **Did the run report a failure?** Check `RunResult::reason()` and
   `RunResult::status()`. A setup failure runs no callback at all, and names
   the attachment that refused.
8. **Do you read a subscriber that a `WaitSet` drives by hand?** Read it in its
   callback instead. At the C ABI, a receive or a wait on an attached subscriber
   returns `InvalidOperation`.
   See section 12.

Use `hypha topic list`, `hypha topic echo <topic>` and `hypha topic hz <topic>` to
see what is really on the bus. `hypha topic hz` measures the publish cadence,
so compare it with the rate at which your callback runs.

---

## 14.1 Worked examples

| Example | What it shows |
|---|---|
| [`speed`](../examples/speed) | Two `CycleNode`s, a source and a monitor, the ordinary shape |

The heavy-work example is the answer to "my callback is slow". The framework
will not add a thread for you: it counts the slow callback and reports the
count in the run result. Move the work to a thread you own, behind a queue
with a bound you chose, and keep the discard visible.

---

## 15. Related documents

| Document | What it adds |
|---|---|
| [cpp-api.md](cpp-api.md) | The C++ SDK task guide, including endpoints and QoS; the reference is the public header comments. |
| [interface-specification.md](interface-specification.md) | The C ABI contract, return codes and threading rules. |
| [Hypha Apps tools](tools.md) | `hypha topic` and the other command-line tools. |
