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
of each input, and a test at each of three levels. The reference for a plain node is
[examples/sensor_pipeline](../examples/sensor_pipeline/README.md) (section 10).

A node is a `hypha::CycleNode` with its ports as members, started by
`hypha::runNode<App>()` (see [cpp-api.md](cpp-api.md) §5.4). A node whose ports
are built at run time uses `hypha::Cycle` ([cpp-api.md](cpp-api.md) §5.5).

---

## 1. When to use this guide

Use it for one ROS 1 C++ node that subscribes, computes and publishes, with or
without timers, parameters and services. The procedure ends in a `CycleNode`. An
event-driven node that handles each message as it arrives is ported to a plain
`hypha::Node` instead: see
[section 10](#10-porting-an-event-driven-node-to-a-plain-hyphanode).

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
  names a ROS call that is gone).
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
  environment variable for it.
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
- A parameter key that nothing read stops the node at start.
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

Map the inventory to one cycle for each timer rate. A node with one timer has
one cycle. One process has one `CycleNode`; a second rate is a composed
`hypha::Cycle` with its own name.

| ROS 1 | hypha cycle |
|---|---|
| Timer callback that computes and publishes | The `step()` method of the cycle node, at `rate_hz` under `[cycle]` in the TOML file. |
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

A node with no timer, whose whole work is in one subscription callback, also
becomes a cycle node: use an `Arrivals` port and choose `rate_hz` above the input
rate. Use a plain callback subscriber ([cpp-api.md](cpp-api.md) §5.2) only
when a latency below one cycle period is a requirement. Write that reason in
the change description. A node of that kind is ported to a plain `hypha::Node`
as [section 10](#10-porting-an-event-driven-node-to-a-plain-hyphanode) shows.

**Check:** each subscription has one input port, and each publication has one
output port.

### Step 3: Write the TOML file

Put the file beside the node, for example `config/<node>.toml`. Copy every
scalar value from the ROS launch and YAML files. Use the TOML type of the value: a
number is a number, not text. A YAML list maps to a TOML array, read with
`hypha::config().list(name)` ([cpp-api.md section 9](cpp-api.md#9-parameters)).

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
  apply the ROS namespace and remaps by hand and write the full result.
- An input that the deployment does not wire has an empty `topic` in
  `[ports.inputs.setpoint]` (`topic = ""`), and the port is declared with
  `optionalTopic`: `Latest<Float64> setpoint{*this, "setpoint", optionalTopic}`. The
  input is then disabled. Any other port fails the start when its table or topic is
  missing or empty. Add `advisory` when node health must not judge the input.
- A relative log path resolves against the directory of the TOML file.
- `${NAME}` in any string, `[log]` and `include` included, expands from the
  environment. An unset or empty variable stops the node.

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
#include <hypha_msgs/std_msgs/Float64.h>

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
- **Tests of a callback or a timer** run the step of a node that was not started
  through `hypha::testing::StepHarness` (`hypha/testing.h`). Keep the test name and
  every assertion. Add one test for each verdict that the ROS test did not cover:
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
  `docs/cpp-api.md`, "Test a step without a bus". The harness moves no clock: the
  step reads the `CycleTime` it is given, for its decisions and for the stamps it
  writes. Ported code that still calls `hypha::now()` gets the process clock in a
  harness step; until it is changed, the test sets simulated time to the step's time
  (`hypha::enableSimTime`, `hypha::setSimTime`). `steadyNow()` measures real elapsed
  time and is never set.
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
3. `hypha topic list` shows every topic of the inventory.
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
   For a plain node, `examples/sensor_pipeline/replay_check.sh` runs this replay and
   compares two outputs by stamp ([section 10.8](#108-a-runnable-sensor-node-and-its-replay-check)).
8. Negative start checks: remove a required `[ports.inputs.<name>]` table, write a number as
   text, pass a second argument. Each one stops the node with a message and a
   non-zero exit status.
9. `Ctrl-C` stops the node, and the process ends.

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
| `message_filters` synchroniser | `Arrivals` ports; match `header.stamp` in the step |
| `nh.advertise` + `publish` | `hypha::Output<T>` port: call `send(message)` |
| `nh.createTimer` | The cycle rate, `rate_hz` under `[cycle]` |
| `ros::TimerEvent` | `const hypha::CycleTime&`: `dt` (no value in the first step), `missed` |
| Hand-written message age check (decides whether an input may be used) | The port's `silence_ms`, verdict `Stale`. A use of the source time as data stays logic |
| `ros::Time::now()` for a stamp | In a cycle step `time.stamp`; elsewhere `hypha::toRosTime(hypha::now())`. See cpp-api.md §3 |
| `header.seq` | The publisher writes it |
| `ros::ServiceServer` | An `Arrivals<Srv::Request>` port on a request topic and an `Output<Srv::Response>` on a reply topic, matched by an identifier (cpp-api.md §8) |
| `ros::ServiceClient::call` | An `Output` of requests and an `Arrivals` of replies; the step tracks the pending request and its own deadline (cpp-api.md §8) |
| `ROS_INFO("%f", x)` | `HYPHA_LOG_INFO("{}", x)`: a `std::format` string |
| `ROS_WARN_THROTTLE(1, ...)` | `HYPHA_LOG_WARN_THROTTLE(1.0, ...)` |
| `ROS_WARN_ONCE`, `ROS_WARN_COND` | `HYPHA_LOG_WARN_WHEN(condition, false, ...)`: writes when the condition becomes true |
| `tf2_ros::TransformBroadcaster` | Output on `/tf` of type `tf2_msgs::TFMessage` |
| `tf2_ros::StaticTransformBroadcaster` | An `Output<tf2_msgs::TFMessage>` declared with `latched` and `topic = "/tf_static"` in its ports table |
| `use_sim_time` | `HYPHA_USE_SIM_TIME=1` in the node's environment; `follow` for `ros2 bag play`'s behaviour, outside the replay lockstep |
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
A fixed-size message is bounded to its own size whatever is named.

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
types therefore compiles with few edits. Make only these:

| ROS 1 | hypha | Edit |
|---|---|---|
| `#include <sensor_msgs/Imu.h>` | `#include <hypha_msgs/sensor_msgs/Imu.h>` | The include line. |
| `boost::array<double, 9>` | `std::array<double, 9>` | Only where the logic names the type. |
| `header.stamp` is `ros::Time` | `header.stamp` is `hypha_msgs::RosTime{secs, nsecs}` | `stamp.sec` to `stamp.secs`, `stamp.nsec` to `stamp.nsecs`. A call such as `stamp.toSec()` needs a helper; write one free function in the node file, do not edit each use. |
| `ros::Time`, `ros::Duration` in logic | `hypha::Time`, `hypha::Duration` | Only when the logic itself measures time. |
| `ros::Time::now()` in logic | `hypha::now()` for framework time, `hypha::steadyNow()` for an interval | Keep the meaning of the original call. |

A message type that hypha does not ship becomes a `.msg` file of the node and
a `hypha_generate_messages()` call. Copy the `.msg` file without changes.

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
- **`INPUTS_USABLE` is not readiness.** It means only that the inputs are
  fresh. Publish an application readiness topic when the system needs one.
- **One `[log]` table for each process.** A second node in the same process
  with a different table fails with `InvalidState`.
- **Stale shared memory after a crash** can look like a code fault. Run
  `./clean-shm.sh` before you look for a bug.
- **A backward seek or a loop of `hypha play --clock` restarts the node.** A cycle
  that sees simulated time go back ends the run with `ClockRewound`, the process
  exits 75 and its supervisor starts it again
  ([execution-model.md](execution-model.md) §11.1). Every member is built anew, so
  the node needs no reset code; do not write one.
- **`replay not faithful`, or `the input history is shorter than one replay chunk`,
  under `hypha play --clock`** is a sizing fault of the replay, not of the step. One
  chunk carried more samples of the topic than the input keeps. Check in this order:
  1. The line names the topic. Take its rate from the recording
     (`hypha topic hz` during an open-loop `hypha play`).
  2. Samples in one chunk = rate x `--chunk` (100 ms unless set). Compare with what
     the input keeps: the policy's `maxDepth` (32 for `default`) for a `Latest` port,
     `perStep(n)` (8 unless set) for an `Arrivals` port.
  3. Lower `--chunk <ms>` until a chunk fits, or give the topic a deeper policy or the
     port a larger `perStep` ([cpp-api.md](cpp-api.md) section 7). A burst counts by
     its peak, not by the mean rate.

---

## 7. Stop and ask

hypha has no equivalent for these. Stop and ask the user before you continue:

- A transform lookup that waits: `waitForTransform`, `lookupTransform` with a timeout,
  or `canTransform` in a loop. `hypha::TransformBuffer` lookups never wait (§10.6;
  [cpp-api.md](cpp-api.md) section 10.2): the node holds its message until a later
  callback finds the transform, or drops it.
- `dynamic_reconfigure`, or a parameter changed at run time. Parameters are
  read once at start.
- `actionlib`.
- `nodelet` and in-process zero-copy between nodes. hypha nodes are processes;
  shared memory gives zero-copy between them.
- A service whose caller blocks on the reply. hypha has no RPC: the call becomes a
  request topic and a reply topic, and the caller a state over steps.
- Missing or stale input with no rule in the ROS node. Ask which output is
  safe.
- A per-message latency requirement below one cycle period.
- A logic change that you think is necessary. Report it, do not make it.

Every item in this list is also a gap report: see section 8.

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
  stale shared memory with `./clean-shm.sh` in the hypha repository first, so
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
- [ ] `main` only calls `hypha::runNode`: one TOML file at most, a second argument stops the node with status 2.
- [ ] The logged snapshot shows the expected topics and period.
- [ ] Step 7 was done on the bus, and its results are recorded.
- [ ] A `hypha launch` unit and an installed TOML file exist.
- [ ] Every blocker, gap, defect and documentation fault found is reported
      to the developer in the form of section 8. No blocker is open.

---

## 10. Porting an event-driven node to a plain `hypha::Node`

Steps 1 to 8 end in a `CycleNode`. This section covers the other target: a plain
`hypha::Node` with typed subscriptions, publishers and timers. Step 1 (inventory),
Step 3 (TOML) and Step 4 (build) apply unchanged. The rules of
[section 2](#2-rules) apply, except the cycle items. The names below are in
[cpp-api.md](cpp-api.md) §1, §3, §4, §5, §7 and §9.

### 10.1 Which target

Use a plain `hypha::Node` when the node handles each message in its callback, has
one timer or a few, keeps its own worker threads, or needs a latency below one
cycle period. `Node::run()` calls every callback on the calling thread.
Use a `CycleNode` when the node computes at one rate over a frozen capture of its
inputs and needs the verdict of each input (`usable()`, `stale()`, `~/state`). A
plain node has none of them: freshness is judged for cycle inputs only
([cpp-api.md](cpp-api.md) §5.1), `runNode` is not used, so the node publishes no
`~/state` and `~/introspection` and refuses no unread parameter key
([cpp-api.md](cpp-api.md) §2). The model is in
[execution-model.md](execution-model.md) §3 and §4. Write the reason for a plain
node in the change description.

### 10.2 Literal topic or port name

`hypha::Topic{...}` names a literal topic that code fixes. A bare string names a
port table of the parameter file. ROS 1:

```cpp
std::string scanTopic;
pnh.param<std::string>("scan_topic", scanTopic, "/scan");   // string parameter
auto scan = nh.subscribe(scanTopic, 5, &Locator::onScan, this);
auto pose = nh.advertise<geometry_msgs::PoseStamped>("pose", 1);
```

hypha, with the topic names in the TOML file:

```cpp
hypha::Node node("locator", argv[1]);                       // argv[1]: the parameter file
auto scan = node.subscribe<sensor_msgs::PointCloud2>(
    "scan", hypha::bind<&Locator::onScan>(*this));          // [ports.inputs.scan]
auto pose = node.advertise<geometry_msgs::PoseStamped>("pose");   // [ports.outputs.pose]
auto tf = node.advertise<tf2_msgs::TFMessage>(hypha::Topic{"/tf"});  // a literal topic
```

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
auto odom = node.subscribe<nav_msgs::Odometry>(
    "odom", hypha::bind<&Locator::onOdom>(*this), hypha::optionalTopic);
if (!odom.enabled()) {
    HYPHA_LOG_INFO("odometry input is off");
}
```

- `runNode` refuses a parameter key that nothing read. A custom `main()` does not;
  it checks the keys itself, after it has read its parameters and created its
  endpoints:

```cpp
const std::vector<std::string> unread = hypha::config().unreadKeys();
if (!unread.empty()) {
    HYPHA_LOG_ERROR("unread parameter(s): {}", unread.front());
    return hypha::kExitConfiguration;
}
```

  `unreadKeys()` returns the unread keys sorted. `kExitConfiguration` is 78, the
  code that `runNode` returns for a refused configuration and that a supervisor
  does not restart (`hypha/application_node.h`).

### 10.3 Time, status and logging

| ROS 1 | hypha | Meaning |
|---|---|---|
| `msg.header.stamp.toSec()` | `hypha::fromRosTime(msg.header.stamp)` (`hypha/message_stamp.h`) | Returns `std::optional<hypha::Time>`. Empty when `nsecs` is one second or more. `Time::nanos()` gives integer nanoseconds. |
| `header.stamp = ros::Time::now()` | `header.stamp = hypha::toRosTime(hypha::now())` | Throws `std::out_of_range` for a time before the epoch or after 2106. |
| `ros::spin()` | `const hypha::RunResult result = node.run();` | `[[nodiscard]]`. `result.ok()` is true when the call succeeded and the run stopped as requested or at its time limit. `reason()`, `status()` and `call()` say why not. |
| `ros::shutdown()` from another thread | `hypha::requestStop()` | The one call that another thread may make on a running node. |
| `pub.publish(msg)` (no result) | `publish(msg)` returns `bool` (`[[nodiscard]]`); `lastPublishStatus()` returns the `Status` of the last attempt | The status names the reason: a disabled publisher, a message that is too large, an exhausted pool, a transport failure. |
| `ROS_WARN_THROTTLE(1, ...)` | `HYPHA_LOG_WARN_THROTTLE(1.0, "{}", x)` | The first call writes, later calls at most once per period. Host monotonic time. |
| `ROS_WARN_ONCE`, `ROS_WARN_COND` | `HYPHA_LOG_WARN_WHEN(state, false, ...)` | Writes when `state` changes from false to true at that call site. |
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

### 10.4 Queue depth

A subscriber queues the `maxDepth` of its topic's policy. The limit of 32 belongs
to the `default` policy. A topic that needs a deeper queue, such as a ROS IMU
subscriber with `queue_size` 200, needs its own policy. Declare it once, in the
policy header that every node of the stack includes, and map the message type to it:

```cpp
// my_stack/qos_policies.h: included by every node of the stack
#include <hypha/qos.h>
#include <hypha_msgs/sensor_msgs/Imu.h>

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

### 10.5 Threads

`Node::run()` runs every subscription and timer callback on the calling thread.
The framework creates no thread. One endpoint belongs to one thread at a time. A
worker thread that publishes therefore owns its publisher, or every access to that
publisher is serialized by the application. Only `hypha::requestStop()` may be
called from another thread while the node runs.

A timer counts the periods it did not fire in, because a callback or the run loop
ran longer than one period:

```cpp
hypha::Timer timer = node.timer(hypha::Duration::fromMillis(100), OnTick{&state});
...
const uint64_t missed = timer.missedCount();   // periods never fired in
```

`OnTick` is a named functor with `void operator()()`. Read `missedCount()` where
the node can log it. A subscriber counts the samples it lost before it read them, and
the frames it refused:

```cpp
const uint64_t lost = imu.missingSequences();      // frames lost before they were read
const hypha::SubscriberMetrics m = imu.metrics();  // m.sequenceGap, m.invalidFrame, ...
```

`missingSequences()` grows on a queue overflow. `m.sequenceGap` counts frames that
arrived and were refused because their sequence was not newer; a lost frame does not
raise it. Both may be read from any thread while the node runs
([cpp-api.md](cpp-api.md) §7).

### 10.6 TF in a plain node

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

hypha::TransformBuffer tf{node};                  // before node.run()
hypha::StaticTransformBroadcaster statics{node};

// in a callback, or in a worker thread while node.run() runs:
const std::optional<hypha::Time> stamp = hypha::fromRosTime(scan.header.stamp);
if (stamp.has_value()) {
    const auto found = tf.lookup("map", "lidar", *stamp);
    if (!found) {
        // found.error() is a hypha::TransformError: UnknownFrame, NotConnected,
        // BeforeBuffer, AfterBuffer or InvalidFrame
    }
}
```

  Create and destroy the buffer on the thread that configures the node, not while the
  node runs on another thread. Nothing is ingested unless the node runs. The buffer
  keeps 10 s of dynamic transforms per frame unless the constructor gets another
  duration. `hypha::Transforms` is the port of a cycle; its lookup throws
  `std::logic_error` outside a step.

### 10.7 A list of scalars in the parameter file

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

### 10.8 A runnable sensor node and its replay check

[examples/sensor_pipeline](../examples/sensor_pipeline/README.md) applies 10.2 to 10.7 in
one plain node: a 400 Hz IMU with `maxDepth = 200`, 10 Hz point clouds of 262144 bytes of
data, one timer, and one worker thread that owns the result publisher, looks the
transform up at the cloud's stamp and publishes. Its replay recipe, `replay_check.sh`,
runs the node on simulated time from a recorded fixture of 98296 bytes with `hypha play
--clock` and `hypha record --sync`, and makes three checks:

- **Loss.** The node prints `missingSequences()` and `metrics()` of each subscriber. A
  replay reports `missing=0`, also in slow mode (`[slow] enabled = true`): the player
  waits for the stalled node instead of overrunning its queues. Live, slow mode shows a
  growing count.
- **Timer misses.** The node prints `Timer::missedCount()`. In a replay its timer runs
  every period at its deadline and misses none; live, the stalls show as misses.
- **Comparison by stamp.** `sensor_pipeline_compare` reads two recordings with the test
  MCAP reader and pairs the outputs by `header.stamp`: two runs of the node, or a run and
  the ROS node's golden output.

A plain node on simulated time takes part in the replay (#72): its callbacks run in
time order at their events' times, and the player waits for it
([execution-model.md](execution-model.md) §11.2). Two replays of the recipe are
identical, and the slow run sets the replay's rate instead of losing samples. A worker
thread the node starts is outside that order.


### 10.9 The exit code of a custom `main`

`runNode` maps every way out to an exit code that a supervisor reads
(`hypha/application_node.h`). A custom `main` does the same itself. `result.ok() ? 0 : 1`
loses two facts: a refused configuration, which a restart cannot fix, and a backward
seek of the replay, after which the process must start again.

| Code | Constant | When | `hypha launch` |
|---|---|---|---|
| 0 | `kExitClean` | the run stopped as requested | the unit's restart policy |
| 2 | `kExitUsage` | wrong arguments | the unit's restart policy |
| 78 | `kExitConfiguration` | the parameter file, a parameter or an endpoint policy is refused; an unread key | never restarts |
| 75 | `kExitRewind` | simulated time went back: the run ended with `Status::ClockRewound` | always restarts |
| 70 | `kExitRuntime` | anything else | the unit's restart policy |

A plain node sees `ClockRewound` too: a timer on simulated time ends the run with it
([execution-model.md](execution-model.md) §11).

```cpp
#include <hypha/application_node.h>   // the exit codes
#include <hypha/hypha.h>

/// The exit code of a run that ended: 0, 75 after simulated time went back, else 70.
int exitCodeOf(const hypha::RunResult& result)
{
    const bool rewound = result.status() == hypha::Status::ClockRewound;
    if (rewound) {
        return hypha::kExitRewind;
    }
    return result.ok() ? hypha::kExitClean : hypha::kExitRuntime;
}

int main(int argc, char** argv)
{
    const bool tooManyArguments = argc > 2;
    if (tooManyArguments) {
        std::cerr << "usage: locator [parameter_file]\n";
        return hypha::kExitUsage;
    }
    const std::string parameterFile = argc == 2 ? std::string(argv[1]) : std::string{};
    try {
        // Loaded first and alone: a file that is missing or does not parse is a
        // configuration fault, which a restart cannot fix.
        const hypha::ConfigContext configuration(parameterFile);
        try {
            hypha::Node   node("locator", parameterFile);
            const Locator locator(node);   // reads its parameters, creates its endpoints
            const std::vector<std::string> unread = hypha::config().unreadKeys();
            if (!unread.empty()) {
                std::cerr << "unread parameter(s): " << unread.front() << '\n';
                return hypha::kExitConfiguration;
            }
            return exitCodeOf(node.run());
        } catch (const hypha::ConfigError& error) {   // ParameterError, QosMismatch
            std::cerr << "configuration refused: " << error.what() << '\n';
            return hypha::kExitConfiguration;
        } catch (const std::exception& error) {
            std::cerr << "start-up failed: " << error.what() << '\n';
            return hypha::kExitRuntime;
        }
    } catch (const std::exception& error) {
        std::cerr << "parameter file refused: " << error.what() << '\n';
        return hypha::kExitConfiguration;
    }
}
```

`crates/hypha-cxx/test/test_plain_main_example.cpp` compiles and runs this `main`. Test
the backward seek under the supervisor: `hypha play --clock --loop` restarts the unit at
each pass.
