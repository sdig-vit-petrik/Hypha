# Execution model

How hypha runs callbacks, timers, cycles and replays. The rules hold for the C++ SDK,
the Rust SDK and the C ABI; the examples use the C++ names.

A stop (SIGINT, SIGTERM, `requestStop()`, the node going away) takes effect at a dispatch
boundary: a running callback finishes, and the run returns before the next one. Destroy
cycles or the node only after the run has returned. The node owns its cycles and
destroys them with its `WaitSet`.

## 1. Who this is for

Engineers who know ROS 1 (`ros::spin()`, `spinOnce()`, `AsyncSpinner`, `CallbackQueue`).
Several hypha functions have ROS names but behave differently. Read section 2 before you
port a node.

## 2. Quick map from ROS 1

| ROS 1 | hypha | What differs |
|---|---|---|
| `ros::spin()` | `Node::run()` | Returns a `RunResult` that you must check. |
| `ros::spinOnce()` | `Node::runFor(limit)` | No one-cycle call; run for a bounded time. |
| `ros::AsyncSpinner(N)`, `MultiThreadedSpinner` | none | hypha creates no thread. Callbacks of one `WaitSet` never overlap. |
| `ros::CallbackQueue`, `getGlobalCallbackQueue()` | `WaitSet` | One per node (`hypha_node_default_waitset`). Samples wait in shared memory. |
| `nh.subscribe(topic, 10, cb)` | `node.subscribe<T>(hypha::Topic{topic}, cb)` | Wakes on each sample (section 7). |
| `ros::Rate` + `sleep()`, `ros::Timer` | `Node::timer` | Monotonic clock on host time, `/clock` on simulated time. Missed periods never fire as a burst (section 11). |
| `ros::ok()` | `hypha::ok()` | Same. |
| `ros::shutdown()` | destroy the `Node` | The bus is released with the `Node`. |
| `const Msg::ConstPtr&` kept | typed callback: keep it; raw callback: copy it | Section 10.3. |
| roscore | none | Nodes find each other through shared memory. |

## 3. The framework creates no thread

Every callback runs on a thread your application created. Nothing runs in the background.

```text
run():
  1. build every attachment once, at run entry
  2. wait on the native WaitSet: a notification, a timer deadline, an operation deadline
  3. take a bounded batch from each ready attachment; call the callback per accepted sample
  4. check for a stop, advance operations, go to 2
```

| Part | What it is |
|---|---|
| Node session | One per process. The first `hypha::Node` creates it. |
| `WaitSet` | The registrations of a node and the thread that runs them. A node has exactly one. |
| Attachment | One subscription, timer, cycle or housekeeping job, with an id. |
| Callback | Your function, on the thread that called `run()`. |

What follows from this:

- **Housekeeping needs a running `WaitSet`.** A late subscriber of a latched topic gets
  the last sample when the publisher's `WaitSet` hears the graph event that a subscriber
  creation fires (`hypha/hk/graph/v1`), or when the publisher sends again. Schema records
  need no run: an endpoint writes its record when it opens.
- **Attachments freeze during a run.** Create subscriptions and cycles before `run()`; one
  created during a run is refused with `InvalidState`. Timers are the exception: a
  callback may create a timer on host time or on the node's own simulated clock (not on a
  clock the node does not own), and may `cancel()` and `reset()` one
  ([cpp-api.md](cpp-api.md) §3).
- **One thread per node.** A slow callback delays every other callback of the node. Work
  that must not wait goes to a thread you create, with your own locking, or to another
  process.

## 4. Running: `run`, `runFor`, `requestStop`

```cpp
hypha::Node node("my_node");
const hypha::RunResult result = node.run();   // blocks until something stops it
if (!result.ok()) {
    return 1;   // the call failed, or the run reported a failure
}
```

`run()` serves the node's `WaitSet` on the calling thread until `requestStop()`, a
callback's stop, or a failure. `RunResult` is `[[nodiscard]]`: a failed run and a stopped
run are different outcomes.

`runFor(limit)` is `run()` with a deadline in real time; at the deadline it stops as
`requestStop()` does and reports a normal stop. It always takes its first turn, so due
timers and queued samples are served even on a loaded host. A limit of zero or less is
refused with `InvalidArgument`. Compute a limit from `steadyNow()`, not from framework
time, which can jump or rewind in a replay.

`requestStop()` is the one call another thread may make on a running `WaitSet`. A stop
requested while nothing runs is kept, so the next run returns at once. The C ABI
(`hypha_waitset_request_stop`) reports `InvalidState` in that case; C++ returns nothing.

### 4.1 There is no `spinOnce()`

| You wanted | Do this |
|---|---|
| pump callbacks for a moment | `runFor(limit)` |
| run until told to stop | `run()`, and `requestStop()` from elsewhere |
| periodic work | a timer, served inside `run()` |
| several inputs, periodically | a `CycleNode` or `Cycle` (section 6.1) |

### 4.2 Restart

A `WaitSet` may run again after it stops. Timers count the stopped time as missed
periods, with no catch-up calls. A cycle's input history carries over, so the first step
judges the kept samples on host time and a value may already be stale. Registrations,
schema caches and sequence state are kept.

## 5. Bounded dispatch

Every turn is bounded, so one busy topic cannot starve a timer or a stop.

| Limit | Default | At the limit |
|---|---|---|
| Registrations per `WaitSet` | 128 | Attach returns `ResourceLimit`. |
| Samples per attachment per turn | 8 | The attachment stays ready; the rotation moves on. |
| Work items before a readiness recheck | 64 | The loop rechecks the native `WaitSet`. |
| Slow-callback budget | a timer's or cycle's period; 10 ms for a subscription | A counter, and a warning at most once per second per callback. |

These are engineering defaults, not safety limits; measure on the target. A bad limit is
refused, never clamped: zero, or a batch above the dispatch budget, is `InvalidArgument`.
The dispatcher rotates over ready attachments with a cursor, and rejected frames count
against the batch, so corrupt traffic is bounded like valid traffic.

The slow-callback budget is the one limit a node sets in its file: `slow_callback_ms` in
`[log]` (0 to 3600000). It replaces the default of every callback of the node; 0 writes no
line and keeps the counter against the default. The line names the callback:
`hypha: callback of topic '/sensing/lidar/points' took 46 ms, budget 10 ms, 9 warnings
suppressed` (or `of timer 100 ms`, `of cycle 'ekf'`). The count is
`node.slow_callbacks`, and `slow_callbacks` of one run's report. Only timers, host-time
cycles and subscriptions are timed; a replay cycle on simulated time and a triggered cycle
are not, so simulated time never makes a callback slow. A 10 Hz timer or cycle is judged
against its 100 ms period, so a 45 ms step logs nothing.

