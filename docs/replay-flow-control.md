# Replay flow control

## 1. Problem

`hypha play --clock` was open loop. It published data and `/clock` on a wall-time schedule and never
learned whether a consumer kept up. A sim-time cycle then had to infer, from what reached it, what a
host stall did to it. Every rule it used for that was a guess that held only for some rates:

| Symptom seen in the study | Cause | Workaround tried |
|---|---|---|
| `replay not faithful: N samples lost` | A consumer stall longer than its queue (21 samples, about 4 ms of wall time at 100x) | Input queues of 256; a start-order failure followed, because the first endpoint fixes a service's queue maximum |
| A "fell N periods behind" warning at 40x | After a stall of its own, the player publishes all due data at once, so a node sees seconds of data in a few milliseconds | A jump bound in the node's parameter file; its tolerance in wall time is `periods x period / rate`, so no value suits every rate |
| `replay time base mismatch` after `--loop` | The pass ends and the clock rewinds at once; the tail of the old pass is still queued, stamped hours ahead | A closing `/clock` and a drain pause were proposed |
| Output fixes missing from a recording | Observers with small queues and polling loops | Deeper queues in the tools |

The result depends on the host's scheduling, so a replay is not reproducible, and loss shows up
only as warnings after the fact.

## 2. Goal

One rule: **`/clock` never runs ahead of what every participant has finished.**

- A replay in sync mode is lossless and deterministic on any host, under any load, in any start
  order. Its output equals that of a replay at any other rate. The one exception is a replay input
  whose topic has several publishers (6.7).
- The player publishes every message of the recording; it never skips ahead of a slow node.
- Nodes need no change. Live operation (no player) is unchanged.
- **Two modes, one switch.** The lockstep is the default and is deterministic. The open loop, a
  playback like `ros2 bag play`, is `HYPHA_REPLAY_LOCKSTEP=0`, set once for the whole launch (4.6).
  Nothing in between: no node of a launch can leave the lockstep alone. The one rule: use the
  lockstep; if the system cannot run in it, set the variable. The old compensations for open-loop
  replay are removed.

Not goals: more throughput than the slowest participant allows (that is CPU work in the node), a
network transport, and replay across hosts.

## 3. Roles

