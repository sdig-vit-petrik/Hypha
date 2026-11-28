# Execution Model

How Hypha schedules callbacks, timers, cycles, and replay. Applies uniformly to the C++ SDK, Rust SDK, and C ABI.

---

## 1. Quick Map from ROS 1

| ROS 1 | Hypha | Key Difference |
|---|---|---|
| `ros::spin()` | `Node::run()` | Returns `[[nodiscard]] RunResult`; stops at a dispatch boundary on `SIGINT`/`SIGTERM` or `requestStop()`. |
| `ros::spinOnce()` | `Node::runFor(limit)` | No single-step call; pass a positive host-time span (`steadyNow()`). |
| `ros::AsyncSpinner` / `MultiThreadedSpinner` | None | **Hypha creates no threads.** Every callback runs on the thread calling `run()`; callbacks of one `WaitSet` never overlap. |
| `ros::CallbackQueue` | `WaitSet` | One default `WaitSet` per node (`hypha_node_default_waitset`). Samples stay in shared memory until drained. |
| `ros::Timer` | `Node::timer()` | Missed periods are counted, never replayed as a burst. |
| `const Msg::ConstPtr&` | Typed: owned; Raw: borrowed | Typed callbacks receive an owned copy; raw callbacks receive a view valid only during the call. |

---

## 2. Single-Threaded `WaitSet` Loop

```text
run():
  1. Build native attachments once at run entry (attachments freeze during run).
  2. Wait on native WaitSet (notifications, timer deadlines, stop, runFor limit).
  3. Dispatch due timers/cycles first, then a bounded batch of ready ingress.
  4. Check stop/failure and repeat.
```

- **Housekeeping requires a running `WaitSet`:** Latched replay to late subscribers happens on graph events (`hypha/hk/graph/v1`) while `run()` / `runFor()` executes. Schema records need no run (written on endpoint creation).
- **Frozen topology:** Subscriptions and cycles cannot be attached while running (`InvalidState`). Timers are the exception (callbacks may create a timer on host time or a node-owned sim clock, and may `cancel()`/`reset()` timers). Detaching from a callback defers context release until `run()` returns; detaching from another thread during a run is refused.
- **`requestStop()`** is thread-safe and sticky (a stop requested before `run()` returns immediately on the next `run()`).
- **Restarting a `WaitSet`:** Timers count the stopped interval as missed periods (no burst); cycle input histories and sequence state persist.
- **No blocking round trips in callbacks:** Calling blocking schema lookups inside a callback returns `InvalidState`; resolve schemas before `run()`.

---

## 3. Bounded Dispatch

Every turn rotates a fair cursor across ready attachments (rejected frames count against the batch):

| Limit | Default | At Limit |
|---|---|---|
| Registrations per `WaitSet` | 128 | `ResourceLimit` |
| Samples per attachment per turn | 8 | Stays ready; rotation advances |
| Work items before readiness recheck | 64 | Rechecks native `WaitSet` |
| Slow callback warning | 10 ms host time | Counter + throttled warning (≤1/s) |

- **Callback exceptions:** C++ catches exceptions at the C ABI boundary, logs (throttled ≤1/10 s), increments `node.callback_failures`, and continues the run.
- **One delivery style per subscriber:** Either a `WaitSet` callback/latest attachment or manual owning receive (Rust SDK); mixing returns `InvalidOperation`.

---

## 4. Notifications and Wildcards

Every publisher notifies its topic's event service after sending. Whether anything wakes depends on the reader:

| Reader | Wakes on Arrival? |
|---|---|
| Callback subscriber (`subscribe`, `subscribeRaw`) | Yes — wakes `WaitSet` immediately. |
| Triggered cycle's trigger ports | Yes — wakes `WaitSet` immediately. |
| Rate cycle inputs | No — drained at the cycle's tick (or every turn under sim time). |
| Wildcard pattern (`*`) | No event service — polled every **1 ms** (triggers: `min(silence_ms)/10`, ≥1 ms). |

Notifications are wake hints only; the data queue is authoritative. Failed notifications increment `transport.failed_notifications` without failing the publish.

---

## 5. Cycles

A cycle (`CycleNode` or `Cycle`) captures all bound inputs at once, runs `step(const CycleTime&)`, and publishes staged outputs after `step()` succeeds.

### 5.1 Execution and Timing
- **Timers-first:** Due timers and cycles run before new ingress, in registration order. Cycles drain bound queues immediately before capture.
- **Host time:** `CycleTime::now` / `stamp` are wall time at step start (varies with wake-up latency: kernel latency via `timerfd` on Linux; up to ~10 ms coalescing on macOS). `dt` is monotonic elapsed time; `missed` is the per-step delta of missed periods (no catch-up burst).
- **Decoding & outputs:** Undecodable samples are counted (`RequiredInput.undecodable`), logged (≤1/10 s), and skipped without failing the run. `Output::send()` serializes into pre-allocated `perStep()` loans and flushes in call order on return.
- **Failure semantics:**
  - Step throws → iteration fails (`Panic` or `ResourceLimit` on `bad_alloc`); no outputs sent.
  - Emission limit or payload bound exceeded → iteration fails; no outputs sent.
  - `PoolExhausted` on output → run fails unless output is tagged `diagnostic` (drops & counts `output_pool_drops`).
  - The `WaitSet` is one failure domain; a terminal error stops all cycles on that `WaitSet`. Captured input ranges are not replayed on failure.