### 5.1 Asking a question whose answer arrives later

A callback runs on the thread that must deliver every other answer, so a call that waits
for a round trip would stop the loop that completes it. Such calls are refused inside a
dispatch with `InvalidState`:

- **Schema lookups.** Resolve before the run (`hypha_schema_lookup`, Rust
  `BusNode::resolve_schema_hash`). A later lookup of the same type answers from the cache,
  also inside a callback.
- **Blocking service calls** (`callAndWait()`, `waitForServer()`, FSR-43). In a callback,
  call `call(request, onResult)`; `onResult` runs at a later turn, never inside `call()`
  ([cpp-api.md](cpp-api.md) §8). A server's handler also runs on the run thread; it answers
  there or defers the request to a thread that may wait.

## 6. Freshness and periodic cycles

Three durations, never one standing for another:

| Meaning | Measured from | Configured by |
|---|---|---|
| Staleness of an input | the newest sample's send time, at each capture | the port's `silence_ms` (section 6.4) |
| Stagnation of an input | the last change of its alive value, on the step's deadline | `stagnation_ms` with `setAlive()` (section 6.4) |
| Age of the source data | the measurement stamp | not checked in this release |

A cycle judges all its inputs together, at one instant: host monotonic time, or the replay
cut on simulated time. A sample exactly at its limit is still usable. An input that is not
wired or has never received a sample is `NeverArrived`; one past its limit is `Stale`.

### 6.1 Cycle execution

A cycle is a periodic (or triggered) registration on the node's `WaitSet`. State lives in
the members of the `hypha::CycleNode`; its `Latest`, `Arrivals` and `Output` ports hold the
typed capture of each call to `void step(const hypha::CycleTime& time)`. A `CycleNode`
takes its rate from `rate_hz` in `[cycle]` or from `setRate()` in `setup()`, and its
triggers from `setTrigger(port)`. The cycle has the node's name; a composed `hypha::Cycle`
has its own and uses `setRate()`.

**Timers first.** Each turn runs due timers and cycles, in registration order, before new
ingress. A step therefore reads data ingested in an earlier turn, and a slow earlier
callback delays later cycles.

**`CycleTime`.** `now` is the step's framework time, `stamp` the same as a ROS header
stamp, `dt` the time since the previous step started (none on the first step), `missed` the
missed periods. `hypha::now()` returns `time.now` while the step runs. On host time `dt` is
monotonic, so a system-clock step never makes it negative; `now` and `stamp` follow the
system clock. A forward gap shows in `dt` and `missed`, with no catch-up burst. On simulated
time a clock that goes back ends the run with `ClockRewound` (section 11.1). A paused
simulation stops cycles while ingress and housekeeping go on.

**Capture.** A `Latest` input selects the newest accepted sample. A queued (`Arrivals`)
input has its own cursor, examines up to its share per step (section 6.5) and keeps the
rest for a later step. A step can see no new sample while its input is healthy: test
`samples().empty()` for "nothing new", and the verdict for health (`Usable` within the
limit, `Stale` after). Captured samples are consumed before the step runs, so a failed step
does not see them again.

**Decode.** The C++ SDK decodes samples into the ports before the step. A `Latest` port
decodes a value once, when it becomes the value. A sample that does not decode fails
nothing: it is counted, logged at most once per 10 s per input, and the input is judged
without it. A `Latest` port keeps its previous value (`lastKnown()`; `*port` reads only a
usable value); an `Arrivals` port delivers the other samples.

**Send.** `send(message)` serializes into a transport loan held until the step returns.
Each output has a per-step send limit (default 1, `perStep()`), and its publisher holds
that many loans. After a successful step the loans go out in call order across outputs;
after a failed step they are released unsent. Exceeding a send limit fails the step with
`ResourceLimit` and sends nothing. Use `Cycle::health()` and `introspect()` for
diagnostics; reading them does not move the capture.

### 6.2 Cycle timing limits

- **No deadline guarantee.** Due cycles run in registration order, and a slow step delays
  the ones after it. The delay from a sample's arrival to the step that reads it depends on
  the work limits of section 5 and the other registrations: measure it on the target.
- **`now` is read before the decode**, so it is not when the algorithm runs. Time real work
  with `hypha::steadyNow()`. A callback's duration includes decode, step and encode.
- **On host time `now` is the wall time when the step starts**, not its deadline, so it
  varies with how late the run woke (section 11).
- **`missed` is a per-step delta**, 0 when on time, never a total (`Cycle::health()` keeps
  the total). A host timer starts counting at the first run turn. A triggered step, a
  replay step and a rewind report 0; a forward jump of `/clock` runs one step per period
  instead. A system-clock step is not a gap. A cycle committed before the first simulated
  time arrives reports a large count at its first tick: start such a cycle after simulated
  time arrives.
- **A managed source copies each payload** into its history below the C ABI and releases
  the transport loan at once, except a large `View` input read in place (cpp-api.md §5.0).
  The history limit counts samples, not bytes. Never reinterpret a serialized payload as a
  C++ message with vectors or strings.
- **Rate-cycle inputs do not wake the `WaitSet`**; only triggers do (section 6.6). Each tick
  drains every bound source into its history before capture; cycles that share a source
  share its history. Size a source's queue for at least one period of arrivals: an
  overflow drops old samples, and a queued input counts the sequence gap in
  `overrun_total` after the next surviving sample (a `Latest` input does not).
- **On simulated time the `/clock` subscriber is notified**, so the node wakes when time
  moves.
- **Data time.** Every frame carries a framework send time: the framework clock of the
  publisher, simulated time on a sim-time node, the recorded time from `hypha play`. The
  player publishes `/clock = T` only after every message stamped at or before T
  ([replay-flow-control.md](replay-flow-control.md) §4.3 gives the one exception), and
  only after every participant reported, on `/clock_progress`, a `clock_sent_nanos` at or
  after the send floor and a `clock_nanos` at or beyond T. A participant that holds a
  deadline reports less. On simulated time a cycle judges its inputs in data time
  (section 11.1); on host time, by host transport time.
- **Services** are the request and reply of `Requests` and `Calls` ports, or of a plain
  node's server and client (cpp-api.md §8).

### 6.3 Cycle failure

