---
name: ros1-to-hypha-node
description: Migrate one ROS 1 (noetic) C++ node to a production-ready hypha node. Keeps the node logic, its formatting, its comments and its tests. Replaces only the ROS glue with a cycle that reads its topics and parameters from one TOML file.
---

# Migrate a ROS 1 node to a hypha node

This guide is a procedure for a coding agent. Do the steps in sequence. Each
step has a check. Do not start the next step before the check passes.

The result is a hypha node that:

- runs its work in a **cycle node** (a class derived from `hypha::CycleNode`),
- reads **every** topic name from the `[ports.inputs.<name>]` and `[ports.outputs.<name>]` tables and every tuning value
  from one TOML file,
- keeps the original logic, formatting, comments and tests,
- reports missing and stale inputs explicitly, and
- passes the checks in [section 9](#9-production-ready-checklist).

Read these before you start: [cpp-api.md](cpp-api.md) §2, §5.4 (cycle node), §9
(parameters), §11 (logging), [execution-model.md](execution-model.md) §2 and
§3. The reference for a cycle node is
[examples/scan_cycle](../examples/scan_cycle/README.md): its entry point, the contract
of each input, and a test at each of three levels. The reference for an `ApplicationNode` is
[examples/sensor_pipeline](../examples/sensor_pipeline/README.md) (section 10).

A node is a `hypha::CycleNode` with its ports as members, started by
`hypha::runNode<App>()` (see [cpp-api.md](cpp-api.md) §5.4). A node that
owns worker threads, or a shim of a ROS client library, is a
`hypha::ApplicationNode`, started by the same call ([section 10.1](#101-which-target)).

**Which target, when two sentences seem to apply.** Decide in this order and stop at the
first yes:

1. The node owns a worker thread (a blocking device read, a vendor SDK that calls back
   from its own thread) or is a shim of a ROS client library: an `ApplicationNode`.
2. The node steps on a timer, or on the samples of its inputs, including a node that
   publishes from the callbacks of several inputs: a `CycleNode` (a rate, or one trigger
   for each input).
3. The inputs are named by a list in the parameter file: still a `CycleNode`. A
   `CycleNode` is a `hypha::Cycle`, so its constructor builds the ports from the list and
   keeps them in a `std::list` (`ports/twist_mux/src/twist_mux.cpp` in the playground).
   A free-standing `hypha::Cycle` ([cpp-api.md](cpp-api.md) §5.5) is for a node with two
   cycles, or ports that are not members of a class.

The sentence "use `hypha::Cycle` when ports are built at run time" of cpp-api.md §5.2 and
§5.5 names the class that holds the ports, not a third target.

A path that starts with `ports/` names a file of the playground repository
(`Hypha-playground`), where the native ports that found these rows live; a port that is
built against the installed SDK can read it as a worked example.

## Contents

- [1. When to use this guide](#1-when-to-use-this-guide)
- [2. Rules](#2-rules)
- [3. Procedure](#3-procedure): [Step 1: inventory](#step-1-make-the-inventory),
  [Step 2: design the cycle](#step-2-design-the-cycle),
  [Step 3: the TOML file](#step-3-write-the-toml-file),
  [Step 4: build](#step-4-replace-the-build-glue),
  [Step 5: node glue](#step-5-replace-the-node-glue),
  [Step 6: tests](#step-6-port-the-tests),
  [Step 7: run and verify](#step-7-run-and-verify-on-the-bus),
  [Step 8: deploy](#step-8-deploy)
- [4. ROS 1 to hypha map](#4-ros-1-to-hypha-map): one row for each ROS 1 call
- [5. Message types and the minimal logic edits](#5-message-types-and-the-minimal-logic-edits):
  time, third-party headers, logging
- [6. Traps](#6-traps)
- [7. Stop and ask](#7-stop-and-ask): what hypha has no answer for yet, with the issue:
  [7.1 services](#71-services), [7.2 actions](#72-actions), [7.3 diagnostics](#73-diagnostics),
  [7.4 waiting transform lookup](#74-a-transform-lookup-that-waits),
  [7.5 run-time parameters](#75-a-parameter-that-changes-at-run-time)
- [8. Report hypha gaps and blockers](#8-report-hypha-gaps-and-blockers)
- [9. Production-ready checklist](#9-production-ready-checklist)
- [10. Porting an event-driven node: an `ApplicationNode`](#10-porting-an-event-driven-node-an-applicationnode):
  [10.1 target](#101-which-target), [10.2 application](#102-the-application),
  [10.3 topic names](#103-literal-topic-or-port-name), [10.4 time and logging](#104-time-status-and-logging),
  [10.5 queue depth](#105-queue-depth), [10.6 threads](#106-threads), [10.7 TF](#107-tf-in-an-application),
  [10.8 lists](#108-a-list-of-scalars-in-the-parameter-file), [10.9 example](#109-a-runnable-sensor-node-and-its-replay-check),
  [10.10 tests](#1010-test-an-application), [10.11 N inputs from a list](#1011-n-subscriptions-named-by-a-list)
- [11. Porting a ROS 2 node](#11-porting-a-ros-2-node): [11.1 message names](#111-the-message-names),
  [11.2 the rows](#112-the-rows)
- [12. Large messages](#12-large-messages): [12.1 sizing](#121-sizing-a-topic),
  [12.2 size not known](#122-a-message-whose-size-the-node-cannot-state),
  [12.3 copies and zero-copy](#123-copies-and-zero-copy-today)

---

## 1. When to use this guide

Use it for one ROS 1 C++ node that subscribes, computes and publishes, with or
without timers, parameters and services. The procedure ends in a `CycleNode`:

- A node whose work runs from a timer becomes a **rate cycle** (`rate_hz`).
- A node whose work runs on each sample of one or several inputs becomes a **triggered
  cycle**: `setTrigger(port)` on the `Arrivals` port of each of those inputs, one step
  per sample at the sample's time ([Step 2](#step-2-design-the-cycle)).
- A `hypha::ApplicationNode` is only for a node that owns worker threads and a shim of a
  ROS client library: see
  [section 10](#10-porting-an-event-driven-node-an-applicationnode).

Do not use it, and ask the user first, when the node needs one of the items in
[section 7](#7-stop-and-ask). hypha has no equivalent for them.

Migrate one node at a time. The other nodes stay in ROS 1 and connect through
`hypha_ros_bridge` (`apps/`) until they are migrated.

---

## 2. Rules

These rules apply to every step.

### 2.1 Small diff: replace the glue, keep the logic

Sort every line of the ROS node into one of three classes:

| Class | What it is | What you do |
|---|---|---|
| **Logic** | Computation, state, algorithms, checks, the classes that hold them. | Keep each line as it is. A check that decides whether an input may be used is supervision, not logic: see Step 2. |
| **Glue** | `ros::init`, `NodeHandle`, `subscribe`, `advertise`, `publish`, `createTimer`, `param`, `spin`, `ROS_*` log macros, ROS includes, catkin. | Replace with the hypha equivalent. |
| **Tests** | gtest, rostest, test data, launch files used by tests. | Keep each test case. Port only the harness. |

- **Do not reformat.** Do not run a formatter on a file you only touched. Do
  not change indentation, brace style, line breaks, names or the sequence of
  functions. Format only new files, and only the lines you changed in old files
  (for example `git clang-format` on the changed lines), with the
  configuration of the repository that owns the node.
- **Keep every comment.** When the line under a comment changes, keep the
  comment. Change its text only when it now states a false fact (for example it
  names a ROS call that is gone). A comment on glue that is removed moves to the code
  that replaces that glue, when that code does what the comment says (step 5 does this
  for `speedCallback`). A comment that no remaining code matches goes with
  its glue, and the change description lists it.
- **Do not refactor the logic.** Do not rename, extract, inline or "clean up".
  Change a logic line only when it does not compile against hypha types
  ([section 5](#5-message-types-and-the-minimal-logic-edits)), and then change
  only the token that fails.
- **Do not delete a test.** If a test cannot run under hypha, keep it, and
  report it to the user with the reason.
- **Supervision or logic.** A check whose result decides whether an input may
  be used (`now - stamp > timeout`, then ignore, hold or expire) is supervision.
  Replace it with the port's `silence_ms`. A use of the source time as data
  (interpolation, latency compensation, ordering of samples, a computed delay) is
  logic. Keep it.
- **Small, not smaller than correct.** The diff must still use the cycle, the
  `[ports.inputs.<name>]` and `[ports.outputs.<name>]` tables, the ports' `silence_ms` limits and the `~/state` report that `runNode` publishes. A diff
  that keeps a ROS-style timer and a hand-written age check to save lines is
  not a migration.

### 2.2 TOML is the one source of configuration

- Every topic that the node reads is a `[ports.inputs.<name>]` table with a `topic` key, and every
  topic that it writes is a `[ports.outputs.<name>]` table with a `topic` key. Code
  reads it through the port: `hypha::Latest<T> speed{*this, "speed"}` reads
  `[ports.inputs.speed]`. A port has **no code fallback**: a missing entry then
  stops the cycle commit with a message.
- Every tuning value is a top-level TOML key. Read it with
  `hypha::config().required<T>("<key>")`. Use `hypha::config().optional<T>(key, fallback)`
  only for a value whose fallback is safe in every deployment. The cycle rate is
  `rate_hz` under the `[cycle]` table.
- The node instance name is `[node].name`. Its own topics are `~/<name>`.
- Logging is the `[log]` table. There is no other configuration file and no
  environment variable for it. **Set `level = "info"` in a ported node.** The default
  `level` of a hypha node is `debug`, and its console takes `trace` (`console_level`,
  [cpp-api.md](cpp-api.md) §11), where ROS 1 shows INFO and above. A kept
  `HYPHA_LOG_DEBUG` on the data path then writes one line for each message (10,428
  lines in 30 s at 400 Hz for `twist_mux`).
- `HYPHA_USE_SIM_TIME=1` in the node's environment replaces the ROS parameter
  `use_sim_time`. It holds `1`, `true`, `0` or `false`; any other value stops node
  creation. The parameter file has no such key: a `use_sim_time` key is an ordinary
  key, which a C++ node refuses at start when no reader asks for it.

### 2.3 Fail restrictive

- An input that is missing or stale reaches the step as an explicit verdict.
  The step chooses the response. hypha never invents a safe output, and you
  must not invent one either: ask the user which output is safe when the ROS
  node had no rule for it.
- A value that is out of range stops the node when the constructor or `setup()`
  throws, or when `required<T>(key, constraint)` refuses it (`hypha::constraints::positive()`,
  `finite()`, `range(a, b)`). `hypha::runNode()` logs it. A `hypha::ParameterError` or
  `hypha::ConfigError` returns 78, the configuration code that a supervisor does not
  restart; any other exception returns 70.
- A parameter key that nothing read stops the node at start. Two consequences:
  - **A ROS parameter file cannot be reused as it is.** Every key the node does not read
    (`odom0_nodelay`, `print_diagnostics`, `publish_tf`, a key of a branch the code does
    not take) is refused with exit 78 and `unread parameter(s): ...`. Copy only the keys
    the ported code reads. `hypha param validate` does not catch it: it checks that the
    file loads, and the unread-key check runs only when the node starts
    ([tools.md](tools.md), `hypha param`). Start the node with the file to test it.
  - **A key that the file sets and the code chose not to read** (the unselected branch of
    a vendor file: three source blocks of which the code reads one) is not removed. Name
    it: `hypha::config().ignore("lidar_udp_type", "source_type does not select it")` in
    code, or a top-level `unused = ["lidar_udp_type"]` in the file. The manifest then
    lists the key as ignored with its reason, and a typo beside it is still refused
    ([cpp-api.md](cpp-api.md) §9, "Keys set on purpose and not read").
- More than one command-line argument stops the node.

### 2.4 Code style

New C++ follows [AGENTS.md](../AGENTS.md) §5: explicit control flow, named
complex conditions, `this->`, no `auto` where the type matters, named functors
instead of stored lambdas. These rules apply to the code you write, not to the
logic you keep.

---

## 3. Procedure

### Step 1: Make the inventory

Read the ROS node, its launch files, its YAML parameter files and its tests.
Write this table in your working notes. Do not write code yet.

| Item | Record |
|---|---|
| Subscriptions | Topic, type, queue size, callback, what the callback does: *stores the latest value* or *does work for each message*. |
| Publications | Topic, type, where it is published (callback or timer), latched or not. |
| Timers | Period, callback. |
| Parameters | Name, type, default, where it is read. Values from launch and YAML files. |
| Remaps and namespaces | Every `<remap>`, `ns=`, relative topic name. |
| Services | Name, type, server or client, timeout. |
| Time | Use of `ros::Time::now()`, `header.stamp`, `ros::Duration`, `use_sim_time`. |
| Own staleness checks | Every place the code compares a message time with now. |
| TF | Broadcaster, static broadcaster, listener. |
| Tests | Each test file, its framework, and whether it needs a ROS master. |
| Items of [section 7](#7-stop-and-ask) | Each one. |

**Check:** every subscription, publication, timer and parameter has one row.
If the table has an item of section 7, stop and ask.

### Step 2: Design the cycle

Map the inventory to one cycle for each timer rate or for its event inputs. A node with
one timer has one rate cycle. A node with no timer, whose work runs in the callbacks of
one or several subscriptions, has one triggered cycle with those inputs as its
triggers. One process has one `CycleNode`; a second rate is a composed `hypha::Cycle`
with its own name.

| ROS 1 | hypha cycle |
|---|---|
| Timer callback that computes and publishes | The `step()` method of the cycle node, at `rate_hz` under `[cycle]` in the TOML file. |
| Each subscription whose callback computes and publishes, in a node with no timer | A `hypha::Arrivals<T>` member made a trigger with `setTrigger(port)` in `setup()`, one call per such subscription, and no `rate_hz`. `step()` runs once for each sample of any trigger: the port that fired holds that sample alone in `samples()`, and every other trigger holds none. The step calls the original callback of the port whose `samples()` is not empty. |
| Subscription whose callback only stores the latest value | A `hypha::Latest<T>` member. |
| Subscription whose callback does work for each message (counts, integrates, buffers, filters) | A `hypha::Arrivals<T>` member. The step calls the original callback once for each sample, in order. |
| Publisher | A `hypha::Output<T>` member. Call `send(message)`. |
| Member variables of the node class that the logic changes | Members of the cycle node. Use the original logic class as a member when one exists. |
| Constant tuning (`gain`, limits) | Members of the cycle node, read from TOML in the constructor or `setup()`. |
| Own staleness check (`now - stamp > timeout`) | Replace it with the port's `silence_ms` (carry the old timeout's value) and read `usable()` or `stale()`. Delete the timeout parameter. A use of the source time as data stays (logic). |
| A store-then-take buffer between callback and timer (input image, slot, double buffer) | Delete it: the `Latest` port is the buffer. Read `*port` when `fresh()`; `lastKnown()` holds the old value. |

`silence_ms` judges the age from the time the sender sent the sample, not from
`header.stamp`. When the old timeout and `silence_ms` differ, the old timeout
decided the behaviour: carry its value. A node that needs the age of the
measurement computes it from the payload, as logic.

A `Latest` port gives the same value again in each step while it is usable.
That is correct for a callback that only stores a value. A callback with an
effect for each message (a counter, an integrator, a queue) must use an
`Arrivals` port, or it runs too often.

**A triggered cycle** ([execution-model.md](execution-model.md) §6.6,
[cpp-api.md](cpp-api.md) §5.4, "Triggered cycles"). The reference is
[examples/mrs/managers/estimation_manager.cpp](../examples/mrs/managers/estimation_manager.cpp):

- The trigger port, `hypha::Arrivals<nav_msgs::Odometry> ground_truth`, is made the
  trigger in `setup()` with `this->setTrigger(this->ground_truth)`. Its table in
  `examples/mrs/config/estimation_manager.toml` has a `silence_ms` (100), and no
  `[cycle]` table sets `rate_hz`: a rate and a trigger exclude each other.
- Each trigger needs its own `silence_ms`, also when the port is `unsupervised`;
  `start()` fails without one and names the port. Carry the old timeout of the input
  when the node had one, else the longest gap the input may have.
- When no step ran for the shortest `silence_ms` of the triggers, one **fallback step**
  runs with every trigger's `samples().empty()`, and each trigger past its own limit
  `stale()`; it repeats every shortest `silence_ms`. Handle it in `step()`:
  `estimation_manager` publishes no estimate and logs the input as stale.
- In every step each trigger is judged by its newest sample against its own limit, so
  `usable()` of a trigger that did not fire says whether its last sample is still
  valid. A trigger that may be idle for long, such as a joystick, is `advisory`, so that
  its silence does not degrade `~/state`.
- On simulated time a step runs at the send time T of its trigger sample, sees every other
  input cut before T, and its outputs carry T. On host time its time, and the stamp of its
  outputs, is the wall time at the start of the step, later than the sample's own stamp
  (execution-model.md §6.6 item 2). The other subscriptions become `Latest` or
  `Arrivals` ports as in the table above, and a TF listener a `hypha::Transforms` port.

A node with several inputs whose callbacks each publish is one triggered cycle with
one trigger per input. Samples at one instant step in the construction order of the
ports, so a node that chooses between its inputs, as `twist_mux` does by priority,
decides with `usable()` of every trigger and forwards only the sample of the port that
fired; its output is then the same in either order
(`crates/hypha-cxx/test/test_twist_mux_shape.cpp`).

**Shapes the table above does not name.**

*A timer that publishes, and measurements from queued subscriptions* (`robot_localization`:
an EKF publishes at `frequency` and integrates the samples of N inputs in stamp order). A
rate cycle with one `Arrivals` port for each measurement input. Each step receives the
samples that arrived since the previous step, at most `perStep` for each port. The node's own
queue ordered by `header.stamp` is logic: keep it. Hypha does not order samples across steps;
`hypha::visitArrivalsInOrder(stampOf, visit, portA, portB, ...)`
(`hypha/cycle_arrivals_merge.h`) visits the samples of one step, for a fixed list of ports,
in the order of the stamp that `stampOf` returns. A sample whose `header.stamp` is older than
the state the node already computed is the ROS node's own rule: hypha does not drop it
(`silence_ms` judges the send time, not `header.stamp`), so the upstream rule stays as
logic. `robot_localization` kept upstream's: a sample older than the previous one of the same
input is dropped with a warning, one older than the state but from another input corrects
the filter without moving the state back (`ports/robot_localization/src/ros_filter.cpp` in
the playground). When the ROS node had no such rule, ask the user.

*A second timer in the node, and a timeout that a callback starts.* A cycle has one rate or
one set of triggers, and no second timer. Issue #267 proposes a helper owned by the cycle
(`every(period)`, `after(duration)`); **it does not exist**, and a port does not write
`hypha::every`. What the MRS ports did, and what a port does until #267 is decided:

- A body that runs at a period longer than the step's (a 10 Hz timer in a 100 Hz step), or a
  one-shot timeout, is a deadline kept in a member and compared with the step's own time
  `time.now`, and not with `hypha::steadyNow()` or a `std::chrono` clock, so that a replay
  behaves as the live run (under simulated time `time.now` is the replay's time). The body runs
  once for each period that passed, as the thread of a `ros::Timer` would have, so the
  deadline counts periods and does not set a flag. The grid is anchored at the first call
  and gives no tick then. `Ticker` of `ports/mrs_trackers/inc/mrs_uav_managers/ticker.h`
  (with `test_ticker.cpp`), about 30 lines:

```cpp
class Ticker
{
  public:
    explicit Ticker(hypha::Duration period) : period_(period)
    {
        if (period.nanos() <= 0) {
            throw std::invalid_argument("a ticker period must be positive");
        }
    }

    // How many instants of the grid lie in (the previous poll, now]. The first poll
    // anchors the grid at now and gives none.
    uint32_t poll(hypha::Time now)
    {
        if (!this->due_.has_value()) {
            this->due_ = now + this->period_;
            return 0;
        }
        uint32_t ticks = 0;
        while (now >= *this->due_) {
            this->due_ = *this->due_ + this->period_;
            ticks++;
        }
        return ticks;
    }

  private:
    hypha::Duration            period_;
    std::optional<hypha::Time> due_;
};
```

  Poll it at the start of the step, before the code that reads its result: the order is the
  difference between an equal result and a failure (`mrs_trackers` failed at step 70 with the
  read before the tick). A timeout that a callback started (`concatenate_and_time_sync`
  creates one per group) is a `std::optional<hypha::Time> deadline_` set to
  `time.now + timeout` where the callback would have created the timer, and checked at the
  top of every step.
- In a **triggered** cycle a step runs only for a sample or for the fallback step (every
  shortest `silence_ms`), so a deadline is seen at the next such step, up to that late. When
  that is too coarse for the node, stop and report it as a gap (issue #267) with the period
  and the lateness the node can take.
- An `ApplicationNode` has `node.timer(period, callback)` for any number of timers, and a
  callback may create one on host time ([cpp-api.md](cpp-api.md) §3). Under simulated time
  it takes no part in the replay lockstep ([section 10.9](#109-a-runnable-sensor-node-and-its-replay-check)).

*A manager that hosts plugins (`pluginlib`).* Hypha has no class loader and no plugin
description file: no `PLUGINLIB_EXPORT_CLASS`, no `plugins.xml`. The shape decided for the
MRS ports is one process with a manager cycle that calls **plain classes, linked statically
and chosen by name from the TOML file**. The manager is a `CycleNode` (usually triggered, by
the sample the plugins act on).

- The plugin base class keeps its virtual methods. The manager builds each plugin from one
  function with one `if` for each class; the host names every plugin it can hold, so a new
  plugin is an edit of that function, a list a safety review can read. A name that no `if`
  knows is a `hypha::ConfigError` (exit 78), and so is a name listed twice
  (`makeTracker` and the constructor of `ports/mrs_trackers/src/tracker_host.cpp`):

```cpp
std::unique_ptr<Tracker> makeTracker(const std::string& name)
{
    if (name == "LineTracker") {
        return std::make_unique<LineTracker>(
            LineTrackerConfig::read(hypha::config("mrs_uav_trackers.line_tracker"))
        );
    }
    throw hypha::ConfigError("trackers: no tracker is named '" + name + "'");
}
```

```toml
trackers = ["NullTracker", "LineTracker"]   # read with requiredArray<std::string>("trackers")
[mrs_uav_trackers.line_tracker]
```

- **The plugin builds no topic and reads no parameter file.** A plugin that made its topic
  names from its own parameters (`"/" + uav_name + "/hw_api/imu"`) loses that code: the
  manager owns every port, with its topic in `[ports.inputs.<name>]` (the topic is a TOML
  value, so the `uav_name` prefix is written in it), and hands the value to the plugin's
  `update()` as an argument. The plugin's interface changes for that, and a plugin's
  parameters arrive as a typed config struct that the manager reads
  (`LineTrackerConfig::read(hypha::config("..."))`; a nested table is a dotted name, see
  Step 3). A plugin's own publisher becomes an output of the manager that `update()`'s
  result feeds; a plugin's own timer is a `Ticker` as above.
- **`ros::Time::now()` inside a plugin is the step's time.** The manager gives the plugin
  the `hypha::CycleTime` of the step (in a small aggregate that can grow without changing
  every signature: `struct StepContext { hypha::CycleTime time; };` in
  `ports/mrs_trackers/inc/mrs_uav_managers/step_context.h`) and the plugin reads
  `context.time.now` for logic and `context.time.stamp` for a message it fills. It does
  not call `hypha::now()`: in a started cycle that returns the step's time, but in a
  `StepHarness` step it returns the process clock ([cpp-api.md](cpp-api.md), "Test a step
  without a bus"), so a plugin that calls the global clock cannot be tested without
  `enableSimTime`, and its `Ticker` would run on the process clock.
- A plugin that serves a ROS service to its caller (`switch_tracker`, `takeoff`): no native
  answer exists, see [section 7](#7-stop-and-ask), "Services" (issue #132).

**Check:** each subscription has one input port, and each publication has one
output port.

### Step 3: Write the TOML file

Put the file beside the node, for example `config/<node>.toml`. Copy every
scalar value from the ROS launch and YAML files. Use the TOML type of the value: a
number is a number, not text. A YAML list of numbers, flags or text maps to a TOML array,
read with `hypha::config().requiredArray<T>(name)` or `optionalArray<T>(name)`
([section 10.8](#108-a-list-of-scalars-in-the-parameter-file)); a YAML list of maps maps to
a list of tables, `[[name]]`, read with `hypha::config().list(name)`
([cpp-api.md section 9](cpp-api.md#9-parameters)). `list(name)` on an array of scalars
reads the elements with the key `""`; use the array readers for it.

```toml
gain = 0.5

[cycle]
rate_hz = 100.0

[node]
name = "speed_controller"

# silence_ms: older than this after it was sent, the input is stale.
[ports.inputs.speed]
topic = "/odom_speed"
silence_ms = 200

[ports.inputs.setpoint]
topic = "/speed_setpoint"
silence_ms = 200

[ports.outputs.command]
topic = "~/command"

[log]
level = "info"
console = true

[log.sink.main]
path = "logs/speed_controller.log"
rotation_mb = 5
rotation_count = 3
```

- A topic is absolute (`/odom_speed`) or private (`~/command`, which resolves
  to `/<node name>/command`). A relative ROS name (`odom_speed`) is refused:
  apply the ROS namespace and remaps by hand and write the full result. **What the name
  rule refuses** (`hypha::validateTopicName`; `crates/hypha_core/tests/topic_spec.rs`,
  `crates/hypha_ffi/tests/topic_name_validation_abi.rs`): a name without the leading `/`
  (`cmd_vel`), the empty name, `/` alone, `/hypha` alone (the product name is reserved:
  applications publish below it), a `*` (a wildcard belongs to a subscription pattern
  only: `/a/*` for one segment, `/a/**` for the rest), a space, tab, newline or other
  control character, an empty segment (`/a//b`, a trailing `/`), a `.` or `..` segment,
  and `~` alone or `~x` (the form is `~/x`). `/a/b_c/D~9` is valid. A refused name is
  `InvalidTopic`, a refused configuration (exit 78).
- An input that the deployment does not wire has an empty `topic` in
  `[ports.inputs.setpoint]` (`topic = ""`), and the port is declared with
  `optionalTopic`: `Latest<Float64> setpoint{*this, "setpoint", optionalTopic}`. The
  input is then disabled. Any other port fails the start when its table or topic is
  missing or empty. Add `advisory` when node health must not judge the input.
- A relative log path resolves against the directory of the TOML file.
- `${NAME}` in any string, `[log]` and `include` included, expands from the
  environment. An unset or empty variable stops the node.
- A value that belongs to one input, such as the priority of each input of a multiplexer
  (`priority = 100` in `[ports.inputs.joystick]`), is a key of its table beside `topic` and
  `silence_ms`. The node reads it with `hypha::config("ports.inputs.joystick")` in its
  constructor or `setup()`, before the cycle starts; a key of an input table that nothing
  has read by then fails `start()` as an unknown key, a typo of `silence_ms` included.
  Do not repeat the input in a table of the node.
- Port names are unique per direction. Each `[ports.inputs.<name>]` is one table, so two
  inputs cannot share a name, and the same holds for `[ports.outputs.<name>]`. When an
  upstream configuration uses one name for two inputs (a velocity input and a lock both
  called `joystick`), rename one of them in its TOML table and in its port declaration,
  for example `joystick_lock`.

- A nested group (`camera_matrix/rows`, `camera_matrix/data` in a YAML file) is a nested
  TOML table, and `hypha::config("calibration.camera_matrix")` is a view of it, with the
  prefix as a dotted name. The keys read through the view count as read:

```toml
[calibration.camera_matrix]
rows = 3
cols = 3
data = [2300.0, 0.0, 1224.0, 0.0, 2300.0, 1024.0, 0.0, 0.0, 1.0]
```

```cpp
const hypha::ConfigView table = hypha::config("calibration.camera_matrix");
table.required<int64_t>("rows", hypha::constraints::range(3, 3));
const std::array<double, 9> data = table.requiredArray<double, 9>("data");
```

  (`ports/arena_camera/src/camera_info.cpp`, `readMatrix`; tests `Calibration.*` in
  `test_parameters.cpp`.) An array inside an entry of a table list, and a list of lists,
  are refused ([cpp-api.md](cpp-api.md) §9).
- **A ROS timeout that has no value.** The old timeout decides `silence_ms`
  ([Step 2](#step-2-design-the-cycle)); when the ROS node had none:
  - *The input has a documented rate or a launch parameter for its age*: that is the old
    timeout; carry it.
  - *No timeout and no documented rate*: ask the user for the longest gap in normal
    operation, in milliseconds, and write the answer in the change description. A rate that
    you measured on a recording or with `hypha topic hz` is evidence for the question, not
    the answer, and a number copied from another node is not either. Do not write a guess
    into the TOML file; mark the open key with one comment that names the question
    (section 8).
  - *`timeout: 0.0`, "not used", or a lock that never expires* (`twist_mux`): the input has
    no age limit. Declare the port `hypha::unsupervised` and give its table **no**
    `silence_ms` (0 is refused, and a limit is refused on an `unsupervised` port).
    `unsupervised` does not mean "need not arrive": see the next item.
- **`advisory` against `unsupervised`.** They answer different questions
  ([cpp-api.md](cpp-api.md) §5.3). `unsupervised`: the input has no age limit, and it still
  counts for health. An `unsupervised` input that has not yet delivered a sample has the
  verdict `NeverArrived` (`usable()` is false), after its first sample it is usable for good,
  and until then `~/state` stays `WAITING`: the node runs normally while the state says it
  waits (`mrs_trackers` held `~/state` at 1 with the UAV state arriving at 100 Hz, because
  two `unsupervised` command inputs had never received a message). **For an input that may
  never arrive** (an operator's command, a joystick, a lock that nobody sets) declare
  `hypha::advisory`: the node is then healthy without it, its silence does not degrade
  `~/state`, and it may still have a `silence_ms`. The two tags exclude each other.
- **A tag from a run-time value.** Every tag takes a flag: `hypha::unsupervised(!expires)`,
  `hypha::advisory(!required)`, `hypha::optionalTopic(flag)` (`hypha/options.h`). A port
  whose limit is known only when the file is read declares it with one constructor:

```cpp
LockInput(hypha::Cycle& cycle, const std::string& name)
    : expires(hypha::config("ports.inputs." + name).optional<int64_t>("silence_ms").has_value())
    , port(cycle, name, hypha::unsupervised(!this->expires))
{
}
```

  The application may read the `silence_ms` of its own table, as here
  (`ports/twist_mux/src/twist_mux.cpp`); a misspelt framework key is still refused.
- **A transport hint has no counterpart.** `ros::TransportHints().tcpNoDelay()`, `udp()`, and
  the parameters that set them (`odom0_nodelay`) are dropped: between the nodes of one host
  the transport is shared memory, with nothing to choose. Delete the call and the key, and list them in the change description.
- **A queue size per input becomes a number of samples per step.** See the queue-sizing
  paragraph of [section 4](#4-ros-1-to-hypha-map): `queue_size` is not `perStep`.

**Check:** every parameter and topic row of the inventory has one TOML entry.

### Step 4: Replace the build glue

Replace `catkin` in `CMakeLists.txt`. Keep the target names, the source lists
and the existing comments.

```cmake
find_package(hypha CONFIG REQUIRED)

# Only when the node owns .msg files. One call for each package.
hypha_generate_messages(
  PACKAGE speed_controller_msgs
  MSG_DIR "${CMAKE_CURRENT_SOURCE_DIR}/msg"
  DEPENDS std_msgs)

add_executable(speed_controller src/speed_controller_node.cpp src/controller.cpp)
target_link_libraries(speed_controller PRIVATE hypha::sdk speed_controller_msgs)
target_compile_features(speed_controller PRIVATE cxx_std_23)
install(TARGETS speed_controller RUNTIME DESTINATION "${CMAKE_INSTALL_BINDIR}")
install(DIRECTORY config/ DESTINATION "${CMAKE_INSTALL_DATADIR}/speed_controller")
```

- hypha's standard message packages (`std_msgs`, `geometry_msgs`,
  `sensor_msgs`, `nav_msgs`, `tf2_msgs`, ...) come with the SDK. Do not copy
  them into the node.
- Set `INSTALL_RPATH` to the directory of `hypha::ffi` as the `scan_cycle`
  example does, so the installed binary finds `libhypha_ffi.so`.
- Do not link the library statically.
- **`MSG_DIR` and `SRV_DIR` hold the definitions directly: a subdirectory is not read**
  (`hyphaMessages.cmake`). A package that groups its `.msg` files in subdirectories
  (`mrs_msgs` has 19 groups, and a script `link_msg_files.sh` that links them flat) needs one
  flat directory first: copy the files it needs, unchanged, into one directory of the port
  named for the package (`ports/mrs_trackers/msg/mrs_msgs`, 9 files), or run the upstream
  script. Copy only the messages the port uses, and declare the package **once** for the
  whole process: two ports that each copy a different subset and each call
  `hypha_generate_messages(PACKAGE mrs_msgs ...)` make two packages of one name, and a
  binary that links both gets two generated `mrs_msgs`. A family of ports that run in one
  process shares one message directory.
- The message names of a ROS 2 node (`sensor_msgs::msg::PointCloud2`) need one more CMake
  call: [section 11](#11-porting-a-ros-2-node).

**Check:** `cmake -S . -B build -DCMAKE_PREFIX_PATH=<hypha prefix>` configures.

### Step 5: Replace the node glue

Change only the ROS node file (the file with `main` and the ROS node class).
Logic files stay as they are, except the token edits of
[section 5](#5-message-types-and-the-minimal-logic-edits).

Example ROS node. `Controller` is ROS-free logic in `controller.h`:

```cpp
#include <ros/ros.h>
#include <std_msgs/Float64.h>
#include "speed_controller/controller.h"

class SpeedControllerNode {
public:
  SpeedControllerNode(ros::NodeHandle& nh, ros::NodeHandle& pnh) {
    pnh.param("gain", gain_, 0.5);
    pnh.param("rate", rate_, 100.0);
    controller_.setGain(gain_);
    speed_sub_ = nh.subscribe("odom_speed", 10, &SpeedControllerNode::speedCallback, this);
    cmd_pub_ = nh.advertise<std_msgs::Float64>("command", 1);
    timer_ = nh.createTimer(ros::Duration(1.0 / rate_), &SpeedControllerNode::timerCallback, this);
  }

  // Measured speed from odometry, in m/s.
  void speedCallback(const std_msgs::Float64::ConstPtr& msg) { controller_.addSpeed(msg->data); }

  void timerCallback(const ros::TimerEvent&) {
    std_msgs::Float64 cmd;
    cmd.data = controller_.update();
    cmd_pub_.publish(cmd);
  }
  ...
};
```

The hypha node file. `controller.h` and `controller.cpp` do not change.
Put the cycle node class in a header of the node (for example
`speed_controller_node.h`) so that the tests can include it. This example shows
it in one file:

```cpp
#include <hypha/hypha.h>
#include <std_msgs/Float64.h>

#include "speed_controller/controller.h"

HYPHA_LOG_MODULE_INIT("speed_controller")

namespace {
using Float64 = std_msgs::Float64;

class SpeedControllerNode final : public hypha::CycleNode
{
  public:
    SpeedControllerNode()
    {
        this->controller.setGain(
            hypha::config().required<double>("gain", hypha::constraints::finite())
        );
    }

    void step(const hypha::CycleTime&) override
    {
        for (const Float64& sample : this->speed.samples()) {
            this->controller.addSpeed(sample.data);
        }
        const bool speedUnavailable = this->speed.verdict() == hypha::InputVerdict::Stale ||
                                      this->speed.verdict() == hypha::InputVerdict::NeverArrived;
        if (speedUnavailable) {
            // The response the user approved for missing speed.
            this->command.send({.data = 0.0});
            return;
        }
        this->command.send({.data = this->controller.update()});
    }

    Controller controller;

    // Measured speed from odometry, in m/s.
    hypha::Arrivals<Float64> speed{*this, "speed"};
    hypha::Output<Float64>   command{*this, "command"};
};
}  // namespace

int main(int argc, char** argv)
{
    return hypha::runNode<SpeedControllerNode>(argc, argv, "speed_controller");
}
```

What changed and why:

- The comment on `speedCallback` moved to the port that now carries the same
  value. No comment was lost.
- `speedCallback` was one line of glue around `controller_.addSpeed(...)`. The
  loop calls the same logic once for each message, as ROS did.
- An `Arrivals` port gives at most `perStep(n)` samples to one step (default 8).
  Set it above
  `input rate / rate_hz`. The port's queue is the service
  maximum, so a burst of that size needs no setting.
- `timerCallback` became `step()`. `cmd_pub_.publish(cmd)` became
  `send(message)`. `controller_.update()` is the same call. The rate is `rate_hz`
  in the `[cycle]` table, so the node reads and checks no rate.
- The ROS node had no rule for missing speed. The stop output is an approved
  behaviour change: ask the user for it, do not choose it yourself.
- `hypha::runNode` loads the TOML file in `argv[1]` (optional), constructs the node
  (`hypha::config()` works there; the process has not joined the bus), joins the bus, calls
  `setup()`, starts its cycle, checks that every parameter key was read, reports the node
  (manifest, `~/introspection`, `~/state`) and runs until a stop. It names the phase in the
  error line `node failed in <phase>: <what>`. It returns 78 when the configuration is refused
  before the run (the file, an unread key, a `hypha::ParameterError` or a QoS mismatch), 70
  when the run fails or `failRun()` is called, and 2 for more than one argument. Without the
  file the node starts with no parameters and no `[log]` table, so the first required key
  stops it.
- `[node].name` overrides the name in code.

Keep the original node class when the logic is in its methods and not in a
separate class. Remove its ROS members (`ros::Subscriber`, `ros::Publisher`,
`ros::Timer`, `NodeHandle`) and its constructor glue, derive it from
`hypha::CycleNode`, and add the ports as members. Do not move method bodies.

**Check:** the node compiles with no ROS header and no `ros::` name.

### Step 6: Port the tests

- **gtest tests of logic** compile against hypha message types with the
  include edits of section 5. Do not change their assertions or names.
- **Tests of a callback or a timer** of a cycle node run its `step` through
  `hypha::testing::StepHarness` (`hypha/testing.h`), without starting the node. A callback
  of an `ApplicationNode` has no step, and `StepHarness` does not run it: test its logic as
  in this step, and its start-up and bus as in [section 10.10](#1010-test-an-application).
  Keep the test name and every assertion. Add one test for each verdict that the ROS test did not cover:
  usable, stale, never arrived.

```cpp
#include <gtest/gtest.h>
#include <hypha/hypha.h>
#include <hypha/testing.h>

TEST(SpeedControllerStep, CommandsAfterAUsableSpeedArrived)
{
    SpeedControllerNode node;
    node.controller.setGain(0.5);
    hypha::testing::StepHarness harness{node};
    harness.set(node.speed, std_msgs::Float64{4.0});
    hypha::CycleTime time;
    time.now = hypha::Time::fromNanos(1'000'000'000);
    harness.step(node, time);
    EXPECT_EQ(harness.sent(node.command).size(), 1U);
}
```

  The harness setters (`set`, `setStale`, `setNeverArrived`, `setStagnant`) give a
  port what a capture would give, for `Latest` and `Arrivals` ports alike; see
  `docs/cpp-api.md`, "Test a step without a bus". An `optionalTopic` input or output is
  enabled in the harness only when the test's TOML gives its table a non-empty `topic`
  (below); with no table, no `topic` or `topic = ""` it is disabled, and `deliver()`,
  `set()`, `setStale()` and `setStagnant()` on a disabled input throw. The harness moves
  no clock: the
  step reads the `CycleTime` it is given, for its decisions and for the stamps it
  writes. Ported code that still calls `hypha::now()` gets the process clock in a
  harness step; until it is changed, the test sets simulated time to the step's time
  (`hypha::enableSimTime`, `hypha::setSimTime`). `steadyNow()` measures real elapsed
  time and is never set.

  To test the branch behind an optional input, wire it in the TOML of the test. The wired
  input needs a `silence_ms` as well, which `deliver()` reads:

```toml
[ports.inputs.mag]
topic = "/test/mag"
silence_ms = 100
[ports.outputs.debug]
topic = "/test/debug"
```

```cpp
TEST(ImuFilterStep, ReadsTheMagnetometerWhenItIsUsable)
{
    hypha::ConfigContext        ctx{"test/with_mag.toml"};  // before the harness
    ImuFilterNode               node;
    hypha::testing::StepHarness harness{node};
    harness.deliver(node.mag, sensor_msgs::MagneticField{}, hypha::Time::fromNanos(1'000'000'000));
    hypha::CycleTime time;
    time.now = hypha::Time::fromNanos(1'050'000'000);
    harness.step(node, time);
    EXPECT_EQ(node.mag.verdict(), hypha::InputVerdict::Usable);
}
```

  The `ConfigContext` (or a `Node` that holds the file) exists before the harness is
  built, because the harness reads the `topic` then. Only one exists at a time, so the
  branch for a port that is left out (`topic = ""` or no table) is a second fixture with
  its own file and its own `ConfigContext`. "One parameter file for each process" holds while
  a `ConfigContext` or a `Node` lives: a different file is refused then, and accepted once
  the last one is destroyed, so two fixtures run in sequence in one test binary, one file each
  (`NodeParameterFile.*` of `crates/hypha-cxx/test/test_node_session_ownership.cpp`; #208).
- **A test that starts the cycle on a bus** (not a harness step) builds the `hypha::Node`
  and then calls `node.setup(bus)` and `node.start(bus)`, in that order.
  `CycleNode::start()` does not call `setup()`, where `runNode` does: a rate or a
  `setTrigger(port)` set in `setup()` is missing otherwise, and the start fails with
  `cycle 'x' is refused: it has neither a rate nor a trigger`
  (`ports/robot_localization/test/test_ekf_bus.cpp`,
  `ports/autoware_voxel_downsample/test/test_voxel_downsample_bus.cpp`). The same holds the
  other way: `StepHarness` does not run `setup()`, so a node whose `setup()` forgot its
  `setTrigger` passes every step test and fails at start. Keep one test that starts the cycle.
- **Recorded regression tests** (golden fixtures of a ROS bag and the outputs of
  the ROS node): convert each bag with `mcap convert`. Replay it through
  `StepHarness` and through the bus (`hypha play --clock` with `hypha record
  --sync`). Keep the ROS comparison rule unchanged. Report the agreement of each
  fixture. The step replay is one gtest with `hypha::testing::McapReplay`
  (`hypha/testing/mcap_replay.h`; link `hypha::testing_mcap`): it feeds the recorded
  inputs into the harness step by step, on the deadlines and with the sample ages a
  bus replay gives, and `read<M>()` returns the golden outputs decoded. An optional
  input that only some recordings hold is fed with `feedIfPresent(port)`. A recording
  whose publishers stop before its end is replayed through the silence with
  `replay.endAt(time)`, so the test sees the stale steps and the cleared outputs. Python is
  needed only to generate fixtures. See `docs/cpp-api.md`, "Test a step without a
  bus".

```cpp
TEST(DetectorGolden, Fixture01MatchesTheRosNode)
{
    hypha::Node                 node("detector_golden", "test/detector_test.toml");
    Detector                    detector;
    hypha::testing::StepHarness harness{detector};
    hypha::testing::McapReplay  replay{"test/fixtures/01.mcap", hypha::Hz(20.0)};
    replay.feed(detector.position);
    replay.feed(detector.markers);
    const std::vector<hypha::testing::Recorded<geometry_msgs::Pose>> golden =
        replay.read<geometry_msgs::Pose>("/detector/pose");
    while (replay.stepDue()) {
        const hypha::CycleTime time = replay.next(harness);
        harness.step(detector, time);
        compareWithGolden(harness.sent(detector.poses), golden, time);
    }
}
```
- **An `ApplicationNode`** has no step, so `StepHarness` does not run it: see
  [section 10.10](#1010-test-an-application).
- **rostest tests** need a ROS master and do not run under hypha. Port the
  harness to a test that starts the node with a test TOML and drives it with
  `runFor()`, or to step tests. Keep every case. Report a case you cannot port.
- Put tests in the test folder of the node, in files named for the behaviour
  they test. Write positive and negative cases.

**Check:** every test that existed before still exists and passes, and the
step has a test for each verdict of each input.

### Step 7: Run and verify on the bus

Reading the diff is not verification. Run the binary.

1. Start the node with its TOML file in a shell with `HYPHA_DOMAIN=<name>`.
   Use the same domain for every tool and peer below.
2. Read the manifest on `~/introspection` (`hypha topic echo /<node>/introspection`,
   or `hypha::introspect()`). Compare its configuration rows, endpoints and
   cycle rows with the TOML file: the resolved topics and the rate.
3. `hypha topic list` shows only the topics seen publishing during its `--timeout`
   (default 1.0 s): a topic of the inventory with no traffic in that time is not listed.
   Check such a topic with `hypha topic hz <topic>` (item 5) once traffic starts.
4. With no input publisher, `hypha topic echo /<node>/state` (published by
   `runNode`) shows `WAITING`, and the outputs follow the approved missing-input
   response.
5. Start the input (a peer node, `hypha play`, or `hypha_ros_bridge` from the
   ROS system). `~/state` shows `INPUTS_USABLE`. `hypha topic hz` on each
   output shows `rate_hz`, or the rate the step publishes at.
6. Stop the input. After its `silence_ms`, `~/state` shows `DEGRADED` and
   the outputs follow the approved stale-input response.
7. Compare the outputs with the ROS node for the same input. Record both
   (`hypha record`), or run the ROS node through the bridge beside the hypha
   node. A recorded input is replayed with `hypha play --topics <inputs>` and the
   input topics only: golden outputs must never reach the live output topics.
   For an `ApplicationNode`, `examples/sensor_pipeline/replay_check.sh` runs this replay and
   compares two outputs by stamp ([section 10.9](#109-a-runnable-sensor-node-and-its-replay-check)).
8. Negative start checks: remove a required `[ports.inputs.<name>]` table, write a number as
   text, pass a second argument. Each one stops the node with a message and a
   non-zero exit status.
9. `Ctrl-C` (SIGINT) or `kill <pid>` (SIGTERM) stops the node, and the process ends. A
   node that a script, a CI step or a shell starts as a **background job** (`node &`)
   inherits an ignored SIGINT, and hypha keeps a signal that the process ignores ignored
   ([docs/README.md](README.md), "Node Creation Owns SIGINT and SIGTERM"): `kill -INT` then
   does nothing. Stop such a node with SIGTERM, which `hypha launch` and `docker stop` send,
   or start it in the foreground to test Ctrl-C. A node that blocks in `started()` does not
   stop on either until `started()` returns ([section 10.6](#106-threads)).

**Check:** each item above gave the expected result. Write the results in the
change description.

### Step 8: Deploy

Replace the ROS launch file with a `hypha launch` units file (see
`docs/tools.md` in `apps/`). One unit for each node, with the TOML file as
its only argument:

```toml
[[units]]
name = "speed_controller"
exec = "/opt/hypha/bin/speed_controller"
args = ["/etc/hypha/speed_controller.toml"]
restart = "on-failure"
restartDelaySeconds = 1.0

[units.env]
HYPHA_DOMAIN = "robot"
```

Two instances of one binary use two TOML files with two `[node].name` values
and their own `[ports.inputs.<name>]` and `[ports.outputs.<name>]` tables. Every process of one system, tools
included, uses the same `HYPHA_DOMAIN`.

---

## 4. ROS 1 to hypha map

| ROS 1 | hypha |
|---|---|
| `ros::init`, `ros::NodeHandle` | `hypha::runNode<App>(argc, argv, name)`, with the TOML file as `argv[1]` |
| `ros::spin()` | `hypha::runNode` runs `node.run()` |
| `pnh.param("x", v, d)`, rosparam YAML | Top-level TOML key, `hypha::config().required<T>("x")` |
| `<remap>`, `ns=` | `[ports.inputs.<name>]` or `[ports.outputs.<name>]` table with the full `topic` |
| Private topic `~x` | `~/x` in a ports table |
| `nh.subscribe` that stores the latest value | `hypha::Latest<T>` port |
| `nh.subscribe` with work for each message | `hypha::Arrivals<T>` port |
| `message_filters` synchroniser | `Arrivals` ports; match `header.stamp` in the step. Trigger on the input that is published last: its step holds the partner samples sent before it, or with it by one step of one node ([execution-model.md](execution-model.md) §6.6). A partner sent after the trigger sample is not in that step |
| `nh.advertise` + `publish` | `hypha::Output<T>` port: call `send(message)`. A step sends one message per Output. A step that sends two declares the Output with `hypha::perStep(2)`: `hypha::Output<T> name{*this, "name", hypha::perStep(2)};` |
| `nh.createTimer` | The cycle rate, `rate_hz` under `[cycle]` |
| `ros::TimerEvent` | `const hypha::CycleTime&`: `dt` (no value in the first step), `missed` |
| `ros::ok()` in the loop of a worker thread | The stop flag of the application: a `std::atomic<bool>` that its destructor sets before it joins the thread ([section 10.6](#106-threads)). Every blocking read in that loop takes a timeout that returns to this check: a read with no timeout never returns to it, so the stop is never seen |
| `ros::Rate r(hz); while (ros::ok()) { ...; r.sleep(); }`, `ros::Duration(s).sleep()`, in a **worker thread** | A wait that ends at the stop, and a period that is a grid of its own. `hypha::Rate` and the clock sleeps do not exist (`Node::timer` runs on the run thread, where a blocking read must not). Pattern and trap: [section 10.6](#106-threads), "A paced worker". In a node whose work is periodic and does not block, the loop is not a worker at all: it is a rate cycle (Step 2) or a `Node::timer` |
| `ros::shutdown()` | `hypha::requestStop()`: a clean stop, from the run or another thread. For a failure (a device error), `failRun(reason)`: the run ends with exit 70, and `hypha launch` restarts the unit by its policy ([section 10.4](#104-time-status-and-logging)). `ros::shutdown()` followed by a normal exit of roslaunch is `requestStop()`; a device that is lost is `failRun()` |
| `ros::Subscriber::shutdown()`, `ros::Publisher::shutdown()` | **None.** No call closes an endpoint while the node runs: an endpoint lives as long as the member that holds it, and `setup()` creates every one (§10.2). A node that opened a subscriber for each connection keeps one endpoint for all of them, and a registry of the connections in the application. Report the loss as a gap in the form of section 8 |
| `ros::TransportHints().tcpNoDelay()`, `.udp()`, a `*_nodelay` parameter | None: dropped, with the parameter. The transport between the nodes of one host is shared memory (Step 3) |
| `tf_prefix` (added to every frame id) | None. Read it with `hypha::config().required<std::string>("tf_prefix")` and write each frame id in full, prefix included |
| Hand-written message age check (decides whether an input may be used) | The port's `silence_ms`, verdict `Stale`. A use of the source time as data stays logic |
| `ros::Time::now()` for a stamp | In a cycle step `time.stamp`; elsewhere `hypha::toRosTime(hypha::now())`. See cpp-api.md §3 |
| `header.seq` | The publisher writes it |
| `ros::ServiceServer`, `ros::ServiceClient::call`, `actionlib` | **No native counterpart** (issue #132 for services; none for actions). What exists today, and what to do, is in [section 7](#71-services) and [section 7.2](#72-actions) |
| `diagnostic_updater`, `diagnostic_msgs/DiagnosticArray` | The message types ship with the SDK; `diagnostic_updater` has no counterpart (issue #217): [section 7.3](#73-diagnostics) |
| `image_transport::CameraPublisher`, `advertiseCamera`, `image_transport::Publisher` | Two plain outputs, `Output<sensor_msgs::Image>` and `Output<sensor_msgs::CameraInfo>`, and the stamp of the info is the stamp of the image (`this->info_.header.stamp = this->image_.header.stamp;` in `ports/arena_camera/src/arena_camera_node.cpp`). `image_transport`'s other transports (`compressed`, `theora`, `depth`) are plugins that hypha does not have: drop them, list them in the change description. A subscriber is a `Latest<sensor_msgs::Image>` port. Size the topic as a large message ([section 12](#12-large-messages)) |
| `camera_info_manager::CameraInfoManager`, `camera_info_url`, `set_camera_info` | No counterpart in the SDK (cpp-api.md §1.1; #265 leaves the decision open). The port reads the calibration from a `[calibration]` table of its TOML file (the fields of the camera info YAML: `image_width`, `image_height`, `distortion_model`, and the matrices as `rows`, `cols`, `data`) with a function of its own that checks the sizes: `readCalibration` in `ports/arena_camera/src/camera_info.cpp` (nested tables: Step 3). The `file://` and `package://` URL forms and the `set_camera_info` service are dropped (a service: #132); a node without a calibration publishes a `CameraInfo` with the image size and zero matrices (REP 104) |
| `nh.advertise(..., latch = true)` | `hypha::latched` on the `Output` (or on `node.advertise`). A late subscriber gets the last sample only |
| `ROS_INFO("%f", x)` | `HYPHA_LOG_INFO("{}", x)`: a `std::format` string |
| `ROS_WARN_THROTTLE(1, ...)` | `HYPHA_LOG_WARN_THROTTLE(1.0, ...)` |
| `ROS_WARN_ONCE(...)` | `HYPHA_LOG_WARN_WHEN(true, false, ...)`: the first call writes, no later one |
| `ROS_WARN_COND(cond, ...)` | `if (cond) { HYPHA_LOG_WARN(...); }`: **not** `_WHEN`. `ROS_WARN_COND` writes every time `cond` is true. `HYPHA_LOG_WARN_WHEN(state, false, ...)` writes only when `state` changes from false to true at that call site, so a condition that stays true is silent after its first line. `_WHEN` is for an edge (`ROS_WARN_COND` in a loop that you want once per episode) |
| `ROS_ASSERT(cond)`, `ROS_ASSERT_MSG(cond, ...)` | `if (!(cond)) { throw std::logic_error("..."); }`. hypha has no assertion macro. An exception that leaves a step or a callback ends the run with `Panic` and no output of that step is sent, as the abort of a failed ROS assertion ends the node: the process exits 70 and `hypha launch` applies the unit's restart policy ([execution-model.md](execution-model.md) §6.3, `crates/hypha-cxx/test/cycle_step_exception_is_logged.cpp`). List the throw as glue in the change description |
| `tf2_ros::TransformBroadcaster` | Output on `/tf` of type `tf2_msgs::TFMessage` |
| `tf2_ros::StaticTransformBroadcaster` | An `Output<tf2_msgs::TFMessage>` declared with `latched` and `topic = "/tf_static"` in its ports table |
| `use_sim_time` | `HYPHA_USE_SIM_TIME=1` in the node's environment; for `ros2 bag play`'s behaviour, outside the replay lockstep, `HYPHA_REPLAY_LOCKSTEP=0` for the player and every node of the launch |
| rosbag record / play | `hypha record` / `hypha play` (`apps/`) |
| roslaunch | `hypha launch` |
| roscore | None. Nodes find each other through shared memory |

**Queue sizing:** `queue_size` has no counterpart: there are no transport depths,
history samples or history byte limits to tune. A topic's service policy (`hypha::Qos`,
cpp-api.md §7) sets the deepest queue a subscriber may ask for, its `maxDepth`: 32 for
the `default` policy of a type the stack maps to no other. A plain `Subscriber<T>`
queues that many samples, and so does a `Latest<T>` cycle input. On an `Arrivals<T>`
input, `perStep(n)` determines both the iteration's sample share and the queue; it is
the ROS queue size of a node that processed every message. A `perStep` above the
policy's `maxDepth` is refused when the port is constructed, so a queue deeper than 32
needs a policy with a deeper `maxDepth` (at most 1024), mapped to the message type with
`hypha::QosOf<M>` in a header that both ends of the topic include. Publishers do not
configure a queue size. Variable-size messages are growable unless the publisher names
its largest message with `maxPayloadDynamic(bytes)` (on `advertise` or an `Output`) or
its policy does (`Qos::maxPayloadDynamic`), which bounds the publisher's shared memory.
A fixed-size message is bounded to its own size whatever is named. Sizing a topic of large
messages: [section 12](#12-large-messages).

**`queue_size` is not `perStep`, in a rate cycle.** In a triggered cycle `perStep(n)` is the
ROS queue size (above). In a **rate cycle** the queue of an `Arrivals` port holds the samples
that arrive in **one period** of the cycle, so the number is the input rate over `rate_hz`,
rounded up, with the peak of a burst counted in, not the ROS value. A 1 Hz aggregator that
reads a 100 Hz topic needs 100 or more: with a queue of 32 (the deepest of the `default`
policy) it dropped 1044 samples in 20 s (`ports/diagnostic_aggregator`). A sample over the port's share is dropped, counted
(`lost()`, `work_limit_drops_total`) and logged. A `perStep` above 32 needs a policy with a
deeper `maxDepth` ([section 10.5](#105-queue-depth)); without one the port is refused at
construction with `std::invalid_argument` (exit 70, which a supervisor restarts). Read the
number from the input's table with its limit, so that a value out of range is a configuration
fault (exit 78, [cpp-api.md](cpp-api.md) §5.4, "Queued work, budget and overflow"):

```cpp
hypha::Arrivals<sensor_msgs::Imu> imu{
    *this, "imu",
    hypha::perStep(hypha::config("ports.inputs.imu")
                       .required<uint32_t>("queue_size", hypha::constraints::range(1, 32)))};
```

The age of a queued sample is counted from when it was sent, not from the step that read it
(`ports/diagnostic_aggregator` counted from the step): `port.sentAt(i)` is the host-monotonic
send time of sample `i` of `samples()` ([cpp-api.md](cpp-api.md) §5.1), so on host time the
age is `hypha::steadyNow() - port.sentAt(i)`. An age that decides whether the sample may be
used is supervision (Step 2), and `silence_ms` already does it.

**`Header` in a new message type:** a ported ROS type keeps its `Header`; removing it
changes the definition and its MD5. A message type written for hypha carries a `Header`
only when it needs the time the data describes (a measurement time for TF lookup,
interpolation or ordering by source time) or a `frame_id`. It needs none for freshness,
loss or order: every frame carries the send time and a per-publisher sequence, and the
receiver checks them ([communication-integrity.md](communication-integrity.md)).

A step stamps its messages with `time.stamp`. Outside a step,
`hypha::toRosTime(hypha::now())` converts the node's time; it throws only for a time
no clock gives (negative, or after 2106).

A service name does not resolve from the ports tables. Read it with
`hypha::config().required<std::string>("<key>")` and pass the value.

---

## 5. Message types and the minimal logic edits

Generated hypha messages keep the ROS package namespace, the type name, the
field names, and the `Ptr` and `ConstPtr` aliases. Logic that uses message
types therefore compiles with few edits. The include line stays: hypha installs
`<sensor_msgs/Imu.h>` at the ROS 1 path. The process then uses hypha's headers in place
of ROS's own; it cannot include both. Make only these edits:

| ROS 1 | hypha | Edit |
|---|---|---|
| `boost::array<double, 9>` | `std::array<double, 9>` | Only where the logic names the type. A message field is a `std::array`. |
| `header.stamp` is `ros::Time` | `header.stamp` is `hypha_msgs::RosTime{secs, nsecs}` | `stamp.sec` to `stamp.secs`, `stamp.nsec` to `stamp.nsecs`. A call such as `stamp.toSec()` becomes `hypha::fromRosTime(stamp)` (`hypha/message_stamp.h`), which returns `std::optional<hypha::Time>`, empty when `nsecs` is one second or more. Check the optional, then call `.seconds()` on the value. Write no helper. |
| `ros::Time`, `ros::Duration` in logic | `hypha::Time`, `hypha::Duration` | Only when the logic itself measures time. |
| `ros::Duration::toSec()`, `ros::Duration::toNSec()`, `ros::Time::toSec()`, and `ros::Time` / `ros::Duration` arithmetic | `hypha::Duration::seconds()` (`double`), `hypha::Duration::nanos()` (`int64_t`), `hypha::Time::seconds()`. The operators keep their meaning: `Time + Duration`, `Time - Duration`, and `Time - Time`, which is a `Duration` | Rename the call. `ros::Duration(s)` is `hypha::Duration::fromSeconds(s)`, which throws `std::invalid_argument` for NaN or infinite `s`. **`seconds()` can differ from `toSec()` in the last bit.** `seconds()` is the double nearest to the exact span, and `ros::Duration::toSec()` is `sec + 1e-9 * nsec`, which for some values is the neighbour: for 22 of the whole milliseconds from 1 to 59 ms they differ, and a throttle of 0.002 s showed 8.7e-19 against upstream over 857,310 compared numbers (`ports/mrs_controllers`; 0 with the ROS formula). Harmless for control. A test that compares a number with a ROS recording therefore allows a tolerance of a few units in the last place, and does not expect equal bits |
| `ros::Time().fromSec(t)`, `ros::Time(t)` from a `double` of seconds | `hypha::Time::fromSeconds(t)`: rounded to the nearest nanosecond with the whole seconds and the fraction converted apart, as ROS does, so `1700000001.25` is `250000000` ns (it was `249999872` before #250). Throws `std::invalid_argument` for NaN or infinity. For a header stamp: `hypha::toRosTime(hypha::Time::fromSeconds(t))`, which throws `std::out_of_range` for a time before 1970 or after 2106; on a thread that must not throw (a vendor SDK's), check the range first | Rename the call. |
| `ros::Time::now()` in logic | `hypha::now()` for framework time, `hypha::steadyNow()` for an interval. For a stamp, `hypha::toRosTime(hypha::now())` (`hypha/message_stamp.h`) | Keep the meaning of the original call. **In logic that a step calls** (a plugin, a helper class of the cycle node) the call is the step's time, passed as an argument: `time.now` for decisions and `time.stamp` for a stamp. `hypha::now()` returns the step's time in a started cycle, but the process clock in a `StepHarness` step, so logic that calls it cannot be tested without `enableSimTime`, and a replay is only deterministic through the step's time (Step 2, plugins). |
| `ros::Time::isSystemTime()` | `!hypha::isSimTime()` (`hypha_base/clock.h`, included by `hypha/hypha.h`) | Edit the call only. |
| `ros::Time::waitForValid()` | No call. A cycle step does not run before the first `/clock` sample (execution-model.md section 11.1), so its `time.now` is valid. | Delete the call. |
| `hypha::now()` before the first `/clock` sample, on a node with `HYPHA_USE_SIM_TIME=1` (`hypha_base/clock.h`) | The placeholder zero: `Time` 0 ns, not a clock value. `hypha::isSimTime()` is already true, and no C++ call says whether a value has arrived. | Inside a step, use `time.now`. Outside one, the value is trustworthy only after the first `/clock` sample, and no call reports that (execution-model.md section 11.1: "The zero that a node starts with is a placeholder, not a value"). |
| `void cb(const Msg::ConstPtr& msg)` whose logic keeps the pointer | `samples()` gives a `const Msg&` for one step, and it is valid only until the step returns (section 6). | Copy the value into a member of the node (`Msg last;`, then `last = sample;`). If the callback only stores the latest value, use a `Latest` port instead (Step 2). |
| `boost::shared_ptr<Msg>`, `boost::make_shared<Msg>(...)` | `std::shared_ptr<Msg>`, `std::make_shared<Msg>(...)`. The generated `Msg::Ptr` is `std::shared_ptr` (for example `std_msgs/Float64.h`). | Glue only (the paragraph below the table): change the namespace and the include (`<memory>`). Nothing else. |
| `#include <sensor_msgs/point_cloud2_iterator.h>`, `<sensor_msgs/point_field_conversion.h>` (`PointCloud2Modifier`, `PointCloud2Iterator`, `PointCloud2ConstIterator`, `readPointCloud2BufferValue`) | Works unchanged: the SDK installs ROS's own headers beside the messages (cpp-api.md §1.1) | None. Do not write a field table or byte copies by hand. For ROS 2 code, `<sensor_msgs/point_cloud2_iterator.hpp>`. |
| `#include <sensor_msgs/image_encodings.h>`, `<sensor_msgs/distortion_models.h>` (`image_encodings::BGR8`, `bitDepth`, `numChannels`, `isColor`, `isMono`, `isBayer`, `distortion_models::PLUMB_BOB`) | Works unchanged: the SDK installs ROS's own headers beside the messages (cpp-api.md §1.1) | None. Do not copy the file into the node. For ROS 2 code, `.hpp`. |
| A header-only library that ROS installs but that is not ROS API, such as `tf2/LinearMath` (`tf2::Quaternion`, `tf2::Matrix3x3`) | The same headers, copied into the node as third-party files with their licence. The SDK ships no math types. | Keep every use in the logic. In the copied headers edit only their ROS lines (`#include <ros/macros.h>` goes, `ROS_DEPRECATED` becomes `[[deprecated]]`), and list the copy and those edits in the change description. |
| The rest of that family: `angles/angles.h` (`angles::normalize_angle`), `tf2::Transform` (`tf2/LinearMath/Transform.h`), `tf2::Stamped` (`tf2/transform_datatypes.h`), the pieces of `mrs_lib` a plugin uses | The `angles` package is one BSD header, not in the SDK: copy it from `github.com/ros/angles` (`robot_localization` took noetic-devel `d2f5e3c`) with its licence. `Transform.h` belongs to the `LinearMath` copy, with no edit. `transform_datatypes.h` holds a `ros::Time`: copy it, change that member to `hypha::Time` (3 lines in `robot_localization`) and list them. | Keep every use in the logic. `tf2::fromMsg` and `tf2::toMsg` of `tf2_geometry_msgs` are not in the copy: write each conversion the logic calls once, in namespace `tf2`, in a header of the node (`ports/robot_localization/inc/robot_localization/ros_filter_utilities.h`), so the logic lines stay. `tf2::convert` (above) is by hand. A copied `.cpp` with `ROS_ERROR` lines: `HYPHA_LOG_ERROR`, and its own `HYPHA_LOG_MODULE_INIT` (the paragraph on log macros in a header, below). |
| A third-party **vocabulary type** that the logic names and that is not installed: `tl::expected<T, E>` and `tl::unexpected` (`tl_expected`) | The C++23 standard type of the same names, `std::expected` (the SDK already needs C++23). Token edits only: `tl::expected` becomes `std::expected` and `tl::unexpected` becomes `std::unexpected`; `has_value`, `value`, `error` are the same | Each edited line gets a comment such as `// hypha port: std::expected for tl::expected.`; the lines are logic edits outside this table, so count them in the change description. A library with no standard counterpart is not installed by the port: copy a header-only one as in the rows above, and stop at any other |
| An include of a ROS or PCL package that **only supplies Eigen**: `<pcl_conversions/pcl_conversions.h>`, `<pcl/point_cloud.h>`, with no `pcl::` name in the file (`grep -c 'pcl::'` is 0) | `<Eigen/Core>` (or `<Eigen/Dense>`), and `Eigen3::Eigen` in `target_link_libraries`. The SDK ships no Eigen. If the file does call `pcl::`, the node needs PCL: stop and ask | Delete the PCL include lines and add the Eigen one: this is the only edit. Evidence: `ports/autoware_voxel_downsample`, whose unchanged core stopped at `pcl_conversions/pcl_conversions.h: file not found` |
| `std::mutex` that only guards callbacks | No mutex. | Delete the mutex and its lock calls. Callbacks of one `WaitSet` never run at the same time (execution-model.md section 10.1), and a cycle step runs on that `WaitSet` (execution-model.md section 6.1). Keep a mutex only where a worker thread shares the data. |
| `pub.publish(msg);` on a plain `Publisher` (`publish` is `[[nodiscard]]`, `hypha/publisher.h`) | `if (!pub.publish(msg)) { ... }`, with `pub.lastPublishStatus()` for the reason | Check the result and log the reason, as cpp-api.md section 1 does: `HYPHA_LOG_WARN_THROTTLE(1.0, "...: {}", pub.lastPublishStatus().name());`. A cycle `Output::send` returns nothing, so this row does not apply to it. |

The `tf2/LinearMath` row does not cover `tf2/convert.h` (`tf2::convert`): it uses
`ros::Time`, so do not copy that header. Convert the fields by hand, one assignment for
each field, and list them in the
change description.

In glue, each `boost::` type becomes its `std::` equivalent, as the rows above say. A
logic file keeps its `boost::` types (section 2.1), except a type that names a message
field. The SDK ships no Boost, so the node's
own build must provide the Boost headers that its logic includes.

A message type that hypha does not ship becomes a `.msg` file of the node and
a `hypha_generate_messages()` call. Copy the `.msg` file without changes.

**Log macros in a header.** `HYPHA_LOG_MODULE_INIT("name")` selects the module of one
**translation unit**: it defines a function in an anonymous namespace, so a `.cpp` file has
one init, at file scope before its first log macro (a log macro written before it uses the
module `default`, and a second init in the same unit does not compile). A header cannot log
by itself: an inline function with a `HYPHA_LOG_*` line in a header has one body for each
includer, each tied to its own unit's module, which breaks the one-definition rule without a
diagnostic. Move the function bodies that log into a `.cpp` of the library with its own
`HYPHA_LOG_MODULE_INIT`, and keep their declarations in the header
(`mrs_lib/quadratic_throttle_model`: two inline functions with `ROS_ERROR_THROTTLE`, 63 lines,
moved unchanged into `src/quadratic_throttle_model/quadratic_throttle_model.cpp` of
`ports/mrs_controllers/third_party/mrs_lib`). A header-only third-party file with a log line
is therefore never a plain copy. (`ports/diagnostic_aggregator` carries one init in the header
that its other headers include, and builds and passes; it is the same hazard, so do not copy
it.)

A `bool` field decodes as ROS 1 decodes it: a zero byte is `false`, and every
other byte is `true`. Some ROS 1 producers send a nonzero byte other than 1
for `true`, so a typed hypha consumer accepts it. hypha itself writes only 0
or 1.

---

## 6. Traps

- **A relative topic is refused.** `odom_speed` is not a valid hypha topic.
  Write `/odom_speed` or `~/odom_speed`.
- **A topic fallback in code hides a missing TOML entry.** A port has none.
  Give a plain subscriber or publisher no fallback for a required topic.
- **A text value is not a number.** `gain = "0.5"` stops the node when it is
  read as `double`. This is intended.
- **A `Latest` port repeats its value.** Use `Arrivals` when the logic has an
  effect for each message.
- **The step must not block.** No sleep, no file or device read, no wait for a
  reply in the step. A hardware driver that blocks runs on a
  thread the application owns and hands its data over through a bounded queue.
- **Do not keep a pointer to a sample of `Arrivals::samples()`.** Samples are
  valid until the step returns. Copy what must outlive the step into a member.
  A `Latest` port keeps its value between steps (`*port`).
- **No timer of your own beside the cycle for the same work.** One rate, one
  cycle.
- **A port that nobody feeds yet logs a warning.** An input with no connected publisher
  warns once for its topic after 5000 ms: `no publisher connected to topic '<t>' in domain
  '<d>' after 5000 ms`. A command port that nobody has used (`switch_controller`,
  `constraints`: a service in ROS, called when an operator wants) warns in a healthy node.
  **No tag silences it**: `advisory` only keeps the node's `~/state` healthy
  ([section 3, Step 3](#step-3-write-the-toml-file)), and `optionalTopic` removes the port.
  A wildcard subscription never warns. Leave the line, say in the change description that
  it is expected for those ports, and report the case as a documentation or gap item
  (a service with an answer, #132, removes it).
- **`started()` must return.** A node that blocks there publishes no `~/state` and does not
  stop ([section 10.2](#102-the-application)).
- **`INPUTS_USABLE` is not readiness.** It means only that the inputs are
  fresh. Publish an application readiness topic when the system needs one.
- **One `[log]` table for each process.** A second node in the same process
  with a different table fails with `InvalidState`.
- **Stale shared memory after a crash** can look like a code fault. Run
  `hypha clean` with the domain stopped before you look for a bug.
- **A backward seek or a loop of `hypha play --clock` restarts the node.** A node
  that reads a `/clock` lower than its last one ends the run with `ClockRewound`, with
  or without a timer and in the open loop too, as does a cycle that sees simulated time go
  back; the process
  exits 75 and its supervisor starts it again
  ([execution-model.md](execution-model.md) §11.1). Every member is built anew, so
  the node needs no reset code for this; do not write one. Only simulated time restarts
  it: a step of the host wall clock does not ([execution-model.md](execution-model.md)
  §6.7, §11). When the ROS node resets its state on a backward header stamp, that reset
  is logic: keep it.
- **`replay not faithful`, or `the input history is shorter than one replay chunk`,
  under `hypha play --clock`** is a sizing fault of the replay, not of the step. The
  input's history could not hold what the cycle had not read yet. Check in this order:
  1. The line names the topic. Take its rate from the recording
     (`hypha topic hz` during an open-loop `hypha play`).
  2. For an `Arrivals` port, samples in one period of the cycle = rate x period.
     Compare with `perStep(n)` (8 unless set), and give the port a larger `perStep`
     ([cpp-api.md](cpp-api.md) section 7). A burst counts by its peak, not by the
     mean rate.
  3. For a `Latest` port, or when `perStep` suffices, look for a second publisher of
     the topic, or one that stamps ahead of the clock: the player splits its own
     chunks to fit the queue, but no other publisher's
     ([replay-flow-control.md](replay-flow-control.md) §6.7). Give the topic a deeper
     policy for them. A publisher that keeps sending while the node drains its inputs
     can also leave samples queued after the node's report, and the next chunk then
     overflows the queue.

---

## 7. Stop and ask

hypha has no equivalent for these. Stop and ask the user before you continue. The
subsections below say, for each, what exists on `develop` today, what does not, the issue,
and what a port does meanwhile, so that you neither invent an answer nor stall:

- A transform lookup that waits: `waitForTransform`, `lookupTransform` with a timeout,
  `canTransform` in a loop, `tf2_ros::MessageFilter` ([7.4](#74-a-transform-lookup-that-waits)).
  `hypha::TransformBuffer` lookups never wait (§10.7; [cpp-api.md](cpp-api.md) section
  10.2): the node holds its message until a later callback finds the transform, or drops it.
- `dynamic_reconfigure`, or a parameter changed at run time ([7.5](#75-a-parameter-that-changes-at-run-time)).
  Parameters are read once at start.
- `actionlib` ([7.2](#72-actions)).
- A service ([7.1](#71-services)), and a service whose caller blocks on the reply. hypha has
  no RPC: the call becomes a request topic and a reply topic, and the caller a state over
  steps.
- `nodelet` and in-process zero-copy between nodes. hypha nodes are processes; shared
  memory gives zero-copy between them (what exists for a large message: [section 12](#12-large-messages)).
- `diagnostic_updater` ([7.3](#73-diagnostics)).
- Missing or stale input with no rule in the ROS node. Ask which output is
  safe.
- A per-message latency requirement that a triggered step does not meet on the
  target. Measure it first.
- A logic change that you think is necessary. Report it, do not make it.

Every item in this list is also a gap report: see section 8.

### 7.1 Services

**There is no native service on `develop`.** No server or client type exists, and the SDK
ships no `.srv` definition of a standard package (`nav_msgs/GetMap` and `LoadMap` are not
there), and `hypha_generate_messages(PACKAGE nav_msgs ...)` is refused once `nav_msgs` is
a standard package (`'nav_msgs' is both the generated package and a dependency`), so a
port cannot add them under the ROS names (`ports/map_server`). Issue #132 asks for native
services and is open; a design for it is not merged, so write no code that assumes a
`serve` or a `call`. What exists is a convention, not an API
([cpp-api.md](cpp-api.md) §8): a request and its answer are two ordinary topics, the
request carries an identifier such as a sequence number, and the reply carries the same one.
A `.srv` file generates a `Request` and a `Response` message type with an MD5 each; the
example `examples/mrs/host/include/mrs_host/service.h` carries services that way, in the
MRS example only (it is not installed). What a port does, in this order:

1. **A setter service that changes a value the node reads from its TOML file** (the seven
   `set_*` services of `arena_camera`): drop the service. The value is the TOML key. Mark
   the place with one comment naming #132, list the service in the change description and
   report it as a gap ([section 8](#8-report-hypha-gaps-and-blockers)).
2. **A service whose callers stay in ROS** (a ROS client, `rosservice call`, the bridge) or
   whose `.srv` must keep its ROS definition: stop and report it as a blocker. The identifier
   has no place in an unchanged ROS definition; adding `uint32 request_id` to both messages
   changes the type names and the MD5, and then no ROS client and no bridge can call
   it (`ports/map_server`: `map_server_msgs/GetMapResponse` against `nav_msgs/GetMap`).
3. **A service that only hypha nodes call, and the user approves the changed types**: the
   topic pair of cpp-api.md §8. In a cycle node an `Arrivals<Srv::Request>` port on the
   request topic and an `Output<Srv::Response>` on the reply topic; in a plain node a
   `subscribe` and an `advertise`. The node's own package copies the `.srv` and adds the
   identifier field to both messages, the server writes the identifier of the request into
   its reply, and a client keeps its pending requests and its own deadline in its step. Both
   topic names are TOML values (read a name with `hypha::config().required<std::string>(key)`,
   a service name does not resolve from the ports tables). The reply is sent by the step that
   decides it, and a caller that blocks on it is stop-and-ask (above).

### 7.2 Actions

`actionlib` has no counterpart and no issue. Stop and ask. A node whose action server is its
function (a navigation or manipulation server) is a blocker; report it. A node where the
action is a side feature (`arena_camera` has two) may drop it only with the user's approval:
list it in the change description and report it as a gap.

### 7.3 Diagnostics

`diagnostic_updater` has no counterpart: issue #217 ("one answer for `diagnostic_updater`") is
open. What exists: the message types `diagnostic_msgs/DiagnosticArray` and `DiagnosticStatus`
ship with the SDK, `~/state` reports the health of the inputs of a node, and an application
counter has no place in it (#121). A **node whose job is diagnostics** (`diagnostic_aggregator`)
is ported as a node that reads `/diagnostics` with an `Arrivals<diagnostic_msgs::DiagnosticArray>`
port and publishes its own arrays on an `Output`. A node that only reports **its own health**
through `diagnostic_updater` (`twist_mux`, `velodyne_driver`) loses that code: delete the
`Updater`, its tasks and the diagnostic parameters (`diagnostic_frequency_tolerance`), leave one
comment `// diagnostics not ported: Hypha issue #217`, and list them in the change description
and the report. Write no substitute publisher.

### 7.4 A transform lookup that waits

Lookups never wait, by design (#91, closed): the buffer fills while the node runs, on the
thread that runs it, so a callback that waited would block every other callback and, under
replay, the clock it waits for. ROS 2's `lookupTransform` with a timeout,
`ManagedTransformBuffer` (a crop box that waits up to 1 s) and `canTransform` with a timeout
have the same answer. What a port does, by the ROS timeout:

- **The ROS node waits 0 s** (`robot_localization` with `tf_timeout = 0`): do the lookup in
  the step; on a `TransformError` do what the ROS node did on failure (`robot_localization`
  drops the sample) and log it. This keeps the ROS behaviour; no question is needed.
- **The ROS node waits `t > 0`**: the sample is held in the node and the lookup is tried again
  in a later step. Hypha gives no hold limit and no rule for a held sample that has become
  older than the state the node already computed, and the port must not choose them: **ask
  the user**, naming the ROS timeout. State your proposal in the question: hold a sample for
  at most the ROS timeout, counted in the step's time `time.now`; drop it after that and log
  it; drop a held sample that is older than the newest sample already integrated if the ROS
  node does. The key that held the ROS timeout (`transform_timeout`) has no reader, and an
  unread key stops the start: delete it from the TOML file.
- **A node that looks up, at stamp T, the transform it broadcasts at T in the same step**
  (`imu_filter_madgwick`) does not find it: the buffer of a step is filled before the step
  (execution-model.md §6.7 item 1), and what the step sends goes out after it returns. The
  lookup fails with `AfterBuffer`, and the message is sent in the next step. That is how the
  buffer is defined, not a defect. The node either holds its message one step (what the port
  did), or uses the transform it just built, which it holds in a variable, in place of
  the lookup; the second is a logic edit and is asked first.

A step test cannot feed a `Transforms` port yet (#248): test the lookup branches on the bus.

### 7.5 A parameter that changes at run time

`dynamic_reconfigure`, `rosparam set`, a ROS 2 `add_on_set_parameters_callback`: hypha reads
the parameter file once, at start; there is no parameter server and no mutation API
([cpp-api.md](cpp-api.md) §9). The values come from the TOML file and the callback is
removed with its comments (section 2.1). A node that changes its gains from a client at run
time (`Se3Controller` with `Drs`): ask; the owner decision for the MRS ports was to drop
it and take the gains from the file.

---

## 8. Report hypha gaps and blockers

A migration shows where hypha is missing a feature or has a defect. Flag each
one and report it to the developer who gave you the task. The developer
reports it to the hypha maintainers. Do not report it to the maintainers
yourself.

Report these:

- **Blocker:** the node cannot be migrated without the missing item. Example:
  a lookup that must wait for its transform.
- **Gap:** the migration is possible, but a hypha feature would make it
  smaller or safer. Example: a service name
  that does not resolve from the ports tables.
- **Defect:** hypha does not do what its documentation says. Example: a
  verdict, a status code or a rate that differs from the documented one.
- **Documentation fault:** a page in `docs/` that describes an API that does
  not exist, or a missing explanation that cost you time.

Rules:

- **Do not work around a gap in the node.** Do not write a TF buffer, a second
  age check, a parameter reloader or a copy of framework logic in the node to
  fill a gap. Workarounds become second implementations that the framework
  must later replace. Stop at the gap and report it.
- **Continue where the gap does not block.** Finish the parts of the
  migration that the gap does not touch, and mark the open part in the code
  with one comment that names the report.
- **Prove a defect.** Give the smallest command or test that shows it, the
  output you saw and the output that the documentation states. Clear
  stale shared memory with `hypha clean` (the domain stopped) first, so
  that an environment fault is not reported as a defect.

Write each item in this form, in your report to the developer:

```text
[hypha BLOCKER | GAP | DEFECT | DOCS] <one-line title>
Node:        <node and file:line where it occurred>
ROS 1 use:   <the ROS feature or behaviour the node needs>
hypha today: <what hypha offers, with the doc section or header>
Impact:      <what the migration cannot do, or what it had to do instead>
Evidence:    <command, test or output; for a DEFECT the expected and the actual result>
Proposal:    <optional: the feature or fix that would remove it>
```

Put the list of reports at the top of the change description, before the
checklist. A migration with an open blocker is not complete.

---

## 9. Production-ready checklist

Report each item in the change description, with the evidence.

- [ ] Logic files have no diff, except the edits of section 5.
- [ ] No file was reformatted. No comment was removed.
- [ ] Every test that existed before exists and passes.
- [ ] The step has tests for usable, stale and never-arrived inputs, positive
      and negative.
- [ ] No ROS header, `ros::` name or catkin call remains.
- [ ] Every topic comes from `[ports.inputs.<name>]` or `[ports.outputs.<name>]`; required topics have no code
      fallback.
- [ ] Every tuning value comes from the TOML file; each value is checked for
      range (`hypha::constraints`, or an exception in the constructor or `setup()`).
- [ ] Every input port has a `silence_ms` chosen from the input rate.
- [ ] No input has an age limit besides `silence_ms`.
- [ ] The response to each missing or stale input is explicit and approved.
- [ ] `main` only calls `hypha::runNode`, for a `CycleNode` and an `ApplicationNode` alike: one TOML file at most, a second argument stops the node with status 2.
- [ ] The logged snapshot shows the expected topics and period.
- [ ] Step 7 was done on the bus, and its results are recorded.
- [ ] A `hypha launch` unit and an installed TOML file exist.
- [ ] Every blocker, gap, defect and documentation fault found is reported
      to the developer in the form of section 8. No blocker is open.

---

## 10. Porting an event-driven node: an `ApplicationNode`

Steps 1 to 8 end in a `CycleNode`. This section covers the other target: a
`hypha::ApplicationNode` with typed subscriptions, publishers and timers, started by
`hypha::runNode<App>()` like a `CycleNode`. Step 1 (inventory), Step 3 (TOML) and
Step 4 (build), Step 7 (run on the bus) and Step 8 (deploy) apply unchanged, except
the items of Step 7 on input verdicts. The rules of [section 2](#2-rules) apply, except
the cycle items; of [section 9](#9-production-ready-checklist), the items on steps,
`silence_ms` and verdicts do not apply, and the `main`, launch-unit and installed-file
items do. The names below are in [cpp-api.md](cpp-api.md) §1, §3, §4, §5, §7 and §9.

### 10.1 Which target

| The ROS node | Target |
|---|---|
| Its work runs from a timer | A rate `CycleNode` (Step 2) |
| Its work runs on each sample of one or several inputs | A triggered `CycleNode`: `setTrigger(port)` once per input (Step 2) |
| It owns worker threads | An `ApplicationNode` |
| It is a shim of a ROS client library (#59) | An `ApplicationNode` |

What a triggered cycle adds over an `ApplicationNode`, and what it costs:

| A triggered cycle adds | It costs |
|---|---|
| A verdict for each input (`stale()`, `usable()`), and the input rows of `~/state` | A `silence_ms` for each supervised input, every trigger's included |
| Outputs held until the step succeeds: a failed step publishes nothing ([execution-model.md](execution-model.md) §6.1) | A fallback step to handle when every trigger is silent for the shortest `silence_ms` |
| Replay order: under simulated time the step runs at the sample's time, with the other inputs cut before it, and waits for its upstream nodes ([execution-model.md](execution-model.md) §6.6) | One decode of each sample into its port before the step, and a payload copy in the input's history ([execution-model.md](execution-model.md) §6.2) |
| | On host time, a trigger older than its port's `silence_ms` runs no step and counts in that port's `age_rejections_total` |

`runNode` gives both targets `~/introspection`, `~/state`, the refusal of an unread
parameter key and the exit codes of §10.2. An `ApplicationNode` has none of the first
column: freshness is judged for cycle inputs only ([cpp-api.md](cpp-api.md) §5.1).
`Node::run()` calls every callback on one thread, and each `publish()` goes out at
once. The model is in [execution-model.md](execution-model.md) §3, §4 and §11.2. Write
the reason for an `ApplicationNode` in the change description.

### 10.2 The application

Write the node as one class and start it with `runNode`. `main` is one line.

```cpp
#include <hypha/hypha.h>
#include <hypha/transform_buffer.h>

#include <geometry_msgs/PoseStamped.h>
#include <sensor_msgs/PointCloud2.h>

#include <optional>

class Locator final : public hypha::ApplicationNode
{
  public:
    // The bus is not joined yet. Read and check the parameters here: a refused value
    // ends the run with exit 78.
    Locator() : gain_(hypha::config().required<double>("gain", hypha::constraints::positive()))
    {
    }

    // Create every subscription, publisher, timer and transform buffer here, and
    // nowhere else. The members hold them.
    void setup(hypha::Node& node) override
    {
        this->scan_ = node.subscribe<sensor_msgs::PointCloud2>(
            "scan", hypha::bind<&Locator::onScan>(*this)
        );  // [ports.inputs.scan]
        this->pose_ = node.advertise<geometry_msgs::PoseStamped>("pose");  // [ports.outputs.pose]
        this->timer_ =
            node.timer(hypha::Duration::fromMillis(100), hypha::bind<&Locator::onTick>(*this));
        this->tf_.emplace(node);  // TransformBuffer has no default constructor
    }

  private:
    void onScan(const hypha::MessagePtr<sensor_msgs::PointCloud2>& scan)
    {
        geometry_msgs::PoseStamped pose;
        pose.header = scan->header;
        pose.pose.position.x = this->gain_ * static_cast<double>(scan->width);  // the node's logic
        if (!this->pose_.publish(pose)) {
            HYPHA_LOG_WARN_THROTTLE(
                1.0, "pose not sent: {}", this->pose_.lastPublishStatus().name()
            );
        }
    }

    void onTick()
    {
        // periodic work
    }

    double                                       gain_;
    hypha::Subscriber<sensor_msgs::PointCloud2>  scan_;
    hypha::Publisher<geometry_msgs::PoseStamped> pose_;
    hypha::Timer                                 timer_;
    std::optional<hypha::TransformBuffer>        tf_;
};

int main(int argc, char** argv)
{
    return hypha::runNode<Locator>(argc, argv, "locator");
}
```

`runNode` loads the parameter file (`argv[1]`, optional), constructs the class, joins the
bus, calls `setup()`, refuses an unread parameter key, publishes `~/introspection` and
`~/state`, calls `started()` and runs until a stop. An exception ends the run with the
line `node failed in <phase>: <what>`, `failRun()` with `application failed: <reason>`, a
failed run with `node run failed`. It returns an exit code that a supervisor reads:

| Code | Constant | When | `hypha launch` |
|---|---|---|---|
| 0 | `kExitClean` | the run stopped as requested | the unit's restart policy |
| 2 | `kExitUsage` | more than one argument | the unit's restart policy |
| 78 | `kExitConfiguration` | the parameter file, a parameter or an endpoint policy is refused; an unread key; a `hypha::ConfigError` or `ParameterError` thrown by the constructor, `setup()` or `started()` | never restarts |
| 75 | `kExitRewind` | simulated time went back (`Status::ClockRewound`) | always restarts |
| 70 | `kExitRuntime` | anything else, and `failRun()` | the unit's restart policy |

- The constructor and `setup()` throw `hypha::ConfigError` or `hypha::ParameterError` for
  a refused configuration (78); any other exception is 70. `subscribe` and `advertise` throw
  `ConfigError` for a port whose table is missing or refused, so a missing
  `[ports.inputs.<name>]` exits 78.
- `started(hypha::Node&)` runs after every check, just before the run. Start worker
  threads and send latched messages there. **It must return.** The run, with the node's
  `~/state` and its stop, begins only when `started()` returns, and a stop requested during
  it is kept until then (`application_node.h`): a `started()` that blocks (a vendor
  `Start()` that waits for the first packet of the device) leaves a node with no state, no
  traffic and no reaction to SIGTERM. Anything that waits for a device runs on the worker
  thread ([section 10.6](#106-threads)).
- **Throw to refuse the run; the exit code follows the type of the exception.** A
  `hypha::ConfigError` or `hypha::ParameterError` thrown in the constructor, `setup()` or
  `started()` exits 78, which a supervisor does not restart: the right code for a setting
  that no restart can mend (a host address that is not the machine's). Any other exception
  exits 70, and so does `failRun()` from any thread (`exitCodeFor`,
  `crates/hypha-cxx/include/hypha/application_node.h`; the test `startup_nohost` of
  `ports/livox_hap`). A fault that is found only after `started()` has returned, such as the
  size of the first frame of a device, can end the run only as 70 (#264).
- **At least one of several optional outputs is the node's own check.** A deployment may
  switch each of them off, and all off is a valid node that publishes nothing, so the framework
  does not refuse it. Check it in `setup()` after the `advertise` calls and throw
  `ConfigError` (`ports/livox_hap/src/livox_hap_node.cpp`):

```cpp
this->cloud_ = node.advertise<sensor_msgs::PointCloud2>("cloud", hypha::optionalTopic);
this->imu_ = node.advertise<sensor_msgs::Imu>("imu", hypha::optionalTopic);
if (!this->cloud_.enabled() && !this->imu_.enabled()) {
    throw hypha::ConfigError("hypha: no output: set a topic in [ports.outputs.cloud] or [ports.outputs.imu]");
}
```
- `failRun("reason")` ends the run as failed from any thread (70).
- The destructor runs on every way out: its body first, then the members (the endpoints),
  then the node. Stop and join every thread of the class in the body.
- A callback runs in the run, on one thread. It must not block.

A custom `main()` that builds a `hypha::Node` is for a bounded probe or tool that ends
by itself ([cpp-api.md](cpp-api.md) §2). It publishes no `~/state` and no
`~/introspection`, refuses no unread key, and maps its own exit codes. A node that runs
until stopped is never written that way.

### 10.3 Literal topic or port name

`hypha::Topic{...}` names a literal topic that code fixes. A bare string names a
port table of the parameter file. ROS 1:

```cpp
std::string scanTopic;
pnh.param<std::string>("scan_topic", scanTopic, "/scan");   // string parameter
auto scan = nh.subscribe(scanTopic, 5, &Locator::onScan, this);
auto pose = nh.advertise<geometry_msgs::PoseStamped>("pose", 1);
```

hypha, with the topic names in the TOML file, in `setup(hypha::Node& node)`:

```cpp
this->scan_ = node.subscribe<sensor_msgs::PointCloud2>(
    "scan", hypha::bind<&Locator::onScan>(*this));                 // [ports.inputs.scan]
this->pose_ = node.advertise<geometry_msgs::PoseStamped>("pose");  // [ports.outputs.pose]
this->tfOut_ = node.advertise<tf2_msgs::TFMessage>(hypha::Topic{"/tf"});  // a literal topic
```

Assign each endpoint to a member of the application, as in §10.2: an endpoint held by
a local is destroyed when `setup()` returns.

```toml
[ports.inputs.scan]
topic = "/lidar/points"

[ports.outputs.pose]
topic = "~/pose"
```

- The `scan_topic` parameter is gone: the topic is the `topic` key of
  `[ports.inputs.scan]`. A plain entry `scan = "/x"` is refused.
- A topic is absolute (`/lidar/points`) or private (`~/pose`, which is
  `/<node name>/pose`). A relative name is refused with `InvalidTopic`.
- An input that a deployment may leave unwired takes `hypha::optionalTopic`. An
  empty `topic = ""` then disables it, and `enabled()` is false:

```cpp
this->odom_ = node.subscribe<nav_msgs::Odometry>(
    "odom", hypha::bind<&Locator::onOdom>(*this), hypha::optionalTopic);
if (!this->odom_.enabled()) {
    HYPHA_LOG_INFO("odometry input is off");
}
```

### 10.4 Time, status and logging

| ROS 1 | hypha | Meaning |
|---|---|---|
| `msg.header.stamp.toSec()` | `hypha::fromRosTime(msg.header.stamp)` (`hypha/message_stamp.h`) | Returns `std::optional<hypha::Time>`. Empty when `nsecs` is one second or more. `Time::nanos()` gives integer nanoseconds. |
| `header.stamp = ros::Time::now()` | `header.stamp = hypha::toRosTime(hypha::now())` | Throws `std::out_of_range` for a time before the epoch or after 2106. |
| `ros::spin()` | `hypha::runNode<App>(argc, argv, "name")` | Runs `node.run()` and returns the exit code of §10.2. |
| `ros::shutdown()` from another thread | `hypha::requestStop()` | The one call that another thread may make on a running node. For a failure, `failRun(reason)` in the application. |
| `pub.publish(msg)` (no result) | `publish(msg)` returns `bool` (`[[nodiscard]]`); `lastPublishStatus()` returns the `Status` of the last attempt | The status names the reason: a disabled publisher, a message that is too large, an exhausted pool, a transport failure. |
| `ROS_WARN_THROTTLE(1, ...)` | `HYPHA_LOG_WARN_THROTTLE(1.0, "{}", x)` | The first call writes, later calls at most once per period. Host monotonic time. |
| `ROS_WARN_ONCE` | `HYPHA_LOG_WARN_WHEN(true, false, ...)` | The first call writes. |
| `ROS_WARN_COND(cond, ...)` | `if (cond) { HYPHA_LOG_WARN(...); }` | Writes each time `cond` is true. `HYPHA_LOG_WARN_WHEN(state, false, ...)` is not it: it writes only when `state` changes from false to true at that call site. |
| `pub.getNumSubscribers()` | `pub.subscriberCount()` | `std::optional<size_t>`. Empty: the count is unavailable (the publisher is disabled, or the transport cannot count). `0`: counted, no subscriber now. |

Do not gate optional work on an empty count. Skip it only for a counted zero:

```cpp
const std::optional<size_t> count = pub.subscriberCount();
const bool nobodyListens = count.has_value() && *count == 0U;
if (!nobodyListens) {
    pub.publish(buildDebugMessage());
}
```

The count is the subscribers of every node on the machine at that instant. It is
not delivery. A cycle `Output::hasSubscribers()` treats an empty count as watched.

### 10.5 Queue depth

A subscriber queues the `maxDepth` of its topic's policy. The limit of 32 belongs
to the `default` policy. A topic that needs a deeper queue, such as a ROS IMU
subscriber with `queue_size` 200, needs its own policy. Declare it once, in the
policy header that every node of the stack includes, and map the message type to it:

```cpp
// my_stack/qos_policies.h: included by every node of the stack
#include <hypha/qos.h>
#include <sensor_msgs/Imu.h>

namespace my::policies {
inline constexpr hypha::Qos imu{
    .name = "imu", .publishers = 2, .subscribers = 4, .maxDepth = 200};
}

template <>
struct hypha::QosOf<sensor_msgs::Imu>
{
    static constexpr hypha::Qos value = my::policies::imu;
};
```

- `maxDepth` is 1 to 1024. `publishers` and `subscribers` are 1 to 256. `name` is 1
  to 32 letters, digits, `_`, `-` or `.`. A value outside is refused with
  `InvalidArgument`.
- Every typed endpoint of the topic declares the same name and limits (through
  `QosOf<M>` or the `hypha::Qos` option). A different policy is refused with
  `QosMismatch`: `advertise` and `subscribe` throw `hypha::ConfigError`.
- The endpoint that opens a topic first creates it. A raw endpoint, a player
  (`hypha play`) or a bridge declares no policy, so it creates the topic with
  `default`, and a typed endpoint with the deeper policy is then refused. Start the
  application nodes first, or map the type to `default`.

### 10.6 Threads

`Node::run()` runs every subscription and timer callback on one thread. The framework
creates no thread. One endpoint belongs to one thread at a time. A worker thread that
publishes therefore owns its publisher, or every access to that publisher is serialized
by the application. Only `hypha::requestStop()` and `failRun()` may be called from another
thread while the node runs.

The application starts a worker in `started()` and joins it in its destructor. The
worker's loop checks a stop flag that the destructor sets, and every blocking read in it
takes a timeout that returns to that check: a read with no timeout never sees the stop,
and the join then waits forever. A thread that fails calls `failRun()`:

```cpp
#include <hypha/hypha.h>

#include <sensor_msgs/PointCloud2.h>

#include <atomic>
#include <exception>
#include <thread>

sensor_msgs::PointCloud2 readFromDevice();  // the driver's blocking read, with a timeout

class LidarNode final : public hypha::ApplicationNode
{
  public:
    ~LidarNode() override
    {
        this->stopping_ = true;
        if (this->worker_.joinable()) {
            this->worker_.join();
        }
    }

    void setup(hypha::Node& node) override
    {
        this->points_ = node.advertise<sensor_msgs::PointCloud2>("points");
    }

    void started(hypha::Node& /*node*/) override
    {
        this->worker_ = std::thread(&LidarNode::readLoop, this);  // after the checks
    }

  private:
    // The only user of points_ once the thread runs.
    void readLoop()
    {
        while (!this->stopping_) {
            try {
                const bool sent = this->points_.publish(readFromDevice());
                if (!sent) {
                    HYPHA_LOG_WARN_THROTTLE(
                        1.0, "cloud not sent: {}", this->points_.lastPublishStatus().name()
                    );
                }
            } catch (const std::exception& error) {
                this->failRun(error.what());
                return;
            }
        }
    }

    std::atomic<bool>                          stopping_{false};
    hypha::Publisher<sensor_msgs::PointCloud2> points_;
    std::thread                                worker_;
};
```

A bounded hand-over from a callback to a worker is in
[examples/sensor_pipeline](../examples/sensor_pipeline/README.md) (`CloudHandover`).

**A paced worker: `ros::Rate` and `ros::Duration::sleep()` in a thread.** hypha has no
`Rate`, and no sleep that ends at the node's stop; `Node::timer` runs on the run thread,
where a blocking device read must not. A worker that paces itself carries the stop through
every wait: a `std::jthread` whose `std::stop_token` the destructor requests, and a sleep
that returns early at the stop. Every other blocking call of the loop takes a timeout that
returns to the same check (`ArenaDevice` slices a 5 s `GetImage` wait in 100 ms steps). The
loop of `ports/arena_camera` (`src/arena_camera_node.cpp`, `inc/arena_camera/stop_sleep.h`):

```cpp
// Sleeps for duration, or until stop is requested. False when it was requested.
inline bool sleepFor(std::stop_token stop, std::chrono::nanoseconds duration)
{
    std::mutex                  mutex;
    std::condition_variable_any wake;
    std::unique_lock            lock(mutex);
    wake.wait_for(lock, stop, duration, [] { return false; });
    return !stop.stop_requested();
}

void started(hypha::Node& /*node*/) override
{
    this->worker_ = std::jthread([this](std::stop_token stop) { this->run(std::move(stop)); });
}

void run(std::stop_token stop)
{
    const std::chrono::nanoseconds period = std::chrono::milliseconds(100);
    auto                           next = std::chrono::steady_clock::now();
    while (!stop.stop_requested()) {
        this->grab(stop);  // every blocking call inside takes a timeout and the token
        next += period;
        const auto now = std::chrono::steady_clock::now();
        if (now > next + period) {
            next = now;  // a turn that took longer than a period does not make up the time
        } else if (!sleepFor(stop, next - now)) {
            break;
        }
    }
}
```

The destructor calls `this->worker_.request_stop()` and joins in its body, as in the example
above, so the worker ends before the members it uses (the endpoints) are destroyed. The pacing
runs on the host clock: under `HYPHA_USE_SIM_TIME` the worker paces in real time while
`hypha::now()`, which stamps the frames, is simulated. A framework call that does this
(a stop token tied to the node, `sleepFor`, a `Rate`) is requested in #263; it does not exist.
A loop with nothing that blocks is not a worker: it is a rate cycle ([Step 2](#step-2-design-the-cycle)).

**A connect-and-retry loop of a driver** (`while (ros::ok()) { try to open; sleep 1 s; }`).
The loop must not sit in a callback, which must not block (§10.2). Choose by whether one
attempt can block:

- *An attempt returns at once* (a non-blocking `open`, a `connect` with a short timeout): **one
  attempt for each tick of a `Node::timer`** on the run thread; a failed attempt waits for the
  next tick. `ports/nmea_comms/src/serial_node.cpp` (a 1 s timer).
- *An attempt can block, or the device waits to answer*: the loop runs on the worker, with
  `sleepFor(stop, retry_period)` between attempts, and an INFO or WARN line at a bounded
  rate while it waits (`ArenaCameraNode::waitForCamera`, which logs every 15 s).

**A push from a thread the node does not own** (a vendor SDK's callback thread). hypha
offers no hand-over type and no call that runs a function on the run thread: issue #233 is
open. The foreign thread touches no endpoint. It pushes the message into a bounded, counted,
drop-oldest queue that the application owns (a mutex and a deque), and a worker that the
application started pops with a timeout and publishes, as every worker above does. The
examples: `CloudHandover` of `examples/sensor_pipeline/src/cloud_processor.cpp` (pushed
from the run thread), and `ports/hesai_lidar/inc/hesai_lidar/handover.h` (111 lines, pushed
from the SDK's thread; its test `MessagesPushedFromAnotherThreadArriveInOrder`). The
destructor stops the queue, then joins the worker, then stops the vendor SDK. Whether the run
thread's housekeeping of a **latched** publisher may run while a worker publishes on the same
`Publisher` is not stated and was not exercised: send latched messages in `started()` (above),
and when a worker must publish a latched output, put the question in the report (#233).

`started()` does not block ([section 10.2](#102-the-application)): a driver whose start waits
for a device (`sdk.Start()`) calls it on the worker.

A timer counts the periods it did not fire in, because a callback or the run loop
ran longer than one period:

```cpp
const uint64_t missed = this->timer_.missedCount();   // periods never fired in
```

Read `missedCount()` where the node can log it. A subscriber counts the samples it lost
before it read them, and the frames it refused:

```cpp
const uint64_t lost = this->scan_.missingSequences();      // frames lost before they were read
const hypha::SubscriberMetrics m = this->scan_.metrics();  // m.sequenceGap, m.invalidFrame, ...
```

`missingSequences()` grows on a queue overflow. `m.sequenceGap` counts frames that
arrived and were refused because their sequence was not newer; a lost frame does not
raise it. Both may be read from any thread while the node runs
([cpp-api.md](cpp-api.md) §7).

### 10.7 TF in an application

The facts of [cpp-api.md](cpp-api.md) §10 apply:

- Dynamic transforms go to `/tf`: `node.advertise<tf2_msgs::TFMessage>(hypha::Topic{"/tf"})`.
- A static broadcaster publishes on `/tf_static`, latched. A late subscriber gets only
  the last message, so the broadcaster keeps every static transform it owns, keyed by
  child frame, and sends the whole set each time. `hypha::StaticTransformBroadcaster`
  does this: `statics.send(baseToLidar)` keeps the transform and publishes the set.
- `hypha::TransformBuffer` looks a transform up at a message stamp. The run of the node
  fills it from `/tf` and `/tf_static`; a lookup never waits and may come from any
  thread, a worker thread too:

```cpp
#include <hypha/transform_buffer.h>

// members of the application
std::optional<hypha::TransformBuffer> tf_;
std::optional<hypha::StaticTransformBroadcaster> statics_;

// in setup(hypha::Node& node):
this->tf_.emplace(node);
this->statics_.emplace(node);

// in started(), after the checks:
if (!this->statics_->send(baseToLidar)) {
    throw std::runtime_error("/tf_static not sent");
}

// in a callback, or in a worker thread while the node runs:
const std::optional<hypha::Time> stamp = hypha::fromRosTime(scan.header.stamp);
if (stamp.has_value()) {
    const auto found = this->tf_->lookup("map", "lidar", *stamp);
    if (!found) {
        // found.error() is a hypha::TransformError: UnknownFrame, NotConnected,
        // BeforeBuffer, AfterBuffer or InvalidFrame
    }
}
```

- A lookup at the stamp of a transform that the same node sends in the same step misses it,
  and a node that holds a message until a transform arrives has the rules of
  [section 7.4](#74-a-transform-lookup-that-waits).
- The application creates the buffer in `setup()` and destroys it with itself, as the
  header requires, and joins a worker thread before that. Nothing is ingested unless the
  node runs. The buffer keeps 10 s of dynamic transforms per frame unless the constructor
  gets another duration. `hypha::Transforms` is the port of a cycle; its lookup throws
  `std::logic_error` outside a step.

### 10.8 A list of scalars in the parameter file

A TOML array of numbers, flags or text is read with `requiredArray`:

```cpp
const std::vector<double> weights = hypha::config().requiredArray<double>("voxel_weights");
const std::array<double, 3> extrinsic = hypha::config().requiredArray<double, 3>("lidar_extrinsic");
const std::optional<std::vector<double>> offsets = hypha::config().optionalArray<double>("offsets");
```

`requiredArray<T>` returns a `std::vector<T>`; `requiredArray<T, N>` returns a
`std::array<T, N>` and throws `ParameterError` for any other length (`hypha: parameter
'lidar_extrinsic': expected 3 elements, found 4`). `optionalArray` returns an empty
`std::optional` only when the key is absent. Each element gets the conversion and the
range check of `required<T>`, and an error names the element, for example
`lidar_extrinsic[2]`. Check the range of the values in the application. `ConfigList`
remains for a list of tables (`[[sensors]]`).

### 10.9 A runnable sensor node and its replay check

[examples/sensor_pipeline](../examples/sensor_pipeline/README.md) applies 10.2 to 10.8 in
two `ApplicationNode`s: a 400 Hz IMU with `maxDepth = 200`, 10 Hz point clouds of 262144
bytes of data, one timer, and one worker thread that owns the result publisher, looks the
transform up at the cloud's stamp and publishes. Its replay recipe, `replay_check.sh`,
runs the node on simulated time from a recorded fixture of 98296 bytes with `hypha play
--clock` and `hypha record --sync`, and makes three checks:

- **Loss.** The node prints `missingSequences()` and `metrics()` of each subscriber. A
  replay at `--rate 1` reports `missing=0`; slow mode (`[slow] enabled = true`) shows a
  growing count, live and in a replay, since nothing waits for an `ApplicationNode`.
- **Timer misses.** The node prints `Timer::missedCount()`; the stalls of slow mode
  show as misses.
- **Comparison by stamp.** `sensor_pipeline_compare` reads two recordings with the test
  MCAP reader and pairs the outputs by `header.stamp`: two runs of the node, or a run and
  the ROS node's golden output.

An `ApplicationNode` on simulated time takes no part in the replay lockstep (#153): its
callbacks run as their samples arrive, and the player does not wait for it
([execution-model.md](execution-model.md) §11.2). Its replays are not identical from
run to run; a node that needs a deterministic replay is built from cycles.

### 10.10 Test an application

`StepHarness` runs cycle steps, and an `ApplicationNode` has none. Test it at four levels,
as [examples/sensor_pipeline](../examples/sensor_pipeline/README.md) does for the first three:

1. **Logic.** Keep the ROS node's logic in classes that need no `Node`, and test them
   as in Step 6. A callback of the application then only calls them.
2. **Start-up.** Run the binary with a missing parameter file and a malformed one, and
   expect exit 78; and, **for a node with a required port**, one without its `[ports.*]`
   tables: `startup_exit_code.cmake`, registered for each case in the example's
   `CMakeLists.txt` with `hypha_add_test`. The case "a file without its `[ports.*]` tables
   exits 78" holds only when a port is required. A node whose outputs are all `optionalTopic`
   (a driver whose outputs are each off unless a topic is set) runs with every output off and
   exits 0, so the case does not apply there: test that node's own rule instead (for "at least
   one output", a file with every topic empty exits 78, §10.2).
   `startup_exit_code.cmake` is not installed with the SDK, but `hypha_add_test` is
   (`hyphaTesting.cmake`). A port against the installed SDK copies
   `examples/sensor_pipeline/startup_exit_code.cmake` from the hypha source tree into its
   own test folder, and registers each case with `hypha_add_test` and `-P` on that copy, as
   the example's `CMakeLists.txt` does. Installing the script beside `hyphaTesting.cmake`
   is a follow-up.
3. **The bus.** Replay a recorded input into the running node and compare its output
   by stamp: `replay_check.sh`, registered as `sensor_pipeline_replay_check`. The
   recipe, and the comparison with a ROS golden, are in the example's README.
4. **The application in the process of the test, against a fake device.** The node of a
   driver (`ports/arena_camera/test/test_node.cpp`) is driven by hand: a `hypha::Node` of
   the test that loads the test's parameter file, the application built **after** it (its
   constructor reads `hypha::config()`), a fake device that the test controls, and a
   subscriber of the test on the same node. `setup()` and `started()` are what `runNode`
   calls:

```cpp
hypha::Node bus("arena_camera_test", "test/camera.toml");  // the file has [node] name = "arena_camera_node"
ArenaCameraNode app;
app.setup(bus);
app.started(bus);  // starts the worker, which reads the fake device
auto frames = bus.subscribe<sensor_msgs::Image>(
    hypha::Topic{"/arena_camera_node/image_raw"}, hypha::bind<&Received::onImage>(received)
);
const hypha::RunResult result = bus.runFor(hypha::Duration::fromSeconds(1.0));  // delivers to the subscriber
EXPECT_TRUE(result.ok());
```

   One process reads one parameter file ([cpp-api.md](cpp-api.md) §9), so a case with
   another configuration is another process: the port registers nine entries of one test
   executable, each with its own `--gtest_filter`.

### 10.11 N subscriptions named by a list

A node whose inputs are named by a parameter (a multiplexer with a list of velocity
topics) needs no code for each name. A name in `subscribe` is the name of a port table,
and a list of names is a TOML array of text. The array holds the names and each name its
own table `[ports.inputs.<name>]` with the topic: the framework cannot list the tables of
`ports.inputs` for you (#234), so the application keeps the list. A functor that holds the
index tells the callbacks apart; the subscribers go in a `std::vector`:

```toml
velocity_inputs = ["navigation", "joystick"]

[ports.inputs.navigation]
topic = "/nav_vel"
[ports.inputs.joystick]
topic = "/joy_vel"
```

```cpp
class Mux final : public hypha::ApplicationNode
{
  public:
    // The parameters are read here, before the bus.
    Mux() : names_(hypha::config().requiredArray<std::string>("velocity_inputs"))
    {
    }

    void setup(hypha::Node& node) override
    {
        this->out_ = node.advertise<geometry_msgs::Twist>("cmd_vel_out");
        for (std::size_t index = 0; index < this->names_.size(); ++index) {
            this->inputs_.push_back(
                node.subscribe<geometry_msgs::Twist>(this->names_[index], OnInput{this, index})
            );
        }
    }

  private:
    struct OnInput
    {
        Mux*        mux;
        std::size_t index;

        void operator()(const hypha::MessagePtr<geometry_msgs::Twist>& message) const
        {
            this->mux->onTwist(this->index, *message);
        }
    };

    void onTwist(std::size_t index, const geometry_msgs::Twist& twist);  // the node's logic

    std::vector<std::string>                             names_;
    hypha::Publisher<geometry_msgs::Twist>               out_;
    std::vector<hypha::Subscriber<geometry_msgs::Twist>> inputs_;
};
```

A node of this shape that wants a verdict for each input (the age, `~/state`) is not this
target: it is a triggered `CycleNode` whose constructor builds one `Arrivals` port for each
name and keeps them in a `std::list` (the order of the sentences in the introduction,
`ports/twist_mux`). A name in the array whose table has no `topic` fails the start with a
refused port table, and exits 78 ([section 10.3](#103-literal-topic-or-port-name)).

---

## 11. Porting a ROS 2 node

The guide above is written for ROS 1. A ROS 2 (`rclcpp`) node follows the same steps, the
same rules and the same target choice; this section maps what is different. The message
names come first, because everything else compiles only with them. Evidence for each row is
from `ports/autoware_voxel_downsample` (a ROS 2 node of Autoware 2.1.0) and from the
vetting of the MRS and Autoware nodes.

### 11.1 The message names

ROS 2 code includes `<sensor_msgs/msg/point_cloud2.hpp>` and names
`sensor_msgs::msg::PointCloud2`. hypha's generated headers keep the ROS 1 path
(`<sensor_msgs/PointCloud2.h>`), and **the ROS 2 names are generated by the application**,
with one CMake call after the packages are declared (the standard packages need no
declaration):

```cmake
find_package(hypha CONFIG REQUIRED)
# The ROS 2 names of the upstream core, as aliases of the SDK's sensor_msgs types.
hypha_generate_ros2_headers(voxel_ros2_headers PACKAGES sensor_msgs std_msgs)
target_link_libraries(voxel_filter PUBLIC voxel_ros2_headers hypha::sdk Eigen3::Eigen)
```

- Each message gets `<package>/msg/<name>.hpp` (`Vector3Stamped` is `vector3_stamped.hpp`) and
  each service `<package>/srv/<name>.hpp`. A package of the node's own is declared with
  `hypha_generate_messages` first and then named in `PACKAGES`; an undeclared package stops the
  configure. The ROS 2 name is a **namespace alias of the ROS 1 type**, so
  `sensor_msgs::msg::PointCloud2` and `sensor_msgs::PointCloud2` are one type
  (`static_assert(std::is_same_v<...>)` compiles; nothing is copied when one is passed as the
  other). The node file may spell it either way; keep the core's spelling in the core.
- The type is the ROS 1 type, so its fields keep their ROS 1 names: `header.stamp.secs` and
  `.nsecs` (ROS 2: `sec`, `nanosec`), and `header.seq` exists. Code that reads a ROS 2 field name
  fails to compile; it does not read another value. A `builtin_interfaces::msg::Time` field of
  a message is named as ROS 2 names it.
- `<sensor_msgs/point_cloud2_iterator.hpp>`, `point_field_conversion.hpp`, `image_encodings.hpp`
  and `distortion_models.hpp` are installed with the SDK: the include lines stay
  ([cpp-api.md](cpp-api.md) §1.1).

### 11.2 The rows

| ROS 2 | hypha | The trap |
|---|---|---|
| `rclcpp_components`, `RCLCPP_COMPONENTS_REGISTER_NODE(Cls)`, `Cls(const rclcpp::NodeOptions&)` | A class derived from `hypha::CycleNode` (or `ApplicationNode`) with a default constructor, and a `main` that is `return hypha::runNode<Cls>(argc, argv, "name");`. The target is chosen as in the introduction. `NodeOptions`, `use_intra_process_comms` and `ComposableNode` are dropped: one process has one node | A container that loads several components is several processes, with one TOML file each (Step 8) |
| `declare_parameter<T>("x", default)`, `get_parameter("x")`, `get_parameter_or` | `hypha::config().required<T>("x")`; `optional<T>("x", fallback)` only for a value whose fallback is safe in every deployment (rule 2.2). A parameter descriptor with a range is `hypha::constraints::range(a, b)`. A name with a dot (`a.b`) is the key `b` of the table `[a]` of the TOML file, read as `hypha::config("a").required<T>("b")`. The node reads each parameter once, in the constructor or `setup()` | A declared parameter that the file leaves out has no default: the first required key stops the node. A parameter that changes at run time (`add_on_set_parameters_callback`): [section 7.5](#75-a-parameter-that-changes-at-run-time) |
| `create_subscription<M>(topic, rclcpp::QoS(n), cb)` and `QoS{n}`, `keep_last(n)` | A **callback that does work for each message**: an `Arrivals<M>` port declared with `hypha::perStep(n)`. **The newest value only**: a `Latest<M>` port, which takes the policy's depth and no number of yours. A plain `ApplicationNode` subscriber queues the `maxDepth` of its topic's policy (Step 4 "Queue sizing"). **A depth above 32** needs a policy of its own ([section 10.5](#105-queue-depth)) | In a rate cycle the number is the samples of one period, not the ROS depth (section 4, `queue_size`). Publishers set no depth: `create_publisher<M>(topic, QoS(n))` is `Output<M>` or `advertise`, and `n` is dropped |
| `rclcpp::SensorDataQoS().keep_last(n)`, `.best_effort()`, `.reliable()` | **hypha has no reliability setting**: there is no best effort and no reliable. The depth becomes the input's depth, as in the row above (`ports/autoware_voxel_downsample`: `keep_last(3)` is `perStep(3)` and a policy with `maxDepth = 3`). A full queue drops its oldest samples and counts them (`lost()`, [communication-integrity.md](communication-integrity.md)); nothing waits for a slow reader | The loss policy is "oldest first, counted" for every port. A node that needs another one: stop and report |
| `rclcpp::QoS(n).transient_local()` | `hypha::latched` on the `Output` (or on `advertise`). A late subscriber gets **the last** sample, whatever `n` is: the latch keeps one (`publisher_history_size` is 1, [interface-specification.md](interface-specification.md)) | A `keep_last(n)` history of `n > 1` for late joiners has no counterpart: report it |
| `.durability_volatile()`, `.deadline(...)`, `.lifespan(...)`, `.liveliness(...)` | `volatile` is the default. The others have no row: an input age limit is `silence_ms` (Step 2); stop and ask for the rest | |
| `MutuallyExclusiveCallbackGroup`, `ReentrantCallbackGroup`, `create_callback_group`, `SingleThreadedExecutor`, `MultiThreadedExecutor`, `rclcpp::spin`, `executor.add_node` | Delete them. Every callback, timer and cycle step of a node runs on one thread, one at a time ([execution-model.md](execution-model.md) §10.1), so there is nothing to separate. A `std::mutex` that only separated callback groups goes too (section 5). `rclcpp::spin(node)` is `runNode` | A callback that blocks until another callback has run (a client call that waits on a future) cannot work: [section 7.1](#71-services) |
| `create_wall_timer(period, cb)`, `create_timer` | A rate cycle (`rate_hz`, Step 2), or `Node::timer` in an `ApplicationNode`. A second timer, or a timeout created in a callback: Step 2 and #267 | |
| `this->now()`, `get_clock()->now()`, `rclcpp::Time(msg->header.stamp)`, `rclcpp::Duration` | In a step `time.now` and `time.stamp`; elsewhere `hypha::now()`. `rclcpp::Time(stamp)` is `hypha::fromRosTime(stamp)` (an `std::optional`); `Time::seconds()` and `Duration::seconds()` are the double nearest the exact value; arithmetic keeps its meaning (section 5). `toRosTime` throws for a time below zero, so `stamp - 1 s` of a stamp under one second is not a stamp: check it | |
| `RCLCPP_INFO(get_logger(), "x {}", v)`, `RCLCPP_WARN_THROTTLE(get_logger(), *get_clock(), 1000, ...)` | `HYPHA_LOG_INFO("x {}", v)`; `HYPHA_LOG_WARN_THROTTLE(1.0, ...)` | **The ROS 2 throttle period is in milliseconds, hypha's is in seconds**: `1000` becomes `1.0`. `RCLCPP_*_ONCE` and `_COND`: the rows of section 4 |
| `tf2_ros::Buffer`, `TransformListener`, `lookupTransform(target, source, tf2::TimePointZero)`, `ManagedTransformBuffer` | A `hypha::Transforms` port in a cycle, a `hypha::TransformBuffer` in an `ApplicationNode`. `TimePointZero` (the latest) is `lookupLatest(target, source)`; a lookup at a stamp is `lookup(target, source, stamp)`. Neither waits | `lookupTransform` with a timeout, and `canTransform` with one (a crop box that waits 1 s): [section 7.4](#74-a-transform-lookup-that-waits) |
| `create_service`, `create_client`, `rclcpp_action` | [section 7.1](#71-services), [section 7.2](#72-actions) | |
| `rclcpp::Publisher::borrow_loaned_message`, `LoanedMessage` | No typed loan on `develop`: [section 12](#12-large-messages) | |
| `rclcpp_lifecycle` (`configure`, `activate`) | No counterpart: the lifecycle of a node is construction, `setup()`, `started()`, the run ([section 10.2](#102-the-application)). Stop and ask | |
| `std::shared_ptr<const M>` callbacks, `M::SharedPtr`, `ConstSharedPtr`, `UniquePtr` | The generated message carries all of ROS 2's pointer typedefs ([cpp-api.md](cpp-api.md) §1.1). A cycle port gives a `const M&` valid for the step, not a pointer (section 5, `ConstPtr` row): a core that takes a shared pointer gets one copy per step (`std::make_shared<const M>(sample)`) | |
| `autoware::agnocast_wrapper` macros, `rclcpp::Logger` passed as an argument | Not covered: stop and ask | |

### 11.3 The logic edits that a ROS 2 file needs besides the rows

The edits of [section 5](#5-message-types-and-the-minimal-logic-edits) apply as written:
a third-party vocabulary type (`tl::expected`), an include that only brings Eigen (a
`pcl_conversions` include in a file with no `pcl::` name), `tf2` and `angles` headers. Mark each
logic line that you edit with a `// hypha port:` comment, and count them.

---

## 12. Large messages

A message with a variable-size field (the `data` of an `Image` or a `PointCloud2`) is
**growable** unless its publisher names its largest message. A growable topic reserves its
pool at the first slot size and grows the segment when a message needs more, checks nothing
against `/dev/shm`, and an undersized `/dev/shm` stops the process with SIGBUS
([cpp-api.md](cpp-api.md) §4, [engineering-traps.md](engineering-traps.md)). The default
policy, sized for small messages, would reserve 649 slots of the largest message.

### 12.1 Sizing a topic

1. **The largest serialized message.** The `data` bytes (`height * step` for an `Image`,
   `width * height * point_step` for a `PointCloud2`) plus the fixed fields and the strings
   (`frame_id`, `encoding`, field names): 53 bytes more for an `rgb8` image with frame id
   `arena_camera`, and 156 for a PandarXT-32 cloud: 64,000 points of 26 bytes and 156 bytes
   are 1,664,156 bytes, which `hypha topic bw <topic>` prints as `mean size`. There is no call
   that gives the serialized size of a message, so add an allowance (the camera port adds 4096).
   The point count of a lidar mode is not in the ROS source. **Take the bound from a key of
   the TOML file** (`image_max_bytes`, `cloud_max_bytes`, `max_points_per_frame`), read with a
   range, and give it to the publisher:

```cpp
this->image_raw_ = node.advertise<sensor_msgs::Image>(
    "image_raw", hypha::maxPayloadDynamic(this->image_max_bytes_ + kImageOverheadBytes)
);
```

   (`ports/arena_camera/src/arena_camera_node.cpp`; on an `Output` the same tag.) A message
   above the bound is refused: `publish()` returns false with `lastPublishStatus()`
   `TooLarge` (`ports/hesai_lidar`: `cloud_max_bytes = 1_000_000` gave `/lidar_points not
   sent: TooLarge`). The refusal is a behaviour change from ROS, which sent any size: report it.
2. **A policy for the topic** ([section 10.5](#105-queue-depth)): `publishers`, `subscribers`
   (the readers you declare, not the default 16), `maxDepth` (the deepest queue a reader asks
   for: 2 or 3 for a cloud or an image), mapped to the message type in a header that both ends
   include. `QosOf<M>` is one policy for a message type in one translation unit: two lidar
   drivers that each map `sensor_msgs::PointCloud2` cannot be built into one unit (#241); give a
   port its own policy with the `hypha::Qos` option of the port instead of the map.
3. **The pool** is the number of slots times the slot size:

   `slots = (subscribers + 3) * (maxDepth + 2) + 1 + loans`

   where `subscribers` and `maxDepth` are the policy's, the 3 are the slots reserved for tools
   that observe the topic (`hypha topic`, `hypha record`), the 1 is the latched sample and
   `loans` is the publisher's own (2 in the two figures below). A slot holds the bound plus 104
   bytes of headers. The camera's `(2 + 3) * (2 + 2) + 1 + 2 = 23` slots of a 15 MB `rgb8`
   image reserve 346 MB; the Hesai cloud with 2 subscribers and depth 3 has 28 slots and
   reserves 50.4 MB; the `default` policy, 649 slots of 1.8 MB, would reserve 1.17 GB. A
   bounded publisher reserves address space of `slots * bound` and only the samples that were
   written are resident, but readers that drain at different rates have written every slot, so
   **budget the whole pool**: `/dev/shm` must hold it, and a bounded publisher whose full pool
   exceeds the free space logs `may need <n> bytes of shared memory` at creation. The formula
   is `qos::pool_samples` and its derivation is in
   [engineering-traps.md](engineering-traps.md) ("A growable publisher keeps bookkeeping ..." and
   the `/dev/shm` entry). `hypha topic`, `hypha node` and `hypha top` print no pool size, so the
   number is arithmetic.

### 12.2 A message whose size the node cannot state

A scan of a variable number of packets (`velodyne_driver` with `cut_angle`) has no maximum that
the node can compute. Two answers, in this order:

- **Ask the user for the largest message the deployment may produce**, put it in the TOML file
  as above, and let the node refuse a larger one. This is what the camera, the Hesai and the
  Livox ports did.
- **If the user will not give a bound, leave the publisher growable**: it is supported, and
  the start-up log says what it costs. Check `/dev/shm` of the target against
  `slots * the largest message you measured` (`hypha topic bw`); no bound is checked for you.

**The growable-payload INFO line.** `topic '<t>' has a growable payload; its later segments
are not checked against /dev/shm, and an undersized /dev/shm can stop the process with SIGBUS`
describes the **publisher** of the topic. A subscriber cannot name the largest message it
accepts (no option exists), so the line for an input topic goes away only when its publisher
names a bound with `maxPayloadDynamic`; when the publisher is another node, leave the line. The
node's own `~/state` and `~/introspection` topics log the same line at every start; it is
expected.

### 12.3 Copies and zero-copy today

What exists on `develop`, and nothing else:

- **A typed `publish(const M&)` encodes the message into a slot of shared memory**, and an
  `Output::send` into a loan of its step: one copy of the payload each, and a second one when
  the node first builds the message from the device's buffer (`copyPixels` in `arena_camera`).
  The measured cost at 30 Hz: 2.6 % of a core for a 5 MP `mono8` image and 5.5 % for a 15 MB
  `rgb8` one (medians 0.13 and 0.44 ms for the two copies of the `mono8` image).
- **A typed subscriber decodes into an owning message**, and a cycle port gives a
  `std::span<const M>` of decoded messages: one copy per message, and one more when a core
  wants a `std::shared_ptr<const M>`.
- **Zero-copy exists only in the raw layer** (`rawPublisher` and its loan, `subscribeRaw`,
  `examples/large_payload`), which needs the wire format written by hand. Raw endpoints are for
  tools and examples; a new application use needs the owner's approval
  ([features.md](features.md) §3.12). A port does not use them.
- **A typed loan**, a message whose `data` is resized and filled in place in the slot, is **not
  merged**: #255 and the pull request #261 are open. Write no code that assumes
  `Publisher::loan()`. A port whose copies matter reports the measured cost against #255.
- `nodelet` and in-process sharing between nodes have no counterpart ([section 7](#7-stop-and-ask)):
  nodes are processes, and a subscriber reads from the shared memory of the publisher.