### 5.2 Input Supervision
Evaluated below the C ABI (`hypha_cycle`) once per step before `step()` runs:
- **Staleness (`silence_ms`):** Evaluated on host monotonic time (or replay cut under sim time). A sample is usable while `age <= silence_ms` (inclusive). A port with no sample counts from first evaluation. Late samples already past `silence_ms` are rejected (`age_rejections_total`) and do not clear staleness.
- **Stagnation (`stagnation_ms` + `setAlive()`):** For relayed sources (drivers/bridges) whose relay keeps sending stale payloads. Runs on `CycleTime::scheduled_nanos` (monotonic in host mode, sim time in replay) and only while not stale. Any change in the alive counter (including wrap) resets it.
- **Ignoring (`setIgnored(true)`):** Hides faults from `stale()`/`stagnant()` and `inputsOk()`; raw faults remain visible in `bindingHealth()` and `~/state`.
- **State-change logs (`hypha` module, logged once per transition; advisory inputs log at INFO):**
  `<in> is usable` | `is stale: newest sample is <age> ms old, silence_ms = <limit>` | `never arrived: no sample for <age> ms, silence_ms = <limit>` | `is stagnant: alive value unchanged for <t> ms, stagnation_ms = <limit>` | `is usable again` | `fault is ignored` | `fault counts again`.

### 5.3 Queued Work and Overflow
`Arrivals` ports examine up to `perStep` samples per step (capped jointly by `[cycle] arrival_budget` across non-idle queued ports). Excess pending samples drop **oldest first** (`work_limit_drops_total`). Separate counters track history overwrites (`lost()`), age rejections (`age_rejections_total`), and overflow drops (`work_limit_drops_total`).

### 5.4 Triggered Cycles
Configured via `setTrigger(arrivalsPort)` (up to 64 triggers; mutually exclusive with rate; each trigger requires `silence_ms`; no two triggers may share a topic):
- **Ordering:** One step per trigger sample across all triggers, ordered by `(framework_send_stamp, port_index, arrival_order)`. Only the firing trigger's `samples()` is non-empty. Triggers bypass `perStep`/`arrival_budget` discard.
- **Step time & cuts:** Under sim time, `CycleTime::now` is the trigger's stamp `T` (other inputs cut strictly `< T`). Under host time, `now` is wall time at step start (non-trigger `Arrivals` take samples sent `<=` trigger's host send time).
- **Fallback:** If no trigger fires for `min(silence_ms)`, a fallback step runs with all triggers empty (repeating every `min(silence_ms)`).
- **Loops:** A feedback loop of triggered cycles alone has no delay element and deadlocks under lockstep until `--timeout`; include at least one rate cycle or clock driver.

### 5.5 TF Input (`hypha::Transforms`)
Captures `/tf` and `/tf_static` (depth 32) before each step (cut `< T` under sim time). Refused transforms (non-finite, non-unit quaternion, second parent, cycle, mixed static/dynamic, invalid stamp) increment `tf.refused` and cause lookups on affected chains to return `TransformError::Refused`. Under host wall-clock jumps backward exceeding cache length, older transforms for that frame are dropped. Under sim time, `/tf` queue overflow or >32 pending samples fails with `SequenceGap`.

---

## 6. Timers and Simulated Time / Replay

### 6.1 Plain Timers vs. `CycleTimer`
- `Node::timer()` uses `timerfd` on Linux (`select` on macOS) in host mode and `/clock` in sim mode. Never bursts missed periods.
- `hypha::CycleTimer` (`<hypha/cycle_timer.h>`) is polled inside `step()` against `CycleTime::now` — deterministic in replay and `StepHarness`.

### 6.2 Replay Cycles (Simulated Time)
Under `HYPHA_USE_SIM_TIME=1`, rate and triggered cycles execute deterministically in data time ([replay-flow-control.md](replay-flow-control.md)):
- **Grid & cuts:** Rate-cycle deadlines lie on whole multiples of the period strictly after the first received clock value. Each step at `D` runs with `now = D`, `missed = 0`, and admits samples stamped strictly `< D`. Forward `/clock` jumps run every expired deadline (sliced at ≤1024 steps / 50 ms per turn).
- **Lockstep progress:** Nodes with cycles publish `hypha_msgs/ReplayProgress` on `/clock_progress` after each turn (draining replay inputs first). A cycle holds deadline `D` until every participant upstream reports `>= D - 1 ns` (`>= D` for triggered cycles).
- **Faithfulness:** Any transport overflow or unread history eviction on a replay input fails the run with `SequenceGap` (`replay not faithful`) in lockstep mode, or warns in open-loop mode (`HYPHA_REPLAY_LOCKSTEP=0`). Inputs tagged `hostClock` (`Latest` only) bypass replay cuts and use host freshness.
- **Rewinds:** Simulated time moving backward ends the run with `ClockRewound` (exit **75** under `runNode`) so the supervisor restarts the process cleanly; nothing rewinds in place.
- **Plain callbacks in replay:** Plain subscriptions and `Node::timer()` do not participate in lockstep, publish no `/clock_progress`, and run non-deterministically as samples arrive. Use cycles for deterministic replay, or `HYPHA_REPLAY_LOCKSTEP=0` for open-loop playback.

---

## 7. Troubleshooting: When a Callback Never Runs

1. Is `node.run()` / `node.runFor()` actually running? (Nothing runs in the background.)
2. Was the subscription created *before* `run()`? (Attaching during a run returns `InvalidState`.)
3. Is the `Subscriber` object still in scope? (Destroying it detaches the callback.)
4. Do topic names and message MD5s match? (A subscriber with no matching publisher warns after 5 s; check `hypha topic list`.)
5. Is it a rate-cycle input? (Rate-cycle inputs drain at ticks, never on arrival.)
6. Did `run()` return an error? (Inspect `RunResult::status()` and `reason()`.)