| Condition | Result |
|---|---|
| An input is missing or stale | The step runs with that verdict. The application decides; the framework never supplies a default. |
| An output is not sent | Nothing is published for it. |
| The step throws | The step fails with `Panic` (`ResourceLimit` for `std::bad_alloc`). Nothing is sent. |
| A payload passes CRC and schema checks but does not decode | Counted (`RequiredInput.undecodable`), logged at most once per 10 s per input, judged without it. The run goes on. |
| A send is refused, or a payload exceeds its publisher's bound | Checked at reserve and before the first send. The step fails and nothing is sent. |
| No free loan (`PoolExhausted`) | The message is dropped and the step goes on. The run fails at dispatch unless the output is `diagnostic` (C ABI `DropPoolExhausted`), which counts `output_pool_drops`. The dropped message takes its sequence number, so subscribers count the loss. |
| `Disconnected`, `Backend`, or a send's `InvalidArgument` or `InvalidState` | The run fails. |

- **The `WaitSet` is one failure domain.** A terminal failure stops the run and every
  cycle of the node. Work that must survive it belongs in another process.
- **Sends are not atomic.** A later failure does not undo an earlier send or a state
  change. If the process dies between two sends of one step, consumers get the first and
  never the second, with no loss counted (the unsent frame took no sequence number); the
  second input turns `Stale` after its own limit. Outputs of one step carry one send
  stamp, so a consumer that needs both can compare stamps.
- **A failed step never replays its captured samples.**
- **The framework never calls `exit()` and never invents a safe output.** It cannot
  promise that a stop command reaches the bus after a failure; a system watchdog covers
  that.
- A failed send and a failed notification after a successful send differ: the second is
  only a degraded diagnostic.

### 6.4 Input supervision

**Every enabled input has a silence limit, or says it has none.** `silence_ms = 0` in its
table (a ROS timeout of 0.0) or the tag `unsupervised` in code means no limit. Without
either, the start is refused; an input never loses its limit because a key was left out
(`hypha_runtime::port_limits`). An *untimed* input is `NeverArrived` before its first
sample and `Usable` after, never stale, and logs no fault (`Input::untimed()`,
`silence_nanos = 0` in `~/introspection`, "no time limit" in `hypha top`). A required
untimed input keeps the node `WAITING` until its first sample.

The runtime (`hypha_cycle`) detects; the application decides what a fault means. There are
two checks. Staleness uses one limit, `silence_ms`, for both the verdict (`Stale`) and
`stale()`; `age_rejections_total` counts the samples it refused. Stagnation is `stagnant()`.

1. Every port with a limit is judged once per step, at capture, before the step: the
   resolution is one cycle period.
2. Staleness uses the capture clock: host monotonic time against the sample's transport
   send time, or the replay cut against its framework send time. The wall clock plays no
   part, so a wall-clock step neither delays nor advances timers, fallbacks or verdicts.
3. A port is stale when its newest sample was sent more than `silence_ms` before capture. A
   sample that is already too old when it arrives is refused and does not end staleness;
   only a fresh one does. A queued port is judged by its newest sample too: a slow publisher
   gives `Usable` steps with no new sample, a stopped one gives `Stale` steps.
4. A port that has received nothing counts from its first evaluation. A sample stamped after
   the capture clock has not aged.
5. **Stagnation needs a life sign**: an integer in the payload that the original source
   advances, declared with `setAlive()`. It is for data that comes through a relay (a
   driver, the ROS bridge, a gateway), where the frame sequence only proves the relay ran.
   A topic a hypha cycle publishes needs none: its sequence advances only when the step ran,
   and `silence_ms` catches a stopped sender. An unchanging payload is not stagnant. The
   C++ port reads the life sign; the runtime applies it after the step.
6. The stagnation timer starts at the first alive value and restarts at any change (a wrap
   included). It runs only while the port is not stale, so a port is never both; a port
   with no sample is stale, not stagnant. It runs on the step's deadline
   (`CycleTime::scheduled_nanos`: monotonic on host time, simulated time in a replay), not on
   the wall clock.
7. **Disabled and ignored differ.** A disabled port (an `Optional` port whose `topic`
   is empty or unset) reads nothing and is never stale or stagnant. An ignored port
   (`setIgnored(true)`) hides its fault from `stale()` and `stagnant()`; `ignored()` shows
   the override.

The frame's checks each cover their own fault: the CRC corruption, the sequence loss,
repeats and reordering, the send time delay and silence, the life sign a relayed source
that stopped while its relay still sends.

**Log lines** (module `hypha`), once per change, at the end of the step that saw it. There
is no periodic summary, and the initial wait logs nothing; an untimed input never logs
`never arrived`. `<in>` is `cycle '<cycle>' input '<port>' (<topic>)`, or `cycle '<cycle>'
input (<topic>)` without a table. An advisory input's line ends in ` (advisory)` and logs
at INFO instead of WARN. Ages and limits are whole milliseconds.

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

A queued input examines at most its share per step (`perStep()`, default 8). An optional
cycle budget (`arrival_budget` in `[cycle]`) caps all queued inputs together and is shared
among those with pending samples; idle and `Latest` inputs take none.

There is one overflow rule: the pending samples beyond the share are the oldest, so the
input discards them, examines the newest, and counts each discard. A `newestOnly` input
(C ABI `HYPHA_CYCLE_INPUT_MODE_NEWEST`) has a share of one, while its queue keeps the depth
of its `perStep()`. In a triggered cycle the triggers keep their pending samples for later
steps, and on host time a non-trigger queued input keeps the samples sent after the
trigger sample (section 6.6).

Each cause of loss has its own counter:

1. History overwrite: the history dropped a sample before the input read it (`overrun_total`).
2. Age rejection: the input read it and the age rule refused it (`age_rejections_total`).
3. Overflow discard: beyond the share, or passed by a `newestOnly` trigger for a newer one
   (`work_limit_drops_total`, section 6.6 item 7).
4. Clear: a step's `clear()` dropped an admitted pending sample (`cleared_total`, section 6.8).

Health queries read the transport's missing-sequence count atomically, without taking the
history locks. Fields, TOML keys and rules: [cpp-api.md](cpp-api.md) §5.4.

### 6.6 Triggered cycles

A cycle runs on a rate or on triggers, never both; a cycle with both or neither is refused
at start with `InvalidArgument` and one log line. A trigger is one of the cycle's
`Arrivals` inputs (`Cycle::setTrigger(port)`, C ABI `hypha_cycle_set_trigger` on a cycle
created with period 0). Each call adds one trigger; a repeat adds nothing; order does not
matter; at most 64. A trigger has its own silence limit, or none (an untimed trigger, section
6.4, which has no fallback, is never stale and never late). Two triggers may not share one
topic through `sharing()`, since each sample would step twice (`InvalidArgument`).