- **Participant**: a process whose output must be faithful: every node on simulated time with a
  rate or triggered cycle, and `hypha record --sync`. Plain callbacks take no part (#153): they
  run as their samples arrive, and a node with only plain callbacks is not waited for
  (`execution-model.md` §11.2).
- **Observer**: everything else (`hypha foxglove`, `hypha top`, `hypha topic`, and
  `hypha record` without `--sync`). A CLI tool's node (`_hyphacli_<tool>`) never takes part, even
  on simulated time. An observer samples and may lose data; it never slows the
  replay. Each followed topic has a queue of 8.
- **Player**: `hypha play --clock`. It is the only source of time and the only place where
  pacing and loss are decided. One player per domain. It refuses to start while another process in
  the domain already publishes `/clock`. It checks before it creates its own `/clock` publisher, and
  again after the start barrier, so two players started together both stop with an error. A ROS
  clock bridged onto the bus is refused too.

## 4. Protocol

### 4.1 Progress topic

Each participant publishes `hypha_msgs/ReplayProgress` on `/clock_progress`:

```text
string node                 # node name, unique in the domain
uint64 clock_nanos          # every deadline of the node's replay cycles at or before this time has run
uint64 clock_sent_nanos     # host send time of the /clock sample that the report answers
```

`clock_sent_nanos` makes acknowledgements unambiguous across passes. Host monotonic time never
goes back, so the player matches an acknowledgement to the `/clock` sample it published, not to a
clock value that a later pass reuses. No pass counter is needed.

A report may show a clock below the newest `/clock` value that the node has read. That partial
report says that the node still holds deadlines (4.2, 6.4). The message definition is unchanged.

`/clock_progress` has the fixed framework limits (32 publishers, 32 subscribers, depth 32); a policy
an endpoint declares is ignored, so the first process to start decides nothing. A node's progress
input asks for depth 8, the player's too: iceoryx2 gives each publisher its own queue, so 8 is the
unread reports of one node, and a lockstep replay leaves at most one (measured 2026-11-15). `apps/hypha_tools/tests/progress_service_limits.rs` checks that a player, a recorder or a
node starts in any order.

### 4.2 Node side (Hypha runtime, no application code)

A node on simulated time wakes on each `/clock` event and runs one step per expired deadline
(execution model, section 11.1). After each turn of a `WaitSet` that holds a replay cycle, the
runtime publishes one `ReplayProgress` for the newest `/clock` sample it has read:

- **Clock.** The sample's value, or one nanosecond below the earliest deadline that has not run,
  whichever is lower. A node that holds a deadline for an upstream node reports a partial clock. It
  sends that anyway, so its downstream nodes can run the deadlines below it.
- **When.** A report is sent for each new sample, identified by its host send time. It is also sent
  when the same sample's clock has risen since the last report. A report is never skipped because
  the clock value equals an earlier one: a resend or a later pass reuses clock values, and a
  repeated value must still be answered.
- **Only a replay `WaitSet` reports.** The `WaitSet` must hold a cycle on simulated time. It
  reports after its timers, so the steps that the sample made due are published first. A sample ahead of the
  node's own time waits for a later turn. A run that stops or fails reports nothing.
- **A node reports for all its `WaitSet`s.** When several `WaitSet`s of a node hold such cycles, the
  node sends the lowest clock they offer, and only once each has answered the same sample. A
  `WaitSet` that reported an older sample holds the report until it answers this one; one that
  has never reported does not count yet. A `WaitSet` stops holding when its run ends for any
  reason but a `run_for` deadline, when it runs again without such a cycle, or when it is
  destroyed. The offers left are then reported once they answer one sample.
- **The inputs are drained before the report.** Each turn of such a `WaitSet` drains the inputs of
  every replay cycle it holds that ran no step on the turn; one that ran steps drained them before
  its first step, and a triggered cycle drains its inputs on each of its turns. Every such drain
  begins after the `/clock` sample was read. A drain takes as many samples as the queues of all
  publishers of the input hold. So a report answers a `/clock` sample only once the samples sent
  before it are in the input histories, and the next chunk cannot push an unread one out of a queue
  (6.7). A publisher that keeps sending during a drain can use up that bound and leave samples
  queued after the report. Plain callback subscriptions are not drained before the report.
- **A failure to publish** is logged once as an error. The node keeps running, and the player times
  out on the missing acknowledgement.
- **The data of a step is published before its progress report.** On one host, iceoryx2 places a
  sample in each subscriber's queue during `send`, so a reader that has seen the progress message
  also finds the outputs that came before it. Section 6.3 depends on this. `tests/replay_progress.rs`
  checks it, including from another thread.

### 4.3 Player side

The player advances in **chunks** of `--chunk` data time (default 100 ms). Every chunk ends with one
`/clock` sample, empty chunks included.

1. **Start barrier** (section 5). Each pass (the start, each `--loop`, each restart) begins with
   `/clock` at its first log time, and the player waits for every participant to acknowledge it
   before any data of that pass. The pass starts at `--start-utc` when it is given, else at its first
   log time.
2. **Publish one chunk.** Every message with log time at or before `T + chunk`, in order. With
   `--rate`, each message waits for its wall deadline first.
3. **Close the chunk.** At the boundary `T + chunk` the player first waits for the wall schedule to
   reach it (none with `--rate max`). Then it records the host monotonic time as the floor and
   publishes `/clock = T + chunk`.
4. **Wait for acknowledgements.** A participant has acknowledged when its newest report was sent at
   or after the floor, and its clock is at or beyond the chunk's value. A partial report does not
   acknowledge. While it waits, the player sends the same `/clock` again every 200 ms; the floor stays
   that of the first send. The executor is served meanwhile.
5. Repeat from step 2.

Data in flight is bounded by one chunk. The rule: **a chunk closed at `T` holds every message
stamped at or before `T`**, on every topic. `/clock = T` is published only after all of them are
out, so a node that reads `/clock = T` has every message stamped `T` queued, whatever their topics:
what it sees at `T` does not depend on the order in which they arrive. A node therefore holds no
deadline for the player (6.4).

**The one exception is the first instant of a pass.** Its `/clock` is the start barrier (step 1),
which goes out before any data, so the messages stamped at the start follow it. The first chunk
thus runs from the start to the first boundary, both included: a topic on the chunk grid has
chunk x rate + 1 samples in it, one more than in any later chunk.

A chunk must also fit each subscriber's queue. With `--clock`, when the next message would be the
one past the smallest queue (`min_subscriber_buffer`) of the subscribers connected to its topic, the
player closes the chunk one nanosecond before that message's time and opens a new one, so the
barrier waits for it as for any boundary, and the closed chunk still holds every message stamped at
or before its `/clock`. The grid is unchanged and the extra boundary is only added; the limit is
read again at each chunk close, so a subscriber that connects later counts from the next chunk. The
player prints `chunks shortened on '<topic>'` once per topic; for a topic on the chunk grid, a queue of
exactly chunk x rate samples shortens the first chunk of every pass, which is expected. Only messages that share one instant and
exceed the queue fail the run (`N messages of '<topic>' at one instant exceed the smallest subscriber
queue B`). A node's input history must still hold what its cycle has not read yet (6.7).

### 4.4 Restart, loop, backward seek and pause

A pass ends with `/clock` at the first boundary after its last message, or at the window's end
(`--end-utc`) when that comes first, so the deadlines up to it run and the tail of a pass is
processed, which open-loop replay never did. A last message on a boundary is closed by that
boundary, as any message is, and the empty chunk after it follows. The
player goes back to the start (`--loop`, the restart key) only after the barrier of that chunk, so no
participant holds unread data from the old timeline. A restart stops before the next message and
closes the open chunk one nanosecond before that message's time; at the first instant of a pass it
sends the start's `/clock` again.
Pause holds at the next message, even in the middle of a chunk. Messages already sent stay sent;
the rest of the chunk, its `/clock` and the wait for acknowledgements wait until resume. The step
key plays the rest of one chunk, sends its `/clock` and pauses again.

**A backward seek restarts the participants.** No node rewinds in place. A pass that goes back runs
in this order:

1. The player sends the pass's first `/clock` value once. Each node on simulated time reads a
   `/clock` lower than its last one and ends the run with `ClockRewound`, whatever its `WaitSet`
   holds; a timer or cycle that sees the clock go back does the same.
2. `hypha::runNode` (C++) destroys the node, so it leaves the bus, and exits 75. Its supervisor,
   `hypha launch` (or systemd with `RestartForceExitStatus=75`), starts it again at once; `hypha launch`
   sets `HYPHA_REJOIN_AFTER_NANOS` to its steady time when it saw the exit. A participant that no
   supervisor restarts stays away, and the player fails after `NAMED_START_TIMEOUT`.
3. The player sends nothing more until no participant is left on `/clock_progress`
   (`NAMED_START_TIMEOUT`, 30 s, else it fails and says how many are left). It then drops its own
   publishers, so no latched sample of the old pass remains, and opens them again.
4. The player runs the start barrier again, with the participants of the first pass as named nodes,
   and sends the first `/clock` value again.
5. A restarted process waits in its first node creation, before any port of its own, for a `/clock`
   sample sent after the one that rewound it. Older `/clock` samples are dropped,
   and a progress report that answers an older sample does not count (6.4). A participant that does
   not come back within 30 s fails the replay under its name.

Every old participant has left before any new one reads its first `/clock`, so no new process reads
a sample of the old timeline. Determinism comes from the fresh start, not from bookkeeping inside a
running cycle. A participant that does not restart itself (a Rust node, or a C++ program with its
own `main`) ends its run with `ClockRewound`, and the player fails when it does not rejoin. A node
on simulated time that takes no part, such as a bridge or any node in the open loop (4.6), ends its
run the same way (#130); only a CLI tool follows the clock back.

A forward jump of `/clock` is not announced. Each period that the jump passes runs one step, in
order, as a chunk is stepped. No node reads an announcement, so none can disagree about the gap. The
player's chunks are contiguous, so one replay run makes no jump; a second run that starts later
in time, against nodes still running from an earlier one, jumps the whole gap at its first
`/clock`. A third-party `/clock` publisher
(for example a bridged ROS clock) that jumps far forward therefore costs one step per period it
covers: that is correct, and only slow. The steps run in slices of at most 1024 steps or 50 ms per
turn of the `WaitSet`; between slices the node checks its stop and run limit and reports its progress
at the first deadline it has not run, so the player keeps waiting for it instead of timing out.

### 4.5 A simulator drives the clock

A simulator that publishes `/clock` itself uses the same barrier as the player. Its step
is a rate cycle marked as clock driver (`Cycle::setClockDriver`, C ABI
`hypha_cycle_set_clock_driver`); the cycle's period is the clock step. The rules live once,
in `hypha_runtime::clock_barrier`; `hypha play` reaches them through `hypha::clock_barrier`.

- **Acknowledgement.** A participant acknowledged a value V when its newest
  `/clock_progress` report was sent at or after the floor (host monotonic time taken just
  before the `/clock` sample went out) and its clock is at least V. Required participants
  are every node heard on `/clock_progress` plus every expected name. A driver expects at
  least one name, or it is refused at commit: with none it would step before any node
  joins. A node that joins late is required from its first report.
- **Step.** The driver's step at D' runs only when every required participant acknowledged
  D. Simulated time is set to D', the step's inputs are cut strictly before D' as a replay
  step's are, its outputs carry D' as their framework send stamp, and `/clock = D'` goes
  out after the step's outputs. A participant therefore reads the driver's step-D' outputs
  together with `/clock = D'`, and the driver reads at D' what the participants sent for D.
- **Start.** The first value is the configured start. The driver resends it until every
  expected name has acknowledged it; an expected name that never reports within 30 s fails
  the run with `Timeout`.
- **Resend.** An unacknowledged value is sent again every 200 ms, with the first floor, so
  a participant that missed the sample still answers it.
- **Silence.** A required participant that acknowledges nothing for the driver's timeout
  fails the run with `Timeout` and one log line naming it and its last clock. The driver
  never advances past it.
- **Pace.** Without a real-time factor the driver steps as fast as the participants
  allow, at most one step per `WaitSet` turn (a 1 ms wake). With a factor f, step D' waits
  until host time reaches the first step's host time plus (D' − start) / f.
- **Ownership.** The driver owns the process's simulated time: commit refuses a process
  already on simulated time (a node started with `HYPHA_USE_SIM_TIME`, or `enableSimTime`), and a bus
  where another process already publishes `/clock`; it enables simulated time at the start
  value, and destroying the driver turns it off. While it runs, the process takes no other
  clock owner: no node started with `HYPHA_USE_SIM_TIME`, no `enableSimTime`. The driver's node runs no other
  cycle, so the simulator never reports progress and never becomes an upstream of the
  nodes it drives; commit refuses either order.
- **Loops.** A loop made only of triggered cycles has no delay element and fails with
  `Timeout`. The driver is the delay element: a command a participant stamps D is read by
  the driver's step at D + period.

### 4.6 Open-loop playback (`HYPHA_REPLAY_LOCKSTEP=0`)

`HYPHA_REPLAY_LOCKSTEP=0` (or `false`) turns the lockstep off for a playback like `ros2 bag play`:
`/clock` and the data on the `--rate` schedule, with no feedback from the nodes (#102). `1`, `true`
or an unset variable keep the lockstep; any other value refuses the player and fails node creation.
The player and every node read it, so set it once for the whole launch.

- **Player.** `hypha play --clock` publishes the data of each chunk on the `--rate` schedule and
  `/clock` once at each chunk boundary. It reads no `/clock_progress`, runs no start barrier, waits
  for no acknowledgement, sends no `/clock` again and splits no chunk for a subscriber queue. A
  `--loop` or a restart sends the first `/clock` of the pass and goes on at once. `--rate max` is
  refused, since nothing would pace the replay, and so are `--nodes`, `--settle` and `--timeout`.
  `--chunk` sets how often `/clock` steps.
- **Nodes.** A node started with `HYPHA_USE_SIM_TIME=1` follows `/clock` outside the lockstep: it
  reports no progress, its plain callbacks run as their samples
  arrive with framework time the clock, and a replay cycle runs one step per deadline but holds no
  deadline for an upstream node. A replay input that lost samples (a queue overflow, a TF input
  that dropped samples) is counted and logged as a warning, `replay not faithful: N samples lost on
  <topic> ...; outside the replay lockstep the run goes on`, instead of failing the run.
- **Rewind.** A `/clock` that goes back still restarts every node (FSR-37, 4.4): it exits 75, and
  its supervisor starts it again. The player does not wait for it, so a restarted node misses the
  data that goes out while it starts.
- **Determinism.** None: what a node sees depends on the host, the load and the start order, as in
  a ROS playback. A slow node drops samples, and nothing stops the replay for it. A
  `hypha record --sync` recorder is not waited for either; record without `--sync`.

## 5. Who participates

A participant is expected either by name or by count:

- By default the player waits for every node that publishes progress during the start barrier,
  until none has appeared for `--settle` (default 2 s). This suits a single launch file. If no node
  has reported by then, the player warns and paces by `--rate` alone, without acknowledgements.
- `--nodes <name>...` adds required nodes: the start barrier also waits for each named node to
  report, and fails after 30 s if one is missing. A node that reports without being named still
  takes part. This suits scripted runs and CI. After 1 s the player names each missing node, and
  the error after 30 s names it again: `'<name>' runs but is not on /clock_progress` (a node with
  no cycle on simulated time, which takes no part) or `no node '<name>' runs in this domain`.

The player also counts the publishers connected to `/clock_progress`. While a publisher has connected
but not reported, the start barrier keeps waiting, so a recorder that announces itself with its
first report is not missed. The wait ends at `--settle` after the start, so a silent publisher delays
the start by that long and never fails the run.

A node opens its `/clock_progress` publisher when it commits its first cycle on simulated time. A
node with only plain callbacks opens none: the player does not wait for it, and no cycle holds a
deadline for it (#153).

A node that joins after the start, for example after a restart, becomes a participant at its first
progress message. The player then waits for it from the next chunk on.

## 6. Failure cases

### 6.1 A participant stops reporting or does not advance

The player never waits in silence (#148). A wait at the start barrier or for an acknowledgement
that lasts longer than 1 s prints the nodes it waits for and the `/clock` value, again whenever
that set changes:
`hypha play: waiting for '<a>' (last clock <c> ns), '<b>' (never reported) to answer /clock <v> ns`.

A participant whose reported clock does not rise for `--timeout` (default 5 s wall time) while the
player waits for it stops the replay, whether it went quiet or keeps answering the resent `/clock`
with a clock it never raises. The error names every node the player still waits for, each as
`never reported`, `stopped reporting: its last clock was <c> ns` or `reports <c> ns but does not
advance to <v> ns`, and ends with the advice that only cycle nodes take part in the lockstep and
that a system the lockstep cannot run plays with `HYPHA_REPLAY_LOCKSTEP=0`. When every awaited node
keeps reporting without advancing, they hold for each other's reports, and the error says so: a
loop with no rate cycle as its delay element (6.4, 6.7). The player exits 1. A replay that must be
faithful does not continue quietly without one of its participants; there is no `--drop-dead`.

### 6.2 A slow participant and the effective rate

The player waits, so the replay runs as fast as the slowest participant. When the replay ends, it
prints one line with the effective rate: the data time published over the wall time, paused time
excluded. It also names what set the rate:

- At each chunk barrier the answers are put in time order. The first is charged the time from its
  send; each later one, the time after the answer before it. A participant that holds for an upstream
  is charged only its own increment, not its upstream's wait.
- The participant with the largest total charge set the rate. The line reads
  `set by '<name>' (last to answer <n> of <m> chunks, <s> s of waiting, <p>% of the wall time)`.
- If the `--rate` schedule waited longer than that, it reads `set by --rate <factor>`.
- With no participant it reads `paced by --rate alone`, or, with `--rate max`,
  `not paced, and no participant held it`.

### 6.3 The recorder as a participant

`hypha record --sync` subscribes to `/clock` and `/clock_progress`. It reports under its own node
name and takes part like any node. It reports the newest `/clock` sample once every other participant
it has heard has acknowledged that sample (the rule of 4.3). Before it reports, it drains every
followed topic. By the ordering in 4.2, every output stamped at or before that sample is then in the
recording. The report carries the sample's full clock value.

- The recorder announces itself on `/clock_progress` when it starts, and repeats the announcement
  every 100 ms until it has answered its first sample. A report with send time 0 acknowledges
  nothing, so a player that starts first or later waits for the recorder from its first chunk.
- With `--topics`, the recorder answers no sample before every named topic has a publisher and is
  followed. The recorder looks again every 20 ms. A topic that never appears makes the player stop
  with the recorder's name after `--timeout`. Topics that match only a `--topics` pattern cannot be known in
  advance, and a topic that appears after the first answer is recorded from its first frame seen.
- `--nodes <name>...` makes the first report wait until each named node has been heard. The recorder
  cannot name itself. Without `--nodes`, the first chunk may be acknowledged before a node that has
  not reported yet is heard (6.7).
- A followed topic whose queue overflows stops the recording. The recorder checks its frame sequence
  gaps at each report. It finishes the MCAP file, names the topic and the lost count, and exits with
  an error. A chunk must then hold fewer samples of that topic: lower `hypha play --chunk`. The
  recorder reads each topic through its own queue of 8, so a deeper publisher queue does not help.
- A sample that cannot be recorded (no schema, a failed write) also stops the recording, with the
  first such sample named. The recorder does not acknowledge a chunk after that.
- After the recorder stops, the player waits for it, and stops with its name after `--timeout`.

Without `--sync` the recorder is an observer. It keeps the default queue of 8, reports nothing, and
drops what it cannot record with one warning per topic.

### 6.4 Chained participants

A replay cycle reads from other nodes. Its step at deadline `D` needs their outputs stamped before
`D`, so each upstream node must have run its deadlines before `D`. A step output is stamped with its
deadline. Before a replay cycle runs deadline `D`, each upstream node `u` must have a valid report
with a clock of at least `D - 1 ns`. Otherwise `D` is held, and the cycle runs nothing for it on this
turn.

- **Upstream.** The nodes that publish on one of the cycle's inputs and also publish on
  `/clock_progress`, minus the node itself. The player publishes inputs but never progress, and the
  recorder publishes progress but no cycle inputs, so neither is anyone's upstream. The names are read
  from the transport when the set of publishers changes.
- **The player needs no hold.** A node that reads `/clock = T` already has the player's data
  through `T`, ties at `T` on several topics included, by the rule of 4.3 and with its one exception.
- **Valid report.** In a process that a backward seek restarted (4.4), a report counts only when it
  answers a `/clock` sample sent at or after the one the process rejoined on: an older one describes
  the timeline that the seek left. In any other process the epoch is zero. Send times decide this;
  clock values are compared only against `D - 1 ns`.
- **Triggered cycles.** A triggered cycle's step at `t`, for a trigger sample or a fallback, needs a
  clock of at least `t` from each upstream node, not `t - 1 ns`: the step reads its trigger samples
  stamped at or before `t`, so all of them must have arrived. A tie of two samples from upstream
  nodes stamped `t` then always runs in the same order (`execution-model.md` §6.6). The player is no
  upstream and sends messages stamped `T` after `/clock = T`, so a tie on player data runs in arrival
  order; #219 makes it deterministic.
- **Feedback loops.** Input cuts are strict (`stamp < D`). Two nodes that read each other each need
  only the other's deadlines before `D`, so a loop does not deadlock. Tested with two nodes. A loop of
  a rate cycle and a triggered cycle does not deadlock either: the rate cycle reports `D' - 1 ns`
  while it holds its next deadline `D'`, which is at least the stamp `D` of the trigger it sent.
- **Partial reports** release the deadlines below them, so a node that holds its later deadlines
  does not block its downstream nodes.
- **No timeout in the node.** A held deadline waits for as long as its upstream does not report. The
  player's `--timeout` (5 s by default) is the one bound: it names the participants that do not
  advance and ends the replay (6.1).
- **Scope.** The hold covers the outputs of cycle steps. A plain callback's outputs carry the clock
  when it ran, and their publisher is no upstream (§5).
- **Progress input.** It holds 64 reports between two drains and keeps the newest report of each
  upstream node. A missed report is logged as an error. The next report from that node replaces it.

### 6.5 A publisher on wall time

Unchanged. The `replay time base mismatch` check stays, because it diagnoses exactly this case.

### 6.6 A lost input

An input (queued or latest) that loses samples in replay fails the step. A loss is a transport sequence
gap or a history eviction that the input cursor had not read. The WaitSet run then fails with `SequenceGap`,
and the node logs `replay not faithful: N samples lost on <topic>` at error level, rate-limited per topic.
In replay, a latest input counts every transport gap, as a queued input does, but an evicted sample only at
a step that finds no retained sample before the deadline that no reader rejected. Until then a newer sample
that no reader rejected, retained or held by the input, supersedes the evicted one in data time; a rejection
of it, by the step or by another reader of the history, ends that (execution-model.md §11.1). When a latest
input's history evicted the sample stamped before the deadline that the step was due, and every retained one
is at or after it, the input history is shorter than the replay chunk, and the node logs
`all N retained samples on <topic> are stamped at or after deadline D: the input history is shorter than one
replay chunk`. An input whose first samples come after the deadline logs nothing.

### 6.7 Limits

These are the limits of the built replay. Each is a rule for how a replay is built, not a fault to
wait out.

- **Progress covers cycles only.** A node on simulated time without a cycle reports nothing, so the
  player does not wait for it: a plain callback node, or the ROS bridge. Naming one with `--nodes`
  fails the start after 30 s. Its replay is not deterministic; build the node from cycles, or play
  the launch in the open loop (4.6).
- **A loop of triggered cycles has no delay element.** It holds until `--timeout`, and the player
  names its nodes as a loop (6.1); a rate cycle or a clock driver in the loop is the delay element.
- **Inputs are drained on every turn.** A node's queues hold only what was sent since the `/clock`
  sample it last answered (4.2): one chunk of the player, split to fit the smallest subscriber queue
  (4.3), or the outputs of one step of a clock driver (4.5). What a drain takes waits in the input's
  history, where a loss is an eviction that fails the step (6.6). For an input of a participant that
  the player or the driver waits for, on a topic that it alone publishes, with the topic's rate r, the
  chunk c and the cycle's period p:
  - the queue of a topic of the player needs no size beyond the messages of one instant. A queue of
    c x r samples, one more on the chunk grid, keeps the player from shortening chunks, which costs
    only throughput. The queue of a topic of a clock driver holds the outputs of one driver step;
  - a `Latest` input's history, its queue plus one sample, always holds the newest sample before each
    deadline: every retained sample stamped at or after the deadline came with the `/clock` that ran
    the step, and those fit the queue;
  - a queued input (`Arrivals`) needs a `perStep` of at least p x r. Its history, its queue plus
    `perStep`, then holds what its cycle has not read when a step runs: the samples stamped since the
    previous deadline, at most p x r, and the s samples stamped at or after the deadline that came
    with the `/clock` that ran the step, at most the queue q. A smaller `perStep` drops the oldest
    due samples of a step on purpose, counted in `work_limit_drops_total` and no failure, but only
    while p x r + s ≤ q + `perStep`; past that the history evicts a sample its cycle has not read,
    and the step fails. s depends on how the chunk grid lines up with the deadline grid: at 100 Hz
    with chunks and a period of 100 ms, q = 10 and a `perStep` of 9, a replay from 10 ms (s ≤ 2)
    drops one sample a step, and one from 90 ms (s = 10) fails at 200 ms. Only a `perStep` of at
    least p x r holds on every phase.

  A topic with several publishers gives each one its queue, and a drain empties all of them, so the
  player's queue still holds one chunk. What the other publishers send, cycle nodes upstream
  included, is not split into chunks: size the queue and the history for what they send between two
  turns of the node, or the step fails with `replay not faithful`; a queue that overflows before a
  publisher's first frame that the node accepted loses it uncounted
  (`communication-integrity.md` §3). A `WaitSet` that has not reported yet does not hold its node's
  report (4.2), so its queues are guarded only from its first report.
- **Several publishers on one topic make a replay input depend on turn timing.** The history keeps
  samples in the order the node drained them, and one drain reads the publishers in a fixed order.
  A `Latest` input takes the newest arrival before the cut and a queued input delivers in arrival
  order, so when two publishers send before one step, which sample the step takes, or in which
  order, depends on whether a turn of the node fell between the two sends. Before every turn
  drained, the same held for samples split across two step turns. A deterministic replay needs one
  publisher per topic of a replay input, or publishers whose samples arrive in stamp order.
  `two_publishers_on_one_input_order_by_when_the_node_drained_them` in
  `crates/hypha_ffi/tests/replay_fidelity.rs` pins it.
- **A forward jump of `/clock` costs one step per period.** It runs one step per period it covers (4.4),
  in slices of at most 1024 steps or 50 ms, so a jump across a long gap is slow, but a turn stays short.
- **Simulated time is process-global.** With two sim-time nodes in one process, a backward seek can be
  missed by one of them.
- **An old parameter file is not checked.** A `sim.max_catch_up_periods` key that is still set is
  silently ignored.
- **The first chunk without `--nodes`** may be acknowledged before a node that has not reported is
  heard, so the drain rule of 4.2 does not guard that node's queues for it: a chunk past a queue
  pushes its first frames out uncounted. Name every node that must take part.
- **The upstream hold is tested with two nodes**, including a feedback loop. Chains of three or more
  nodes are not tested.
- **The recorder misses losses it cannot see.** A loss is counted when the next frame of that
  publisher arrives. A loss before a publisher's first accepted frame is not counted. Neither is a loss
  at the end of a publisher's stream, when no later frame follows it.
- **A burst of more than 8 samples of one topic within one chunk** can outrun the recorder's queue at a
  high `--rate`. With `--sync` the recording then stops (6.3). Without it, the samples are dropped.