1. **One step per trigger sample.** All trigger samples run in one order: framework send
   stamp, then port (the binding index, which is construction order in C++), then arrival
   order. In a step the port that fired holds exactly its sample and every other trigger
   holds none; test `samples()` to see which fired. Two steps at one time give `dt = 0`. A
   turn runs every due trigger sample; the trigger's queue depth bounds them, and a queue
   overflow is a counted transport loss. The share, the cycle budget and the overflow
   discard do not apply to triggers, only to the other `Arrivals` inputs.
2. **What the step sees.**
   - **Its time.** `CycleTime::now` is the send stamp of every output of the step. Under
     simulated time it is the trigger sample's send time T (a fallback's own time). On
     host time it is the wall time when the step starts, as for a rate step, so outputs do
     not carry the trigger's stamp: a node that needs it copies the payload's
     `header.stamp` or reads `sentAt(index)` of the trigger port. (`scheduled_nanos` in
     the C ABI is T on simulated time, and the trigger's monotonic send time on host time.)
   - **The trigger sample** is delivered without a second age check: the schedule judged it
     once (item 3). The step is empty only if the sample does not decode. Each trigger port
     is judged by its newest sample against its own limit: on simulated time the samples
     sent at or before T, aged T − sent; on host time every kept sample.
   - **The other queued inputs** follow one rule on both clocks: *a trigger step holds what
     the upstream step that sent its trigger sample sent with it, and nothing a later
     upstream step sent.* One step stamps all its outputs with one send time, so a pair a
     driver publishes (an IMU and a magnetometer message) meets in one step, live and in a
     lockstep replay (#231).
     - On simulated time the step reads every other input through T, whatever the input's
       mode: stamped T is in, stamped after T is out. Two upstream steps at one T count as
       one. A fallback at T reads through T too. Item 5's hold makes this deterministic. A
       publisher that reports no progress (a plain callback) is not waited for, so whether
       its sample stamped T is in the step depends on arrival
       ([replay-flow-control.md](replay-flow-control.md) §4.3). Several trigger samples
       stamped T run as several steps at T; the first takes the queued samples stamped T.
       A grid (rate) step is cut strictly before its deadline instead (section 11.1).
     - On host time the input takes its pending samples, in arrival order, up to the first
       one sent after the trigger sample (by host send time). A sample sent by the same
       upstream step has the same send time and is taken; a later one waits for a later
       step or a fallback. With two publishers, an earlier sample that arrives behind a
       later one waits with it. The verdict counts every arrived sample. A `Latest` input
       and the TF input capture as on a host-time rate step: the newest kept sample, which
       may be newer than the trigger.
3. **Fallback.** When no step ran for the shortest silence limit among the timed triggers,
   one step runs with no trigger sample, and again every shortest limit until a trigger
   comes. Untimed triggers count for no fallback. A cycle whose triggers are all untimed
   runs no fallback, and no step before its first trigger: `~/state` shows its required
   inputs `NEVER_ARRIVED` and the node `WAITING` (`CycleState.steps` is 0), as a ROS
   callback with no timeout never runs.
   - The limit counts from the last step, or from the cycle's first evaluation if later; a
     trigger stamped before the first evaluation moves it no earlier. In the fallback step
     every trigger is empty; one without a sample for its own limit is stale.
   - A trigger stamped exactly at the fallback time runs instead of the fallback.
   - On host time a trigger older than its own limit (exactly at the limit still steps), or
     stamped before its port's last trigger step, runs no step: it is consumed and counted
     in `age_rejections_total`. A trigger stamped before another port's last step, or
     before a fallback, steps at that step's time, so step time never goes back.
   - The send stamp is aged once, on receipt; the silence then runs on the monotonic clock,
     so a wall-clock step moves no fallback. Fallbacks a late turn missed run as one step at
     the latest due time. On simulated time each fallback runs, at the last trigger stamp
     plus multiples of the limit, so replays repeat them.
4. **Wake-up.** Each trigger wakes the `WaitSet` when its publisher notifies, and the
   `WaitSet` wakes at the next fallback deadline. A wildcard trigger has no listener: the
   cycle polls its triggers every tenth of its shortest limit, at least every millisecond
   (every millisecond if no trigger has a limit). On simulated time the wake follows a
   cycle timer (section 11.1).
5. **Simulated time: the hold.** A step at T runs when the clock is at least T and every
   upstream of the cycle has reported at least T on `/clock_progress`, so no trigger
   stamped T can still come. (A grid cycle needs only T − 1 ns, since it cuts strictly
   before T.) A held step waits for as long as its upstream does not report; the player's
   `--timeout` is the bound.
   - Each port's trigger stamps must ascend in arrival order. A trigger stamped before the
     last step (from a publisher that reports no progress, or from several publishers whose
     samples arrive out of order) fails the run with `SequenceGap`, and the log names both
     times. A tie with a non-reporting publisher's samples runs in arrival order.
   - **Service edges need one nanosecond less.** Requests (`hypha::Requests`) and responses
     (`hypha::Calls`) are cut strictly before T on both kinds of cycle, so a triggered
     step at T needs its service peers at T − 1 ns only. An upstream that also sends the
     cycle a topic still has to report T. Without this, a triggered client and a triggered
     server would wait for each other for good.
   - **Within one `WaitSet`**, a client and its server hold each other the same way, and so
     do two cycles joined by a topic (section 11.1). Cycles of two `WaitSet`s of one node
     report as one node and are not held: their replay is not deterministic, and for
     services the cycle logs it. A peer that reports no progress (a tool, a plain node) is
     no upstream: its requests and answers are read at the first step after they came.
6. **Progress and loops.** A triggered cycle reports at most the lowest clock its topic
   upstreams reported (a cycle of its `WaitSet` that sends a topic it reads counts as one),
   and stays below its earliest pending trigger or fallback. Requests and responses step
   no cycle, so a service peer bounds which steps run, not the reported clock. A feedback
   loop made only of triggered cycles has no delay element and holds for good, until the
   player's `--timeout` ends it with an error naming the loop's nodes. Give a loop a delay
   element: a rate cycle or a clock driver.
7. **Newest only.** A `newestOnly` trigger (C ABI `HYPHA_CYCLE_INPUT_MODE_NEWEST`; ROS's
   `queue_size` 1) steps on the newest of its samples due together and passes the older
   ones. Each passed sample is consumed with no step and counted as an overflow discard
   (section 6.5): `work_limit_drops` in its next step's report, `work_limit_drops_total`,
   and `dropped` in `~/state` for a required input. Only the reader changes: the queue keeps
   its depth and nothing is lost on the wire.
   - On host time "due together" is every sample pending when the node gets its turn, so a
     slow step next steps on the newest sample, not a backlog. Item 1's merge still holds:
     another trigger's sample stamped between a passed sample and the newest steps first. A
     sample older than its limit is late (item 3), not passed.
   - On simulated time a sample is passed only by a later sample of the same stamp T, as one
     upstream step sends them; by item 5 they are all there, so the result follows from the
     recording. A non-reporting publisher's second sample at T that arrives after the step
     at T steps on its own. Two publishers on one topic keep the limit of section 11.1 (#262).

### 6.7 The TF input

A cycle can hold a TF input (C++ `hypha::Transforms`, C ABI `hypha_cycle_bind_transforms`):
a buffer of `/tf` and `/tf_static`, below the C ABI. A lookup returns an explicit result and
never waits.

1. **Filled before the step, under the step's cut.** The input is two queued optional
   bindings. Before the step every captured `TFMessage` goes into the buffer (`/tf_static`
   as static): on simulated time the samples sent strictly before a rate step's time, or at
   or before a triggered step's time (section 6.6 item 2); on host time every kept sample. A
   message that does not decode fails the step with `InvalidFrame`. Refused transforms are
   left out: a rotation that is not a unit quaternion, a
   non-finite value, a second parent, a cycle, a frame both static and dynamic, a stamp
   whose `nsecs` reaches one second. They are logged at most once a second per TF input
   with the count, and counted for good in the node metrics `tf.refused` and
   `tf.undecodable`. The input keeps each frame's last refusal until a transform of that
   frame is accepted again.
2. **Lookups** name a target, a source and a time, or ask for the latest common time
   (C ABI `latest`, C++ `lookupLatest`). Time 0 is a normal time. Unknown frame, not
   connected, before the buffer and after the buffer are distinct results; an empty name is
   refused and one leading `/` is dropped. A lookup that would fail as unknown, not
   connected or after the buffer while a frame on its chain has a refused newest transform
   answers `Refused` (#123): the producer is broken, not late. A fallback policy ("at the
   stamp, else the newest") is the application's.
3. **A clock that goes back.** On simulated time the run ends with `ClockRewound` and the
   process restarts with an empty buffer (section 11.1). A host wall-clock step does not end
   the run: a dynamic transform stamped more than the buffer length before its frame's
   newest one drops that frame's older transforms, so lookups answer from the new timeline
   at once. Descendants and static transforms keep theirs. Logged once per frame and step:
   `hypha: cycle '<cycle>' TF input: '<child>' in '<parent>' is stamped <d> s before its
   newest transform, more than the <c> s buffer: its older transforms are dropped`.
4. **Static transforms.** `/tf_static` is read with depth 32, one latched sample per
   publisher a topic can have, so a late cycle sees every static publisher.
5. **Losses.** A `/tf` burst larger than the queue is a counted transport loss and, on
   simulated time, fails the replay step with `SequenceGap`; lost samples are never
   interpolated over. A replay step takes at most 32 samples per topic; more pending fails
   with `SequenceGap` and logs the count.
6. **Loops.** Every `/tf` publisher is an upstream of the cycle. Two triggered cycles that
   feed each other through TF form the loop of section 6.6 item 6.
7. **Clear.** `clear()` forgets the dynamic transforms and keeps the static ones (section 6.8).

A plain node uses the same buffer through a transform buffer (C ABI
`hypha_transform_buffer_create`,
[interface-specification.md](interface-specification.md#transform-buffer-of-a-plain-node)).
The node's `WaitSet` ingests each `TFMessage` on the run thread as it arrives, with no cut,
on either clock. A lookup may come from any thread; one lock guards the buffer per message
or lookup, and no lookup waits on the transport. An undecodable message is counted and
logged and fails nothing; a `/tf` loss is the subscriber's missing-sequence count.

### 6.8 Clearing an input or the TF input (a reset)

A node that resets (robot_localization's `reset` and `set_pose`, an EKF restart, a tracker
reset) forgets what it received. A step calls `clear()` on an input or on its `Transforms`
port (C ABI `hypha_cycle_image_clear`). The rules are below the C ABI
(`hypha_cycle::BindingCapture::clear`, the cycle's TF input).

1. **Applied when the step returns**, like `setIgnored()` and the outputs. The rest of the
   step still reads what it captured (`*port`, `samples()`, every lookup). Two clears of one port in one step are one.
2. **What an input forgets:** its value (`has()` and `lastKnown()` of a `Latest` port turn
   false and throw), and the samples the step's capture admitted and left pending. A rate
   step leaves none pending; a host-time triggered step can leave those sent after its
   trigger (section 6.6 item 2), and they are dropped. A sample the capture did not admit
   stays (on host time one that arrived after capture, on simulated time one sent at or
   after the step's time). So a clear at step T drops exactly what was pending at T, in
   every run and every `--chunk`.
3. **A trigger port cannot be cleared**: its pending samples are planned steps. `clear()`
   throws `std::logic_error` (C ABI `InvalidArgument`).
4. **Supervision.** Until a sample after the clear arrives, the input has no value and is
   `NeverArrived`. Silence keeps counting from the newest sample before the clear: the input turns `Stale` on
   the schedule it had without the clear, so a clear never hides a stopped source and logs
   nothing for one that keeps sending. Stagnation keeps its
   value and timer; `received()` and `deliveredCount()` keep counting. The next usable
   sample logs `<in> is usable again`.
5. **Counters and log.** Dropped samples count in `cleared_total` of `bindingHealth()`
   (cause 4 of section 6.5), in no loss counter, and not in `~/state`'s `dropped`: a clear
   is the application's choice. Each clear logs one INFO line: `hypha: <in> cleared: <n>
   pending sample(s) dropped`.
6. **The TF input.** `Transforms::clear()` forgets every dynamic transform, like tf2's
   `Buffer::clear()`, and keeps the static ones. A lookup through a forgotten frame answers
   `UnknownFrame` (or `Refused` if its newest transform was refused) until a new transform
   arrives, and an earlier stamp then starts a new history. Logged: `hypha: cycle '<cycle>'
   TF input cleared: <n> dynamic frame(s) forgotten, static transforms kept`. The TF bindings take every admitted sample before the step, so no
   `/tf` sample is left pending to drop.
7. **Replay.** A reset at step T forgets the samples stamped before T, and the step at
   T + period sees those stamped in [T, T + period). A `testing::StepHarness` step records
   the clear (`cleared(port)`) and applies it when the step returns: the input is
   `NeverArrived` until the test gives it a value, and a Transforms port keeps only its
   static transforms.

## 7. Notifications: every publisher notifies, the reader decides

Shared memory wakes nobody, so every publisher also signals its topic's event service after
each send. How the topic is read decides whether that wakes anything:

| Reader | Wakes on a sample? |
|---|---|
| Callback subscription (`subscribe`, `subscribeRaw`, Rust `WaitSet::attach_subscription`) | Yes: the `WaitSet` opens a listener. |
| A triggered cycle's triggers | Yes: the `WaitSet` listens on each trigger topic. |
| A rate cycle's inputs | No: drained at the tick, with no listener. |
| A wildcard pattern | No event service spans a pattern; it is polled (section 8). |

A publisher with no listening reader signals nobody; each topic costs one small event
service. A wake never disturbs a rate cycle's schedule.

### 7.1 How a notification reaches your callback

The `WaitSet` opens one listener per callback subscription at attach, attaches it once at
run entry, and never rebuilds it during the run. A subscriber read on a schedule owns no
listener. When a listener fires, the run marks the subscription ready, consumes the pending
notifications (so an unread event cannot spin the loop) and takes a bounded batch. A
notification is a wake hint, not a count: the queue is authoritative. Nothing polls, so a
sample whose notification was lost waits for the next one; the failure is counted
(`transport.failed_notifications`). There is no relay thread: the thread in `run()` waits on
the listener.

## 8. Wildcard subscriptions

A wildcard pattern has no single event service, so its callback subscription is polled
every 1 ms: about 1000 wakes a second and up to 1 ms latency. A literal topic does not wake
an idle run, so nodes that must stay idle subscribe to literal topics. A node of rate cycles
wakes only at its rates and timers; with nothing due, a wait lasts at most 1 s.

## 9. Waiting

The C++ SDK has no blocking read. The `WaitSet` is the wait.

## 10. Callback rules

### 10.1 Which thread runs it

The thread in `run()` or `runFor()`. Callbacks of one `WaitSet` never run at the same time.
Two `WaitSet`s on two threads do, so state they share needs your own locking.

### 10.2 What a callback may do

It may publish, read a subscriber that no `WaitSet` drives, read the schema caches, and ask
for a stop. It may **not**:

- **Attach.** Attachments are frozen for the run (`InvalidState`). It may detach a
  registration of its own `WaitSet`; that registration is never dispatched again, and is
  removed and its context released when the run returns. A detach from another thread
  during a run is refused.
- **Block on a reply.** A service call is `call(request, onResult)`, answered at a later
  turn; a blocking call is refused (section 5.1).

Detach a registration before you destroy the object its context points to. The endpoint
stays alive until the detach, but the context is only borrowed: a callback on a freed
context is a use-after-free. After a detach from a callback, keep the context valid until
its `release` runs.

### 10.2.1 What a callback returns

| Action | Effect |
|---|---|
| continue | Keep running (the default). |
| continue, with a nonzero status | Keep running; node health counts the failure. |
| stop | End the run; a normal stop. |
| fail | End the run with the callback's status. |

### 10.3 How long the data lives

| Callback | Receives | May you keep it? |
|---|---|---|
| Typed (`Subscriber<T>`) | a decoded message in a shared pointer | Yes: an owned copy. |
| View (`Subscriber<View<M>>`) | a `ViewPtr<M>` over the received bytes | Yes, within the loan budget; past it the bytes are copied once (cpp-api.md §5.0). |
| Raw (`subscribeRaw`) | `RawSample.bytes`, a span over shared memory | No: copy the bytes during the call. |

A raw span lives only for the call, unlike a ROS 1 `ConstPtr`.

### 10.4 Exceptions

An exception from a subscription or timer callback does not propagate. The C++ SDK catches
it at the C ABI and returns continue with the status `Panic`: the failure is counted and the
run goes on. It logs the first failure of each callback, then at most one line every 10 s.
`hypha::metric("node.callback_failures")` reads the count; the `node.*` metrics also give
every call, the last failure's status and the time the last sweep ended. Handle your own
errors; do not rely on an exception reaching `main`.

### 10.5 A frame that does not parse

A sample that is not a hypha frame is still delivered, with `payload` null and only the
topic set, so a node that counts foreign traffic sees it.

## 11. Timers

`Node::timer(period, callback)` returns a `hypha::Timer` on the node's `WaitSet` and throws
`std::runtime_error` if refused (Rust: `attach_timer` on a `WaitSet` you created). It is the
one periodic registration; there is no timer thread.

- **Clock.** On host time a timer is due on the monotonic clock, so a system-clock step
  neither delays nor advances it. On Linux a `timerfd` ends the wait at the deadline
  (creating a `WaitSet` without `timerfd` fails with `Backend` code 42); on macOS the wait
  is `select`, which the kernel may defer by up to half the wait, at most about 10 ms. On
  simulated time a timer follows `/clock`.
- **Thread.** It runs on the same thread as the subscription callbacks, never at the same
  time.
- **No burst.** If several periods passed since the last sweep, the callback runs once and
  reports the missed periods; the next on-time call reports 0. This holds for plain timers
  and host-time cycles; a cycle on simulated time replays its deadlines (section 11.1).
- **No rewind.** When `/clock` moves back, the timer fires nothing and its run ends with
  `ClockRewound`; `runNode` restarts the process (section 11.1). A host wall-clock step is
  not a rewind.
- **A second rate inside a step is a `CycleTimer`**, not a timer ([cpp-api.md](cpp-api.md)
  §5.4). The step polls it with `CycleTime`; it ticks on a grid anchored at its first poll
  (or `start(time)`) on the step's time, so a replay repeats its ticks. A poll gives the
  grid instants passed since the last poll. It wakes nothing: a tick runs in the first step
  at or after its instant.

### 11.1 Cycles on simulated time

A cycle on simulated time is a replay cycle: it runs every period in data time, so results
do not depend on how fast the clock moves or on the host.

**A simulator can drive `/clock`.** A rate cycle marked `Cycle::setClockDriver` publishes its
start value with no step, then steps only after every participant acknowledged the previous
value, as `hypha play` waits for its chunks. Its first step is one period after the start.
The start is 0 to, not including, 2^32 s (the range of `/clock` seconds); another value is
refused (`hypha_cycle_set_clock_driver`, with the range in `hypha_last_error`). Rules:
[replay-flow-control.md](replay-flow-control.md) §4.5.

- **Deadlines are on the grid**: whole multiples of the period, the first strictly after the
  first clock value the cycle sees, so the phase does not depend on start time. A clock
  value is a `/clock` sample, `setSimTime`, `advanceSimTime`, or `enableSimTime` with a
  start. The initial zero is a placeholder that sets no phase; `hypha::isClockValid()` is
  false until a value arrives.
- **One step per expired deadline.** When the clock passes `D1 < D2 < … ≤ now`, inputs are
  drained once, then one step runs per deadline, each with `now = Di`, `missed = 0`, and
  outputs and header stamps of `Di`.
- **Each step sees its deadline's data**: samples sent before `Di`. A queued input stops its
  cursor there; a `Latest` input takes the newest by arrival before it (with several
  publishers, which arrived last depends on host timing, replay-flow-control.md §6.7). A
  sample sent at `Di` belongs to the next step.
- **Ages are data-time ages**: `Di − sent`, for silence and stagnation alike. A sample 600 ms
  old at the deadline is stale under a 500 ms limit, whatever the host speed.
- **Wake-ups follow the clock**: the node wakes on each `/clock` event; timers do not poll.
- **Progress is reported per `/clock` sample.** After each turn, a `WaitSet` with a replay or
  triggered cycle publishes `hypha_msgs/ReplayProgress` on `/clock_progress`: the node's
  name, a clock value and the send time of the sample it answers. First it drains the inputs
  of every replay cycle (not its plain subscriptions), so samples sent before that `/clock`
  are in the histories. The value is the sample's, lowered to 1 ns below the earliest
  deadline not yet run. Every new sample gets a report, even at a repeated value.
- **A held deadline waits for upstream nodes**: those that publish the cycle's inputs and
  `/clock_progress`, except itself. Before deadline `D` each must report at least `D − 1 ns`
  (`D` for a triggered cycle, section 6.6), for a `/clock` sample sent at or after the one a
  restarted process rejoined on. A feedback loop does not deadlock: each node needs only
  the other's deadlines before `D`. A held cycle still reports up to that deadline. Nothing
  in the node times out a hold; the player's `--timeout` names an upstream that stops.
- **Cycles of one `WaitSet` hold each other as two nodes do.** A cycle that reads a topic
  (same topic and stream) sent by another cycle of its `WaitSet` treats it as an upstream:
  deadline `D` waits until the other ran every deadline before `D`, a triggered step at T
  until the other ran T. A client and its server in one `WaitSet` do the same (section 6.6
  item 5). The turn repeats these cycles' timers while a held step can run, within its step
  budget, so what a step reads depends neither on `--chunk` nor on turn order. A cycle is no
  upstream of itself. The reader drains at its own turn, so its queue must hold one chunk of
  the sender's steps (an `Arrivals` port's `perStep()`: 20 for a 100 Hz sender at `--chunk
  200`); a smaller queue fails the run with `SequenceGap`. Cycles in two `WaitSet`s of one
  node are not held.
- **A time-base mismatch is reported.** A sample stamped more than 250 periods past a
  deadline can never be admitted, usually a wall-clock publisher feeding a replay. Logged as
  `replay time base mismatch`, at most once a second per topic.
- **Forward jumps are stepped, in slices**: one step per expired deadline, none skipped. A
  replay cycle runs at most 1024 steps per turn and starts none after 50 ms; the next turn
  follows at once, after checking stop and limit and reporting progress, so a bounded run
  ends and the player's `--timeout` holds. A triggered cycle admits at most 1024 events per
  turn. A jump over 1024 periods logs `hypha: cycle '<name>' steps <n> periods for a forward
  jump of /clock from <a> ns to <b> ns`. One replay run makes no jumps (the chunks are
  contiguous); a second `hypha play` against still-running nodes jumps the whole gap, so
  restart the nodes between runs.
- **Nothing rewinds in place.** When simulated time goes back under a timer or cycle, the run
  ends with `ClockRewound` and logs it; `runNode` exits 75 and `hypha launch` restarts it, and
  it rejoins on the next `/clock` (replay-flow-control.md §4.4). A node that follows `/clock`
  also checks `/clock` itself, whatever it holds and in the open loop too: a lower sample is
  not applied, the run ends with `ClockRewound`, and the node logs `hypha: '<node>' saw
  /clock go back from <a> ns to <b> ns` (#130). A CLI tool (`_hyphacli_<tool>`) follows the
  clock back. The new process anchors its grid on its first clock value, so a repeated pass
  runs on the same phase. A plain timer starts one period after that first value.

**Faithfulness.** A replay is faithful when no sample it should have read was lost: no
transport overflow (a sequence gap) and no history eviction of an unread sample. A step with
a new loss on any input fails, and its `WaitSet` run fails with `SequenceGap`; in the open
loop (section 11.2) the loss is counted and warned of. The node logs `replay not faithful: N
samples lost on <topic>` at error level, at most once per topic per second (repeats are
summed), and the binding counts the losses in `overrun_total`.

A `Latest` binding counts differently:

- On host time it counts neither gaps nor history losses: an unread value was superseded.
- On a replay it counts every transport gap. A sample it passed over, or had not read when
  the history evicted it, counts only at a step that finds no kept, unrejected sample before
  its deadline. Until then a newer unrejected sample (kept, or held by the binding)
  supersedes it, since every later deadline admits that sample. A rejection of that sample,
  by the step or by another reader, ends the supersession. A step that selects nothing counts
  every evicted sample not superseded by one the binding holds for good.
- The count errs only towards a loss, in the three cases of replay-flow-control.md §6.6: an
  evicted rejected sample, an evicted sample at or before the newest evicted rejected one,
  and a step that selects nothing counting an evicted sample stamped at or after its
  deadline. The last cannot happen on a topic only the player publishes to a node it waits
  for.

When samples were accepted but none is admitted because every kept sample is stamped at or
after the deadline, the history is shorter than one replay chunk. The binding's C ABI verdict
is `EMPTY` (not `NEVER_ARRIVED`), or `STALE` for a queued binding past its limit; an Arrivals
port shows `Usable` with no new sample, a Latest port without a value `NeverArrived`. If the
history evicted a sample stamped before D, a `Latest` binding logs `all N retained samples on
<topic> are stamped at or after deadline D: the input history is shorter than one replay
chunk`. An input whose first samples all come after D logs nothing.

**Foreign clocks.** A topic relayed from another domain carries that domain's simulated clock,
which this node's deadlines cannot cut. The `hostClock` option (C ABI
`hypha_cycle_set_host_clock`, `Latest` inputs only) captures it as a host-time step does: the
newest sample whatever its stamp, judged by host time (so on one host), with no replay loss or
time-base check. The step then depends on arrival timing, so its replay is not deterministic.

**Sizing.** `hypha play --clock` waits for the nodes and a node drains its replay inputs every
turn, so a queue holds one chunk, which the player splits to fit. A replay fails only when an
input history is too short: a queued input needs a `perStep` of at least period × rate; a
`Latest` input's history of its queue plus one always suffices for a topic only the player
publishes (replay-flow-control.md §6.7). A run at a high `--rate` is only as faithful as the
host keeps up.

### 11.2 Plain callbacks in a replay

The lockstep runs cycles only: rate cycles (section 11.1) and triggered cycles (section 6.6).
A plain subscription or `Node::timer` takes no part (#153):

- **When it runs.** As its sample arrives, with the clock as framework time. A plain timer is
  due against `/clock`, fires once per turn and counts missed periods (section 11). It holds
  for no node, and the player does not wait for it.
- **Progress.** It reports nothing. A node opens `/clock_progress` when it commits its first
  cycle, so a node with only plain callbacks is neither waited for nor anyone's upstream; its
  replay is not deterministic. Framework timers (the 1 s `~/state` timer of `runNode`) are
  plain timers too.
- **Rewind.** A node on simulated time that reads a lower `/clock` ends its run with
  `ClockRewound`, whatever its `WaitSet` holds (#130).
- **Services.** A plain node's server and client are plain callbacks: requests are served and
  outcomes delivered as they arrive, with no cut and no hold. A client's timeout counts on
  simulated time, so it does not expire while `/clock` is paused. A `Deferred` answered from a
  worker thread is outside any replay. The cycle ports (`Requests`, `Calls`, cpp-api.md §8.5)
  make calls part of the lockstep (section 6.6 item 5,
  [design/native-services.md](design/native-services.md) §4).
- **Two modes, one switch.** The lockstep is the default. `HYPHA_REPLAY_LOCKSTEP=0`, set for
  the whole launch, is the open loop: every sim-time node follows `/clock` as a ROS node
  follows a bag, reports no progress, holds no deadline, and warns of a lost replay input
  instead of failing (replay-flow-control.md §4.6). `HYPHA_USE_SIM_TIME=follow` is refused:
  no single node leaves the lockstep. A system that needs a deterministic replay is built from
  cycles; one that cannot be, such as a port of ROS nodes, plays in the open loop.

### 11.3 Design note: one execution model for cycles and plain callbacks (#136)

Closed by the owner's decision of 2026-11-24 (#153): the lockstep covers rate and triggered
cycles, and plain callbacks leave it. A plain subscription has no supervision; an
event-driven node that needs verdicts or a deterministic replay becomes a triggered cycle
(#138).

## 12. One subscriber, one delivery style

The type you create decides the style.

| Style | Type | How you get data |
|---|---|---|
| Callback | `Subscriber<M>` with a callback | The run calls you once per sample. |
| Latest value | `Subscriber<M>` | The run takes every sample; `latestSample()` returns the newest. For tests and tools; a step reads a `Latest<M>` port. |

A `Subscriber<M>` attaches to the node's `WaitSet` when created. Below the C++ SDK the first
claim decides: an owning receive (Rust SDK only; the C ABI has none) or a `WaitSet`
attachment. Every other style then returns `InvalidOperation` and takes no sample, and so
does a wait by hand on a subscriber a `WaitSet` drives. One `WaitSet` drives a subscriber at a
time: a second attach returns `InvalidState` until the first is detached or its `WaitSet`
destroyed.

## 13. What will surprise you

- Nothing runs until a `WaitSet` runs, including latched delivery to a late subscriber.
- You cannot subscribe during a run; only a timer on host time or on the node's own simulated
  clock may be created from a callback.
- A wildcard subscription wakes about 1000 times a second (section 8).
- A publisher or output writes `header.seq` and nothing else. hypha neither writes nor checks
  `header.stamp`; the convention is framework time on simulated time, Unix wall time otherwise
  ([cpp-api.md](cpp-api.md) §3).
- A raw callback's bytes are gone when the callback returns.
- A timer that fell behind does not burst. A cycle on simulated time runs each expired
  deadline (section 11.1); a plain timer does not (section 11.2).
- A loop of triggered cycles alone fails with `Timeout` on simulated time. Put a rate cycle in
  the loop (section 6.6).
- A callback that throws does not reach `main`: the run counts `Panic` and goes on
  (`hypha::metric("node.callback_failures")`).
- `run()` returns a result; ignoring it is a compiler diagnostic.
- `subscriberCount()` shows a connected subscriber, not a delivered sample.

## 14. When a callback never runs

Check in order:

1. **Is a `WaitSet` running?** Callbacks run only inside `run()` or `runFor()`.
2. **Did you attach before running?** A subscription created during a run is refused with
   `InvalidState`.
3. **Is the subscriber alive?** When the `Subscriber` goes out of scope, so does its
   registration.
4. **Do the topic names match?** There is no master. A subscriber with no publisher logs one
   warning after 5 s, naming the topic and domain, or, if the topic has publishers of other
   schemas, its own type and MD5 and the published ones. It warns again when the published
   schemas change, and 5 s after its publisher left.
5. **Callback or cycle input?** A cycle input is read at the tick, never on arrival (section 7).
6. **Same message type on both sides?** A topic has one stream per type and MD5; a subscriber
   of another MD5 receives nothing. The warning of item 4 names both.
7. **Did the run fail?** Check `RunResult::reason()` and `status()`. A setup failure runs no
   callback and names the attachment that refused.
8. **Do you read a `WaitSet`-driven subscriber by hand?** Read it in its callback. At the C ABI
   a receive or wait on an attached subscriber returns `InvalidOperation` (section 12).

`hypha topic list`, `hypha topic echo <topic>` and `hypha topic hz <topic>` show what is on the
bus; compare `hz` (the publish rate) with how often your callback runs.

### 14.1 Worked examples

| Example | What it shows |
|---|---|
| [`speed`](../examples/speed) | Two `CycleNode`s, a source and a monitor: the ordinary shape. |

A slow callback gets no extra thread: the framework counts it and reports it in the run
result. Move the work to a thread you own, behind a queue with a bound you chose, and keep the
discards visible.

## 15. Related documents

| Document | What it adds |
|---|---|
| [cpp-api.md](cpp-api.md) | The C++ SDK task guide, endpoints and QoS. The reference is the header comments. |
| [interface-specification.md](interface-specification.md) | The C ABI contract, return codes and threading rules. |
| [tools.md](tools.md) | `hypha topic` and the other CLI tools. |
